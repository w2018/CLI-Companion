//! 临时验证：对真实 daemon（安装版）复刻端到端场景——
//! 创建"根 cmd 空闲 + powershell 子进程满载"服务，观察树聚合 CPU 是否非 0。
//!
//! 运行：cargo run -p cli-companion-daemon --example real_daemon_verify

use std::time::{Duration, Instant};

use cli_companion_domain::{Arg, ArgKind, ConsoleMode, ServiceDefinition};
use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use serde_json::json;
use tokio::net::windows::named_pipe::ClientOptions;

async fn call(
    method: Method,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    // 管道实例可能暂忙（os error 231），按 gui-core 同款策略重试
    let mut pipe = loop {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(p) => break p,
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("打开管道失败: {e}")),
        }
    };
    let req = Request::new(1, method, params);
    codec::write_frame(&mut pipe, &req)
        .await
        .map_err(|e| e.to_string())?;
    let resp: Response = codec::read_frame(&mut pipe)
        .await
        .map_err(|e| e.to_string())?;
    resp.into_result().map_err(|e| format!("{e}"))
}

#[tokio::main]
async fn main() {
    // 清理上次运行可能残留的同名服务
    if let Ok(list) = call(Method::ServiceList, None).await {
        for row in list["services"].as_array().unwrap_or(&vec![]) {
            if row["service"]["name"] == json!("探针-满载验证") {
                if let Some(old) = row["service"]["id"].as_str() {
                    println!("清理残留服务: {old}");
                    let _ = call(Method::ServiceStop, Some(json!({ "service_id": old }))).await;
                    let _ = call(Method::ServiceDelete, Some(json!({ "service_id": old }))).await;
                }
            }
        }
    }

    // 构造满载服务：根 cmd 启动 powershell 子进程满载，自身 ping 等待（近 0 CPU）
    let mut svc = ServiceDefinition::new(
        "探针-满载验证",
        std::path::PathBuf::from("C:\\Windows\\System32\\cmd.exe"),
    );
    let parts = [
        "/c",
        "start",
        "/b",
        "powershell",
        "-NoProfile",
        "-Command",
        "while($true){}",
        "&",
        "ping",
        "-n",
        "40",
        "127.0.0.1",
        ">nul",
    ];
    svc.args = parts
        .iter()
        .enumerate()
        .map(|(i, p)| Arg {
            id: format!("p{i}"),
            key: String::new(),
            value: Some((*p).to_string()),
            enabled: true,
            kind: ArgKind::Positional,
            description: String::new(),
        })
        .collect();
    svc.console.mode = ConsoleMode::NoConsole;

    let created = call(Method::ServiceCreate, Some(json!({ "service": svc })))
        .await
        .expect("创建服务失败");
    let sid = created["service"]["id"].as_str().unwrap().to_string();
    println!("服务已创建: {sid}");

    call(Method::ServiceStart, Some(json!({ "service_id": sid })))
        .await
        .expect("启动服务失败");
    println!("服务已启动，轮询 25s 观察树聚合 CPU ...");

    let mut max_cpu = 0.0f64;
    let deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(800)).await;
        if let Ok(m) = call(Method::ServiceMetrics, None).await {
            if let Some(entry) = m["metrics"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["service_id"] == json!(sid)))
            {
                if let Some(cpu) = entry["cpu_percent"].as_f64() {
                    max_cpu = max_cpu.max(cpu);
                    if max_cpu > 0.5 {
                        break;
                    }
                }
            }
        }
    }

    // 无论结果如何先停止并删除（满载进程不能残留）
    let _ = call(Method::ServiceStop, Some(json!({ "service_id": sid }))).await;
    let _ = call(Method::ServiceDelete, Some(json!({ "service_id": sid }))).await;

    println!("真实 daemon 树聚合 CPU 峰值: {max_cpu}%");
    if max_cpu > 0.5 {
        println!("✅ CPU 树聚合修复在安装版上验证通过");
    } else {
        println!("❌ 未观测到子进程满载占用，需要进一步排查");
        std::process::exit(1);
    }
}
