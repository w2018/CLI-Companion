//! 临时探针：连接真实 daemon 命名管道，验证 v2.4.0 service.metrics
//! 新字段（GPU/磁盘/网络）与 CPU 树聚合采样，并检测本机 GPU 计数器可用性。
//!
//! 运行：cargo run -p cli-companion-daemon --example metrics_probe

use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use tokio::net::windows::named_pipe::ClientOptions;

async fn call(method: Method) -> Result<serde_json::Value, String> {
    let mut pipe = ClientOptions::new()
        .open(PIPE_NAME)
        .map_err(|e| format!("打开管道失败: {e}"))?;
    let req = Request::new(1, method, None);
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
    // 1. system.ping：确认 daemon 版本
    match call(Method::SystemPing).await {
        Ok(v) => println!("ping: {v}"),
        Err(e) => {
            eprintln!("ping 失败: {e}");
            std::process::exit(1);
        }
    }

    // 2. GPU 可用性（本机是否具备 GPU Engine 计数器）
    {
        let mut g = cli_companion_platform::gpu::GpuMonitor::default();
        let pids = vec![std::process::id()];
        let _ = g.sample(&pids);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let s = g.sample(&pids);
        println!(
            "GPU 探针（本机）: {}",
            match s {
                Some(x) => format!("可用 利用率={:.1}% 显存={}B", x.percent, x.mem_bytes),
                None => "不可用（无 WDDM GPU/虚拟机，前端将隐藏 GPU 项）".into(),
            }
        );
    }

    // 3. service.metrics：等待采样生效后读取两次
    for round in 1..=2 {
        std::thread::sleep(std::time::Duration::from_millis(2300));
        match call(Method::ServiceMetrics).await {
            Ok(v) => println!(
                "metrics 第{round}次: {}",
                serde_json::to_string_pretty(&v).unwrap_or_default()
            ),
            Err(e) => eprintln!("metrics 失败: {e}"),
        }
    }
}
