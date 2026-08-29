//! AppState：daemon 全局共享状态 + 启动引导流程（开发文档 §3.1）

use crate::app_config::{load_app, load_secrets, save_app, save_secrets, AppConfig, Secrets};
use crate::dirs::{atomic_write, DataDirs};
use crate::events::{make_event, EventTx};
use crate::manager::ServiceManager;
use crate::sync::SyncEngine;
use cli_companion_domain::{ServiceStatus, ServicesConfig};
use cli_companion_platform::lock::{LockError, SingletonLock};
use cli_companion_protocol::EventTopic;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// daemon 全局状态（Clone 为浅拷贝）
#[derive(Clone)]
pub struct AppState {
    pub dirs: DataDirs,
    /// 是否为 Win32 服务模式
    pub as_service: bool,
    pub manager: Arc<ServiceManager>,
    /// 配置存储（services + app + secrets），写锁保护
    pub config: Arc<AsyncMutex<ConfigStore>>,
    pub sync: Arc<SyncEngine>,
    /// 事件总线（event.subscribe 订阅者接收广播）
    pub events: Arc<EventTx>,
    /// 关闭通知
    pub shutdown: Arc<tokio::sync::Notify>,
}

impl AppState {
    /// 广播事件（无订阅者时静默忽略）
    pub fn emit(&self, topic: EventTopic, service_id: Option<String>, payload: serde_json::Value) {
        let ev = make_event(topic, service_id, payload);
        let _ = self.events.send(ev);
    }
}

/// 配置存储
pub struct ConfigStore {
    pub services: ServicesConfig,
    pub app: AppConfig,
    pub secrets: Secrets,
}

impl AppState {
    /// 保存 services.json 并同步 actor 表
    pub async fn save_services(&self, cfg: ServicesConfig) -> Result<(), String> {
        cfg.validate().map_err(|e| e.to_string())?;
        // v2.2.0：写盘前快照当前配置（自动备份，失败不影响保存）
        crate::backup::snapshot_before_save(&self.dirs);
        let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        atomic_write(&self.dirs.services_json(), &json).map_err(|e| e.to_string())?;
        let mut store = self.config.lock().await;
        store.services = cfg;
        let ids: Vec<_> = store.services.services.iter().map(|s| s.id).collect();
        drop(store);
        self.manager.sync_actors(&ids);
        Ok(())
    }

    /// 保存 app.json
    pub async fn save_app(&self, app: AppConfig) -> Result<(), String> {
        save_app(&self.dirs, &app).map_err(|e| e.to_string())?;
        self.config.lock().await.app = app;
        Ok(())
    }

    /// 保存 secrets
    pub async fn save_secrets(&self, secrets: Secrets) -> Result<(), String> {
        save_secrets(&self.dirs, &secrets).map_err(|e| e.to_string())?;
        self.config.lock().await.secrets = secrets;
        Ok(())
    }

    /// 当前 services 配置快照
    pub async fn services(&self) -> ServicesConfig {
        self.config.lock().await.services.clone()
    }

    /// 当前 app 配置快照
    pub async fn app(&self) -> AppConfig {
        self.config.lock().await.app.clone()
    }

    /// 启动所有 autostart 服务（开发文档 §3.1 恢复策略）
    pub async fn startup_autostart(&self) {
        let cfg = self.services().await;
        for svc in cfg.services {
            if svc.autostart && svc.enabled {
                tracing::info!(service = %svc.id, "恢复 autostart 服务: {}", svc.name);
                if let Err(e) = self.manager.start(&svc).await {
                    tracing::error!(service = %svc.id, "autostart 失败: {e}");
                }
            }
        }
    }

    /// 优雅关闭：停服务 → 通知
    pub async fn shutdown(&self, stop_services: bool) {
        if stop_services {
            tracing::info!("daemon 关闭：停止所有受管服务");
            self.manager.stop_all().await;
        }
        self.shutdown.notify_waiters();
    }
}

