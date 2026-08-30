//! 临时 UI 验证辅助：创建/清理一个存活较久的临时服务，供界面截图验证。
//!
//! 用法：
//!   cargo run -p cli-companion-daemon --example temp_service -- start   # 创建并启动（打印 id）
//!   cargo run -p cli-companion-daemon --example temp_service -- stop <id> # 停止并删除

use std::time::Duration;

use cli_companion_domain::{Arg, ArgKind, ConsoleMode, ServiceDefinition};
use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use serde_json::json;
use tokio::net::windows::named_pipe::ClientOptions;

async fn call(method: Method, params: Option<serde_json::Value>) -> serde_json::Value {
    let mut pipe = loop {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(p) => break p,
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("打开管道失败: {e}"),
        }
    };
    let req = Request::new(1, method, params);
    codec::write_frame(&mut pipe, &req).await.unwrap();
    let resp: Response = codec::read_frame(&mut pipe).await.unwrap();
    resp.into_result().unwrap()
}

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "start" => {
            let mut svc = ServiceDefinition::new(
                "UI验证-临时",
                std::path::PathBuf::from("C:\\Windows\\System32\\PING.EXE"),
            );
            let parts = ["-n", "600", "127.0.0.1"];
            svc.args = parts
                .iter()
                .enumerate()
                .map(|(i, p)| Arg {
                    id: format!("t{i}"),
                    key: String::new(),
                    value: Some((*p).to_string()),
                    enabled: true,
                    kind: ArgKind::Positional,
                    description: String::new(),
                })
                .collect();
            svc.console.mode = ConsoleMode::NoConsole;
            let created = call(Method::ServiceCreate, Some(json!({ "service": svc }))).await;
            let sid = created["service"]["id"].as_str().unwrap().to_string();
            call(Method::ServiceStart, Some(json!({ "service_id": sid }))).await;
            println!("{sid}");
        }
        "stop" => {
            let sid = std::env::args().nth(2).expect("用法: stop <id>");
            let _ = call(Method::ServiceStop, Some(json!({ "service_id": sid }))).await;
            let _ = call(Method::ServiceDelete, Some(json!({ "service_id": sid }))).await;
            println!("已清理 {sid}");
        }
        _ => eprintln!("用法: temp_service start | stop <id>"),
    }
}
