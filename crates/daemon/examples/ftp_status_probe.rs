//! 临时探针：连接真实 daemon 命名管道，调用 ftp.status 验证部署版响应
//! 运行：cargo run -p cli-companion-daemon --example ftp_status_probe

use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use tokio::net::windows::named_pipe::ClientOptions;

async fn call(method: Method) -> Result<serde_json::Value, String> {
    // ERROR_PIPE_BUSY 重试（并发场景管道实例占满很常见）
    const ERROR_PIPE_BUSY: i32 = 232;
    let mut pipe = None;
    for _ in 0..20 {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(p) => {
                pipe = Some(p);
                break;
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("打开管道失败: {e}")),
        }
    }
    let mut pipe = pipe.ok_or("管道持续繁忙")?;
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
    match call(Method::FtpStatus).await {
        Ok(v) => println!(
            "ftp.status: {}",
            serde_json::to_string_pretty(&v).unwrap_or_default()
        ),
        Err(e) => {
            eprintln!("ftp.status 失败: {e}");
            std::process::exit(1);
        }
    }
}
