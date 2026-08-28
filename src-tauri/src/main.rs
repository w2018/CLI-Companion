//! CLI Companion GUI 入口
//!
//! - 单实例：二次启动时唤起已有窗口（tauri-plugin-single-instance）
//! - 注册 gui-core 的 RPC 桥接命令与 daemon 自动拉起命令
//! - 托盘：右键菜单（显示 / 退出 GUI / 完全退出），左键单击切换窗口显隐
//! - 关闭窗口行为由前端控制（托盘隐藏 或 弹窗确认退出）
//! - panic 时写入 crash 日志，便于诊断"闪退"类问题

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

fn main() {
    install_panic_logger();

    tauri::Builder::default()
        // 单实例必须最先注册：二次启动时唤起已有窗口后立即退出新进程
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            gui_core::commands::daemon_rpc,
            gui_core::commands::daemon_status,
            gui_core::commands::ensure_daemon,
            gui_core::commands::exit_app
        ])
        .setup(|app| {
            setup_tray(app)?;
            Ok(())
        })
        // 关闭行为完全由前端 onCloseRequested 处理（托盘隐藏 / 弹窗确认退出）。
        // 注意：Rust 侧不得再拦截 CloseRequested —— 否则窗口会在 JS 弹窗出现前
        // 被隐藏，导致确认框"未生效"。
        .run(tauri::generate_context!())
        .expect("CLI Companion 启动失败");
}

/// panic 日志：写入 exe 同目录 logs/gui-crash.log（闪退时可查原因）
fn install_panic_logger() {
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("logs").join("gui-crash.log")));
    std::panic::set_hook(Box::new(move |info| {
        let Some(path) = &log_path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = format!("[unix:{secs}] PANIC: {info}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, entry.as_bytes()));
    }));
}

/// 创建系统托盘（右键弹菜单；左键单击切换窗口显隐）
fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_gui = MenuItem::with_id(app, "quit_gui", "退出 GUI（服务保持运行）", true, None::<&str>)?;
    let quit_all = MenuItem::with_id(app, "quit_all", "完全退出（停止全部服务）", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit_gui, &quit_all])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("CLI Companion")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit_gui" => {
                // 仅退出 GUI；daemon 作为独立进程/服务继续运行
                app.exit(0);
            }
            "quit_all" => {
                // 完全退出：显示窗口并通知前端执行"逐条停止服务"进度流程，
                // 前端完成后自行销毁窗口退出；此处仅做兜底定时退出
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                    let _ = win.emit("quit-all-requested", ());
                }
                // 兜底：前端无响应（webview 未加载等异常）时强制退出；
                // 正常路径由前端完成停止进度后自行销毁窗口，远快于此
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 仅左键单击抬起时切换窗口显隐（右键交给系统菜单）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let visible = win.is_visible().unwrap_or(false);
                    let _ = if visible {
                        win.hide()
                    } else {
                        let _ = win.show();
                        win.set_focus()
                    };
                }
            }
        });

    // 图标：优先嵌入的应用图标；缺失时 Tauri 用系统默认占位，不 panic
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}
