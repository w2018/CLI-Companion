//! FTP 手动调试探针：拉起 127.0.0.1:2121 的 FTP 服务端，供 curl -v 手工互操作
//! 用法：cargo run -p cli-companion-daemon --example ftp_probe
//! 用户 power / pw，全权限，根目录 = 临时目录 home，被动区间 0-0（临时端口）

use cli_companion_daemon::app_config::{
    FtpListener, FtpPermissions, FtpSettings, FtpUser, Secrets,
};
use cli_companion_daemon::ftp::{run_server_on, BoundListener, FtpServerShared};
use cli_companion_daemon::state::{AppState, ConfigStore};
use cli_companion_daemon::{dirs::DataDirs, events, manager::ServiceManager, sync::SyncEngine};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let tmp = std::env::temp_dir().join("cc-ftp-probe");
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("welcome.txt"), "hello-ftp-probe").unwrap();

    let dirs = DataDirs::resolve(Some(tmp.clone()));
    let app = cli_companion_daemon::app_config::AppConfig {
        ftp: FtpSettings {
            enabled: true,
            autostart: false,
            passive_port_start: 0,
            passive_port_end: 0,
            listeners: vec![FtpListener {
                name: "探针".into(),
                port: 2121,
                root: home.clone(),
                enabled: true,
            }],
            users: vec![FtpUser {
                username: "power".into(),
                allowed_roots: vec![home.clone()],
                permissions: FtpPermissions {
                    list: true,
                    download: true,
                    upload: true,
                    delete: true,
                    rename: true,
                    mkdir: true,
                },
                enabled: true,
            }],
        },
        ..Default::default()
    };
    let mut secrets = Secrets::default();
    secrets.set_ftp_password("power", "pw").unwrap();

    let state = AppState {
        as_service: false,
        manager: Arc::new(ServiceManager::new(
            dirs.clone(),
            Arc::new(events::new_bus()),
            false,
        )),
        config: Arc::new(tokio::sync::Mutex::new(ConfigStore {
            services: Default::default(),
            app,
            secrets,
        })),
        sync: Arc::new(SyncEngine::new()),
        events: Arc::new(events::new_bus()),
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    };

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let sd = Arc::new(tokio::sync::Notify::new());
    let root = std::fs::canonicalize(&home).unwrap();
    let root_disp = root.display().to_string();
    tokio::spawn(run_server_on(
        state.clone(),
        vec![BoundListener {
            name: "探针".into(),
            root,
            listener,
        }],
        (0, 0),
        Arc::new(FtpServerShared::default()),
        sd.clone(),
    ));
    println!("PORT={port}");
    println!("FTP 探针已启动: ftp://127.0.0.1:{port}/  用户 power/pw  根 {root_disp}");
    // 存活 300 秒供手工 curl 调试
    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    sd.notify_waiters();
}
