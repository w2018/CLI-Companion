//! CLI Companion 守护进程入口
//!
//! 运行模式：
//! - 默认：前台进程（开发 / 手动运行），可直接 Ctrl+C 停止
//! - `--service`：作为 Win32 服务运行（SCM 调度）
//! - `--install-service` / `--uninstall-service`：安装/卸载服务（需管理员权限）
//! - `--portable`：便携模式，数据目录固定为 exe 所在目录（写入 portable.marker）
//! - `--data-dir <dir>`：显式指定数据目录（开发用）
//!
//! release 构建为 windows 子系统（无控制台）：开机自启经注册表 Run 项拉起时
//! 不闪黑框；日志走 daemon.log 文件，不依赖控制台。debug 构建保留控制台便于开发。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--install-service") {
        if let Err(e) = cli_companion_daemon::service::install() {
            eprintln!("安装服务失败: {e:#}");
            std::process::exit(1);
        }
        println!("服务安装成功");
        return;
    }
    if args.iter().any(|a| a == "--uninstall-service") {
        if let Err(e) = cli_companion_daemon::service::uninstall() {
            eprintln!("卸载服务失败: {e:#}");
            std::process::exit(1);
        }
        println!("服务卸载成功");
        return;
    }
    if args.iter().any(|a| a == "--service") {
        // Win32 服务模式：入口交由 SCM 调度
        cli_companion_daemon::service::run_dispatcher();
        return;
    }

    // v2.2.0：看门狗心跳（计划任务每 5 分钟调用）——可达即退出，否则静默拉起后退出
    if args.iter().any(|a| a == "--watchdog-check") {
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        let _ = rt.block_on(cli_companion_daemon::watchdog::ensure_daemon_if_needed());
        return;
    }

    // 前台模式
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    if let Err(e) = rt.block_on(run_foreground()) {
        eprintln!("daemon 退出: {e:#}");
        std::process::exit(1);
    }
}

/// 前台运行：Ctrl+C 触发优雅关闭
async fn run_foreground() -> anyhow::Result<()> {
    tracing::info!("daemon 以前台模式启动");
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("收到 Ctrl+C，开始优雅关闭");
        let _ = stop_tx.send(());
    });
    cli_companion_daemon::state::bootstrap(stop_rx, false).await
}
