//! Win32 服务模式：SCM 调度、安装、卸载（用户决策：V1 daemon 以 Win32 服务运行）

use crate::state::bootstrap;
use anyhow::{anyhow, Context, Result};
use std::ffi::OsString;
use std::time::Duration;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher, service_manager};

/// 服务名（SCM 注册名）
pub const SERVICE_NAME: &str = "CliCompanionDaemon";
/// 显示名
pub const DISPLAY_NAME: &str = "CLI Companion Daemon";

define_windows_service!(ffi_service_main, service_main);

/// SCM 调度入口（阻塞至服务停止）
pub fn run_dispatcher() {
    if let Err(e) = service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        eprintln!("服务调度启动失败: {e}");
        std::process::exit(1);
    }
}

/// 服务主函数：注册控制处理器 → 运行 bootstrap
fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_as_service() {
        eprintln!("服务运行失败: {e:#}");
    }
}

fn run_as_service() -> Result<()> {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    // FnMut 闭包不能移动 stop_tx：用 Option+Mutex 包装实现"只发送一次"
    let stop_tx = std::sync::Mutex::new(Some(stop_tx));
    let handler = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop => {
            if let Some(tx) = stop_tx.lock().ok().and_then(|mut g| g.take()) {
                let _ = tx.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| anyhow!("注册服务控制处理器失败: {e}"))?;

    // 状态上报闭包
    let set_status = |state: ServiceState, accept: ServiceControlAccept| {
        handler
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            })
            .map_err(|e| anyhow!("set_service_status 失败: {e}"))
    };

    set_status(ServiceState::StartPending, ServiceControlAccept::empty())?;

    // 在服务线程内构建 tokio runtime
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("创建 tokio runtime 失败")?;
    let result = rt.block_on(bootstrap(stop_rx, true));

    set_status(ServiceState::Stopped, ServiceControlAccept::empty())?;
    result
}

/// 安装服务（需管理员权限）；服务 exe 使用当前进程路径
pub fn install() -> Result<()> {
    let manager = service_manager::ServiceManager::local_computer(
        None::<&str>,
        service_manager::ServiceManagerAccess::CONNECT
            | service_manager::ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| anyhow!("打开 SCM 失败: {e}"))?;
    let exe = std::env::current_exe().context("获取当前 exe 路径失败")?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::START)
        .map_err(|e| anyhow!("创建服务失败: {e}"))?;
    service
        .start(&[] as &[&std::ffi::OsStr])
        .map_err(|e| anyhow!("启动服务失败: {e}"))?;
    Ok(())
}

/// 卸载服务（需管理员权限）；运行中先停止
pub fn uninstall() -> Result<()> {
    let manager = service_manager::ServiceManager::local_computer(
        None::<&str>,
        service_manager::ServiceManagerAccess::CONNECT,
    )
    .map_err(|e| anyhow!("打开 SCM 失败: {e}"))?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE)
        .map_err(|e| anyhow!("打开服务失败: {e}"))?;
    // 忽略停止失败（可能未运行）
    let _ = service.stop();
    service.delete().map_err(|e| anyhow!("删除服务失败: {e}"))?;
    Ok(())
}
