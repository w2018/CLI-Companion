//! 托盘动态菜单：服务列表快捷启停
//!
//! - 初始与守护事件（配置/启停/健康变化）到达时异步重建"服务"子菜单
//! - 点击服务项直接向 daemon 发启停指令（不经过前端）
//! - 拉取失败/daemon 不可达时保留原菜单，只记日志——托盘永不拖垮 GUI

use crate::connection::DaemonConnection;
use cli_companion_protocol::{Event, EventTopic, Method};
use serde_json::Value;
use std::time::Duration;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

/// 这些守护事件会触发托盘菜单重建
pub fn should_rebuild_on(ev: &Event) -> bool {
    matches!(
        ev.topic,
        EventTopic::ConfigChanged
            | EventTopic::ServiceStarted
            | EventTopic::ServiceStopped
            | EventTopic::ServiceHealth
            | EventTopic::ServiceRestartAttempt
    )
}

/// 异步重建托盘菜单（失败静默）
pub fn schedule_rebuild(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = rebuild(&app).await {
            tracing::debug!("托盘服务菜单刷新失败: {e}");
        }
    });
}

/// 异步重建并重试：GUI 启动初期 daemon 可能尚未就绪（首次拉起需数秒），
/// 每 3 秒重试直至成功或达上限，保证"服务"子菜单最终一定出现
pub fn schedule_rebuild_with_retry(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for attempt in 0..20u32 {
            match rebuild(&app).await {
                Ok(()) => return,
                Err(e) if attempt == 19 => {
                    tracing::warn!("托盘服务菜单初始化失败（重试耗尽）: {e}");
                }
                Err(e) => tracing::debug!("托盘菜单初始化重试中: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

/// 初始托盘菜单（setup 阶段使用）：服务入口始终存在，daemon 就绪后由
/// schedule_rebuild 换成真实服务列表
pub fn build_initial_menu(app: &AppHandle) -> Result<Menu<Wry>, String> {
    build_menu(app, &[])
}

/// 托盘菜单点击：执行服务启停并刷新菜单
///
/// `action`：start | stop；`service_id`：服务 UUID 字符串。
pub fn run_service_action(app: &AppHandle, action: &str, service_id: &str) {
    let method = match action {
        "start" => Method::ServiceStart,
        "stop" => Method::ServiceStop,
        _ => return,
    };
    let app = app.clone();
    let sid = service_id.to_string();
    tauri::async_runtime::spawn(async move {
        let params = serde_json::json!({ "service_id": sid });
        if let Err(e) = DaemonConnection::call(method, Some(params)).await {
            tracing::warn!("托盘服务操作失败: {e}");
        }
        // 操作后立即刷新（事件流到达时也会触发一次，双保险）
        schedule_rebuild(&app);
    });
}

/// 拉取服务列表并重建菜单
async fn rebuild(app: &AppHandle) -> Result<(), String> {
    let rows = fetch_rows().await?;
    let Some(tray) = app.tray_by_id("main-tray") else {
        return Err("托盘不存在".into());
    };
    let menu = build_menu(app, &rows)?;
    tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
    Ok(())
}

/// 托盘展示用的服务行
struct TrayService {
    id: String,
    name: String,
    status: String,
}

async fn fetch_rows() -> Result<Vec<TrayService>, String> {
    let call = DaemonConnection::call(Method::ServiceList, None);
    let v = tokio::time::timeout(Duration::from_secs(3), call)
        .await
        .map_err(|_| "拉取服务列表超时".to_string())?
        .map_err(|e| e.message)?;
    let items = v
        .get("services")
        .and_then(Value::as_array)
        .ok_or_else(|| "service.list 响应无效".to_string())?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let s = item.get("service")?;
            Some(TrayService {
                id: s.get("id")?.as_str()?.to_string(),
                name: s.get("name")?.as_str()?.to_string(),
                status: item
                    .get("runtime")
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_str)?
                    .to_string(),
            })
        })
        .collect())
}

fn build_menu(app: &AppHandle, rows: &[TrayService]) -> Result<Menu<Wry>, String> {
    const E: fn(tauri::Error) -> String = |e| e.to_string();

    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>).map_err(E)?;
    let quit_gui = MenuItem::with_id(
        app,
        "quit_gui",
        "退出 GUI（服务保持运行）",
        true,
        None::<&str>,
    )
    .map_err(E)?;
    let quit_all = MenuItem::with_id(
        app,
        "quit_all",
        "完全退出（停止全部服务）",
        true,
        None::<&str>,
    )
    .map_err(E)?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(E)?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(E)?;

    // 服务子菜单：运行中 → 点击停止；其余 → 点击启动；切换中置灰
    let mut svc_items: Vec<MenuItem<Wry>> = Vec::with_capacity(rows.len().max(1));
    if rows.is_empty() {
        svc_items
            .push(MenuItem::with_id(app, "noop", "（暂无服务）", false, None::<&str>).map_err(E)?);
    }
    for s in rows {
        let (action, glyph) = match s.status.as_str() {
            "running" => ("stop", "■"),
            "starting" | "stopping" | "restarting" => ("none", "…"),
            _ => ("start", "▶"),
        };
        let label = match action {
            "stop" => format!("{glyph} {}　·　停止", s.name),
            "start" => format!("{glyph} {}　·　启动", s.name),
            _ => format!("{glyph} {}　·　状态切换中", s.name),
        };
        let id = format!("svc:{action}:{}", s.id);
        svc_items
            .push(MenuItem::with_id(app, &id, label, action != "none", None::<&str>).map_err(E)?);
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = svc_items
        .iter()
        .map(|i| i as &dyn IsMenuItem<Wry>)
        .collect();
    let services = Submenu::with_id_and_items(app, "services", "服务", true, &refs).map_err(E)?;

    let top: Vec<&dyn IsMenuItem<Wry>> = vec![&show, &sep1, &services, &sep2, &quit_gui, &quit_all];
    Menu::with_items(app, &top).map_err(E)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_companion_protocol::Event;

    #[test]
    fn 启停与配置事件触发重建() {
        let mk = |topic| Event {
            topic,
            service_id: None,
            payload: serde_json::json!({}),
            ts: chrono::Utc::now().to_rfc3339(),
        };
        assert!(should_rebuild_on(&mk(EventTopic::ConfigChanged)));
        assert!(should_rebuild_on(&mk(EventTopic::ServiceStarted)));
        assert!(should_rebuild_on(&mk(EventTopic::ServiceStopped)));
        assert!(should_rebuild_on(&mk(EventTopic::ServiceHealth)));
        assert!(!should_rebuild_on(&mk(EventTopic::SyncProgress)));
        assert!(!should_rebuild_on(&mk(EventTopic::DaemonShuttingDown)));
    }
}
