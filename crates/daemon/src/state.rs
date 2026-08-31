//! AppState：daemon 全局共享状态 + 启动引导流程（开发文档 §3.1）

use crate::app_config::{load_app, load_secrets, save_app, save_secrets, AppConfig, Secrets};
use crate::dirs::{atomic_write, DataDirs};
use crate::events::{make_event, EventTx};
use crate::manager::ServiceManager;
use crate::sync::SyncEngine;
use cli_companion_domain::{ServiceStatus, ServicesConfig};
use cli_companion_platform::lock::{LockError, SingletonLock};
use cli_companion_protocol::EventTopic;
use std::sync::atomic::{AtomicU64, Ordering};
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
        // v2.6.0：FTP 配置校验（监听器/用户/端口区间）
        app.ftp
            .validate()
            .map_err(|e| format!("FTP 配置无效: {e}"))?;
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

// ===== daemon 自身进程指标采样（供 FTP 状态卡等使用）=====

/// daemon 自身的 CPU/内存指标（每 2s 采样，供 ftp.status 等读取）
#[derive(Debug, Default)]
pub struct DaemonMetrics {
    /// CPU 使用率百分比（0-100），None = 尚未采样
    pub cpu_percent: std::sync::Mutex<Option<f32>>,
    /// 内存工作集字节
    pub mem_bytes: AtomicU64,
    /// 内存占系统百分比
    pub mem_percent: std::sync::Mutex<Option<f32>>,
}

/// 全局 daemon 指标（OnceLock，bootstrap 初始化一次）
static DAEMON_METRICS: std::sync::OnceLock<Arc<DaemonMetrics>> = std::sync::OnceLock::new();

/// 读取 daemon 指标快照
pub fn daemon_metrics_snapshot() -> (Option<f32>, u64, Option<f32>) {
    let m = DAEMON_METRICS.get().unwrap();
    let cpu = *m.cpu_percent.lock().unwrap();
    let mem = m.mem_bytes.load(Ordering::Relaxed);
    let pct = *m.mem_percent.lock().unwrap();
    (cpu, mem, pct)
}

/// 采样 daemon 自身进程指标（2s 间隔，spawn 在 bootstrap 中）
pub fn spawn_daemon_sampler(metrics: Arc<DaemonMetrics>, shutdown: Arc<tokio::sync::Notify>) {
    tokio::spawn(async move {
        use crate::metrics::{compute_mem_percent, compute_tree_cpu_percent, SAMPLE_INTERVAL_MS};
        use cli_companion_platform::process::snapshot;
        use cli_companion_platform::sysinfo::total_phys_bytes;
        use std::time::Instant;

        let pid = std::process::id();
        let mut prev_cpu: Option<u64> = None;
        let mut prev_time: Option<Instant> = None;
        let total_mem = total_phys_bytes();
        let cores = num_cpus();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(SAMPLE_INTERVAL_MS)) => {}
                _ = shutdown.notified() => break,
            }
            if let Ok(snap) = snapshot(pid) {
                let now = Instant::now();
                // CPU
                if let (Some(prev_t), Some(prev_c)) = (prev_time, prev_cpu) {
                    let elapsed = now.duration_since(prev_t);
                    let delta = snap.cpu_time_100ns.saturating_sub(prev_c);
                    *metrics.cpu_percent.lock().unwrap() =
                        compute_tree_cpu_percent(delta, elapsed, cores);
                }
                prev_cpu = Some(snap.cpu_time_100ns);
                prev_time = Some(now);
                // 内存
                metrics
                    .mem_bytes
                    .store(snap.working_set_bytes, Ordering::Relaxed);
                *metrics.mem_percent.lock().unwrap() =
                    compute_mem_percent(snap.working_set_bytes, total_mem);
            }
        }
    });
}

/// 获取逻辑 CPU 核数（Windows 上用 SystemInfo）
fn num_cpus() -> u64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1)
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
                // 管道可达 = 已有健康实例在正常服务：本实例无事可做，静默成功退出
                // （看门狗/自启/GUI 的冗余拉起都走这里，属正常情况而非错误；
                //   用户要求完全屏蔽——降为 debug，info 级日志不再出现）
                if crate::rpc::existing_daemon_alive().await {
                    tracing::debug!("已有 daemon 实例在服务，本实例退出");
                    return Ok(());
                }
                // 管道不可达：旧实例可能正在退出，等待其释放锁后接管
                if attempt == 0 {
                    // 用户要求屏蔽刷屏：该场景仅短暂存在（旧实例毫秒级退场），降为 debug
                    tracing::debug!("检测到旧实例正在退出（锁: {p}），等待其释放后接管…");
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
    let mut app = load_app(&dirs);
    let secrets = load_secrets(&dirs);
    if let Err(e) = services.validate() {
        tracing::warn!("services.json 校验失败（仍已加载）: {e}");
    }
    // v2.6.0：FTP 开机自启逻辑——未勾选自启时，daemon 启动强制关闭 FTP
    // 与受管服务 autostart 行为一致：autostart=false → enabled 被重置为 false
    if !app.ftp.autostart && app.ftp.enabled {
        tracing::info!("FTP 未勾选开机自启，daemon 启动时已停用 FTP（用户可手动启用）");
        app.ftp.enabled = false;
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

    // 6.5 v2.2.0：本机只读状态页（监督任务随 config.changed 即时启停/换端口）
    crate::status_http::spawn_supervisor(state.clone());

    // 6.6 v2.6.0：内置 FTP 服务端（监督任务随 config.changed 即时启停/换端口）
    crate::ftp::spawn_supervisor(state.clone());

    // 6.7 v2.6.0：daemon 自身 CPU/内存指标采样（供 ftp.status 等读取）
    let daemon_metrics = Arc::new(DaemonMetrics::default());
    let _ = DAEMON_METRICS.set(daemon_metrics.clone());
    spawn_daemon_sampler(daemon_metrics, state.shutdown.clone());

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
