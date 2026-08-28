//! Tauri 命令定义（放在子模块以避免宏命名冲突）

use crate::connection::DaemonConnection;
use cli_companion_protocol::error::ErrorCode;

/// 通用 RPC 转发命令：前端 invoke("daemon_rpc", { method, params })
///
/// 错误以 JSON 字符串返回（RpcError 序列化），前端解析后得到稳定错误码。
///
/// 容错：若 daemon 未运行（已被停止或崩溃），自动从同目录拉起后重试一次，
/// 保证 GUI 任意操作（含同步）都不需要用户手动启动 daemon。
#[tauri::command]
pub async fn daemon_rpc(
    method: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    // 方法名 → 枚举（未知方法直接拒绝，fail closed）
    let method_value = serde_json::Value::String(method);
    let method: cli_companion_protocol::Method =
        serde_json::from_value(method_value).map_err(|e| format!("未知或非法的方法名: {e}"))?;

    // 直接调用（daemon 运行时零额外开销）
    match DaemonConnection::call(method, params.clone()).await {
        Ok(v) => Ok(v),
        Err(e) if e.code == ErrorCode::DaemonUnavailable => {
            // daemon 不可达 → 自动拉起 → 重试一次
            tracing::warn!("daemon 不可达，自动拉起后重试: {e}");
            DaemonConnection::ensure_daemon().await?;
            match DaemonConnection::call(method, params).await {
                Ok(v) => Ok(v),
                Err(e2) => {
                    tracing::warn!("拉起后重试仍失败: {e2}");
                    Err(serde_json::to_string(&e2).unwrap_or(e2.message))
                }
            }
        }
        Err(e) => {
            tracing::warn!("RPC 失败: {e}");
            Err(serde_json::to_string(&e).unwrap_or(e.message))
        }
    }
}

/// 连接状态探测（比完整 RPC 更轻量）
#[tauri::command]
pub async fn daemon_status() -> Result<bool, String> {
    Ok(DaemonConnection::is_alive().await)
}

/// 确保 daemon 在运行（未运行则从同目录拉起）；前端启动时首先调用
#[tauri::command]
pub async fn ensure_daemon() -> Result<bool, String> {
    DaemonConnection::ensure_daemon().await
}

/// 退出 GUI 应用（不经过窗口关闭流程，daemon 作为独立进程不受影响）
///
/// 此前前端用 window.destroy() 退出，但 capabilities 未授予
/// core:window:allow-destroy 权限导致调用被静默拒绝、界面"卡住无反应"。
#[tauri::command]
pub async fn exit_app(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    #[allow(unreachable_code)]
    Ok(())
}