/// 启动引导：日志 → 单例锁 → 加载配置 → autostart → 同步调度 → 管道服务
pub async fn bootstrap(
    stop_rx: tokio::sync::oneshot::Receiver<()>,
    as_service: bool,
) -> anyhow::Result<()> {
    // 1. 目录与日志
    let dirs = DataDirs::resolve(crate::dirs::data_dir_override());
    let file_appender = tracing_appender::rolling::never(&dirs.logs, "daemon.log");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::sync::Mutex::new(file_appender))
        .with_ansi(false)
        .try_init()
        .ok();

    // 2. 单例锁（旧实例正在退出时锁未释放：等待重试而非直接退出，
    //    解决"刚停止就重启"时新 daemon 抢不到锁的问题）
    let lock_path = dirs.daemon_lock();
    let mut lock: Option<SingletonLock> = None;
    for attempt in 0..75u32 {
        match SingletonLock::acquire(&lock_path) {
            Ok(l) => {
                lock = Some(l);
                break;
            }
            Err(LockError::AlreadyRunning(p)) => {
                if attempt == 0 {
                    tracing::info!("检测到旧实例（锁: {p}），等待其退出…");
                }
                // 已有健康实例正在服务 → 立即退出，避免产生多余的 daemon 进程
                // （管道不可达说明旧实例正在退出，继续等待锁以支持"刚停就启"）
                if crate::rpc::existing_daemon_alive().await {
                    return Err(anyhow::anyhow!("已有 daemon 实例在服务，本实例退出"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let _lock =
        lock.ok_or_else(|| anyhow::anyhow!("等待 15 秒后旧 daemon 实例仍未退出，本实例放弃启动"))?;
    tracing::info!(root = %dirs.root.display(), "daemon 启动");

    // 3. 加载配置（损坏时备份为 .corrupt 并使用默认配置）
    let services = load_services_with_recovery(&dirs);
    let app = load_app(&dirs);
    let secrets = load_secrets(&dirs);
    if let Err(e) = services.validate() {
        tracing::warn!("services.json 校验失败（仍已加载）: {e}");
    }

    // 4. 构建状态
    let events = Arc::new(crate::events::new_bus());
    let state = AppState {
        as_service,
        manager: Arc::new(ServiceManager::new(
            dirs.clone(),
            events.clone(),
            as_service,
        )),
        config: Arc::new(AsyncMutex::new(ConfigStore {
            services,
            app,
            secrets,
        })),
        sync: Arc::new(SyncEngine::new()),
        events,
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    };

    // 5. 恢复 autostart 服务（先做一次明文机密迁移，幂等）
    crate::secrets_env::migrate_existing(&state).await;
    state.startup_autostart().await;

    // 6. WebDAV 周期同步调度
    crate::sync::spawn_scheduler(state.clone());

    // 6.5 v2.2.0：本机只读状态页（按开关；修改开关后需重启 daemon 生效）
    let app_cfg = state.app().await;
    crate::status_http::spawn_if_enabled(
        state.clone(),
        app_cfg.status_page.enabled,
        app_cfg.status_page.port,
    );

    // 7. 管道 RPC 服务 + 关闭等待
    let shutdown = state.shutdown.clone();
    tokio::select! {
        r = crate::rpc::run_pipe_server(state) => {
            if let Err(e) = r {
                tracing::error!("管道服务异常退出: {e}");
            }
        }
        _ = shutdown.notified() => {
            tracing::info!("daemon 已停止接受连接");
        }
        _ = wait_stop(stop_rx) => {
            tracing::info!("收到外部停止信号");
        }
    }
    Ok(())
}

/// 等待外部停止信号（服务模式或 Ctrl+C）
async fn wait_stop(rx: tokio::sync::oneshot::Receiver<()>) {
    let _ = rx.await;
}

/// 加载 services.json；损坏时把坏文件改名为 .corrupt 并返回默认配置
fn load_services_with_recovery(dirs: &DataDirs) -> ServicesConfig {
    let path = dirs.services_json();
    match std::fs::read_to_string(&path) {
        Ok(raw) => match ServicesConfig::from_json(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!("services.json 无效: {e}，已备份为 .corrupt 并使用默认配置");
                let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                ServicesConfig::default()
            }
        },
        Err(_) => {
            // 首次运行：写入默认配置
            let default = ServicesConfig::default();
            if let Ok(json) = serde_json::to_string_pretty(&default) {
                let _ = atomic_write(&path, &json);
            }
            default
        }
    }
}

/// 状态健康检查工具：判断给定运行时状态集合中是否有活跃服务
#[allow(dead_code)] // 供集成测试使用
pub fn has_running(
    runtimes: &std::collections::HashMap<cli_companion_domain::ServiceId, ServiceStatus>,
) -> bool {
    runtimes
        .values()
        .any(|s| matches!(s, ServiceStatus::Running | ServiceStatus::Starting))
}
