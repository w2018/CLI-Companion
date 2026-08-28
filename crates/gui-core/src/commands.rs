//! Tauri 命令定义（放在子模块以避免宏命名冲突）

use crate::connection::DaemonConnection;
use cli_companion_protocol::error::ErrorCode;
use std::sync::atomic::{AtomicBool, Ordering};

/// 显式停止 daemon 后抑制"不可达即自动拉起"。
///
/// 否则前端的周期轮询（服务列表 8s 等）会在 daemon 停止后触发自动拉起，
/// daemon 复活并按 autostart 把服务全部重新拉起来 —— "停止 daemon"形同虚设。
/// 仅设置页手动"启动 daemon"（ensure_daemon 命令）会解除抑制。
static AUTO_SPAWN_SUPPRESSED: AtomicBool = AtomicBool::new(false);

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
            // 用户刚显式停止 daemon：不自动拉起，直接把"不可达"返回给前端
            if AUTO_SPAWN_SUPPRESSED.load(Ordering::SeqCst) {
                return Err(serde_json::to_string(&e).unwrap_or(e.message));
            }
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

/// 确保 daemon 在运行（未运行则从同目录拉起）；前端启动时首先调用。
/// 这是显式的用户动作：解除"停止后抑制自动拉起"。
#[tauri::command]
pub async fn ensure_daemon() -> Result<bool, String> {
    AUTO_SPAWN_SUPPRESSED.store(false, Ordering::SeqCst);
    DaemonConnection::ensure_daemon().await
}

/// 读取开机自启模式（off | daemon | both；从未配置默认 daemon）
#[tauri::command]
pub async fn get_boot_autostart_mode() -> Result<String, String> {
    crate::autostart::get_mode()
}

/// 设置开机自启模式（off | daemon | both），立即同步登录启动项
#[tauri::command]
pub async fn set_boot_autostart_mode(mode: String) -> Result<(), String> {
    crate::autostart::set_mode(&mode)
}

/// 设置 daemon 自动拉起开关（显式停止 daemon 前由前端关闭）
#[tauri::command]
pub async fn set_daemon_autostart(allowed: bool) -> Result<(), String> {
    AUTO_SPAWN_SUPPRESSED.store(!allowed, Ordering::SeqCst);
    Ok(())
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

/// 读取文本文件（配置导入用；限 4MB 防误读大文件）
#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("无法访问文件: {e}"))?;
    if meta.len() > 4 * 1024 * 1024 {
        return Err("文件超过 4MB，不是有效的配置备份".into());
    }
    std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))
}

/// 写入文本文件（配置导出用）
#[tauri::command]
pub async fn write_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("写入失败: {e}"))
}
