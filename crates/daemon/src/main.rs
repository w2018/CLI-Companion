//! CLI Companion 守护进程入口
//!
//! 运行模式：
//! - 默认：前台进程（开发 / 手动运行），可直接 Ctrl+C 停止
//! - `--service`：作为 Win32 服务运行（SCM 调度）
//! - `--install-service` / `--uninstall-service`：安装/卸载服务（需管理员权限）

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
