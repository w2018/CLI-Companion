//! daemon 事件流订阅转发：管道长连接 → Tauri 事件
//!
//! GUI 全生命周期内持有一条 event.subscribe 订阅连接，
//! daemon 推送的事件逐帧解析后通过 `daemon-event` 转发给前端；
//! 连接断开自动重连（daemon 未运行时静默重试）。

use crate::connection::DaemonConnection;
use cli_companion_protocol::codec;
use cli_companion_protocol::{Method, Request, Response};
use tauri::Emitter;

/// 转发给前端的事件名（前端 listen("daemon-event") 接收）
pub const EVENT_NAME: &str = "daemon-event";

/// 断线重连间隔
const RETRY_SECS: u64 = 3;

/// 启动事件转发后台任务（GUI 退出时任务随之结束）
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(e) = run_once(&app).await {
                tracing::debug!("事件流断开，{RETRY_SECS}s 后重连: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(RETRY_SECS)).await;
        }
    });
}

/// 单轮：连接 → 订阅 → 循环转发事件帧
async fn run_once(app: &tauri::AppHandle) -> Result<(), String> {
    let mut pipe = super::connection::open_pipe()
        .await
        .map_err(|_| "daemon 管道不可达".to_string())?;
    let req = Request::new(DaemonConnection::next_id(), Method::EventSubscribe, None);
    codec::write_frame(&mut pipe, &req)
        .await
        .map_err(|e| e.to_string())?;
    let resp: Response = codec::read_frame(&mut pipe)
        .await
        .map_err(|e| e.to_string())?;
    resp.into_result().map_err(|e| e.to_string())?;
    tracing::info!("事件流已连接");
    loop {
        let ev: cli_companion_protocol::Event = codec::read_frame(&mut pipe)
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit(EVENT_NAME, serde_json::to_value(&ev).unwrap_or_default());
    }
}
