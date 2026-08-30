//! per-service actor：每个受管服务一个串行任务（开发文档 §3.3）
//!
//! - 所有 start/stop/restart 命令进入邮箱串行执行，避免并发竞争
//! - Job Object 保证停止时清理整个进程树
//! - 崩溃自动重启：指数退避 + 10 分钟熔断器

use crate::dirs::DataDirs;
use crate::events::{make_event, EventTx};
use crate::metrics::{
    compute_mem_percent, compute_rate_per_sec, compute_tree_cpu_percent, SAMPLE_INTERVAL_MS,
};
use crate::notify;
use chrono::Utc;
use cli_companion_domain::{
    Backoff, RestartPolicy, RuntimeState, ServiceDefinition, ServiceId, ServiceStatus,
};
use cli_companion_platform::console::creation_flags;
use cli_companion_platform::gpu::GpuMonitor;
use cli_companion_platform::job::Job;
use cli_companion_platform::net::NetMonitor;
use cli_companion_platform::process;
use cli_companion_platform::sysinfo;
use cli_companion_protocol::EventTopic;
use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// actor 共享运行时状态
pub type SharedState = Arc<Mutex<RuntimeState>>;

/// 发送给 actor 的命令
pub enum ActorCmd {
    Start {
        def: Box<ServiceDefinition>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
    Restart {
        def: Box<ServiceDefinition>,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// actor 句柄：manager 持有，用于发命令和读状态
#[derive(Clone)]
pub struct ActorHandle {
    pub tx: mpsc::Sender<ActorCmd>,
    pub state: SharedState,
}

/// 单个服务日志轮转上限：10 MB
const LOG_ROTATE_SIZE: u64 = 10 * 1024 * 1024;

/// actor 主结构
struct Actor {
    id: ServiceId,
    dirs: DataDirs,
    state: SharedState,
    rx: mpsc::Receiver<ActorCmd>,
    child: Option<Child>,
    _job: Option<Job>,
    /// 当前运行使用的定义（健康检查间隔、停止超时取自此）
    current_def: Option<ServiceDefinition>,
    /// 10 分钟窗口内的重启时间戳（熔断器）
    restart_times: Vec<Instant>,
    /// 事件总线（崩溃/自动重启事件）
    events: Arc<EventTx>,
    /// daemon 是否以 Win32 服务运行（session 0 收不到 Toast）
    as_service: bool,
    /// 进程树 CPU 累计时间基线（pid → 100ns；树聚合差分用）
    last_cpu_tree: HashMap<u32, u64>,
    /// 上次采样时刻
    last_sample_at: Option<Instant>,
    /// 磁盘 I/O 累计基线（读, 写 字节）
    last_io: Option<(u64, u64)>,
    /// TCP 流量采集器（按连接启用统计并维护差分基线）
    net: NetMonitor,
    /// GPU 监控器（PDH 查询按服务持有）
    gpu: GpuMonitor,
    /// 上次内存告警时刻（v2.2.0，10 分钟冷却）
    last_mem_alert: Option<Instant>,
    /// 命令探活连续不健康次数（v2.2.0，达到 failure_threshold 终止进程走自愈）
    cmd_unhealthy_streak: u32,
}

/// 启动 actor 任务并返回句柄
pub fn spawn_actor(
    id: ServiceId,
    dirs: DataDirs,
    events: Arc<EventTx>,
    as_service: bool,
) -> ActorHandle {
    let state = Arc::new(Mutex::new(RuntimeState::default()));
    let (tx, rx) = mpsc::channel(16);
    let actor = Actor {
        id,
        dirs,
        state: state.clone(),
        rx,
        child: None,
        _job: None,
        current_def: None,
        restart_times: Vec::new(),
        events,
        as_service,
        last_cpu_tree: HashMap::new(),
        last_sample_at: None,
        last_io: None,
        net: NetMonitor::default(),
        gpu: GpuMonitor::default(),
        last_mem_alert: None,
        cmd_unhealthy_streak: 0,
    };
    tokio::spawn(actor.run());
    ActorHandle { tx, state }
}

impl Actor {
    /// 更新运行时状态
    fn update(&self, f: impl FnOnce(&mut RuntimeState)) {
        if let Ok(mut st) = self.state.lock() {
            f(&mut st);
        }
    }

    /// 广播事件（payload 扁平携带服务名便于前端展示）
    fn emit(&self, topic: EventTopic, mut payload: serde_json::Value) {
        let name = self
            .current_def
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_default();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("name".into(), serde_json::Value::String(name));
        }
        let _ = self
            .events
            .send(make_event(topic, Some(self.id.to_string()), payload));
    }

    async fn run(mut self) {
        loop {
            if self.child.is_some() {
                // ===== 运行中：健康检查 + 命令处理 =====
                let interval = self
                    .current_def
                    .as_ref()
                    .map(|d| d.health.interval_ms)
                    .unwrap_or(5_000);
                tokio::select! {
                    cmd = self.rx.recv() => {
                        match cmd {
                            Some(ActorCmd::Stop { reply }) => {
                                let r = self.stop().await;
                                let _ = reply.send(r);
                            }
                            Some(ActorCmd::Restart { def, reply }) => {
                                let _ = self.stop().await;
                                let r = self.start(&def).await;
                                let _ = reply.send(r);
                            }
                            Some(ActorCmd::Start { reply, .. }) => {
                                let _ = reply.send(Err("服务已在运行中".into()));
                            }
                            None => break, // 所有句柄关闭，actor 退出
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(interval)) => {
                        let exited = self
                            .child
                            .as_mut()
                            .and_then(|c| c.try_wait().ok().flatten());
                        if let Some(status) = exited {
                            let code = status.code().unwrap_or(-1);
                            tracing::warn!(service = %self.id, "服务意外退出，退出码 {code}");
                            self.on_exit(code);
                            self.emit(
                                EventTopic::ServiceHealth,
                                serde_json::json!({"exit_code": code, "status": "failed"}),
                            );
                            notify::notify_service_failure(
                                &self.dirs,
                                self.as_service,
                                "CLI 服务已崩溃",
                                &format!("{}：进程意外退出（退出码 {code}）", self.def_name()),
                            );
                            // v2.2.0：归档崩溃诊断（脱敏，失败只记日志）
                            self.write_crash_report(code);
                            // 按策略尝试自动重启（熔断器 + 退避等待均满足才启动）
                            if let Some(def) = self.current_def.clone() {
                                if self.should_auto_restart(&def) && self.wait_backoff(&def).await {
                                    tracing::info!(service = %self.id, "自动重启服务");
                                    match self.start(&def).await {
                                        Ok(()) => {
                                            self.emit(
                                                EventTopic::ServiceStarted,
                                                serde_json::json!({"auto": true}),
                                            );
                                            notify::notify_service_failure(
                                                &self.dirs,
                                                self.as_service,
                                                "服务已自动重启",
                                                &format!("{}：崩溃后已自动恢复运行", self.def_name()),
                                            );
                                        }
                                        Err(e) => {
                                            tracing::error!(service = %self.id, "重启失败: {e}");
                                            self.emit(
                                                EventTopic::ServiceRestartAttempt,
                                                serde_json::json!({"ok": false, "error": e}),
                                            );
                                            notify::notify_service_failure(
                                                &self.dirs,
                                                self.as_service,
                                                "服务自动重启失败",
                                                &format!("{}：{e}", self.def_name()),
                                            );
                                        }
                                    }
                                }
                            }
                        } else if self.child.is_some() {
                            // v2.2.0：进程仍在运行 → 命令探活（仅 HealthKind::Command）
                            self.check_command_health().await;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(SAMPLE_INTERVAL_MS)) => {
                        // 资源指标采样：失败静默（进程刚退出属正常）
                        self.sample_metrics();
                    }
                }
            } else {
                // ===== 已停止：等待命令 =====
                match self.rx.recv().await {
                    Some(ActorCmd::Start { def, reply }) => {
                        let r = self.start(&def).await;
                        let _ = reply.send(r);
                    }
                    Some(ActorCmd::Stop { reply }) => {
                        let _ = reply.send(Ok(()));
                    }
                    Some(ActorCmd::Restart { def, reply }) => {
                        let r = self.start(&def).await;
                        let _ = reply.send(r);
                    }
                    None => break,
                }
            }
        }
        // actor 退出时兜底停止残留子进程
        let _ = self.stop().await;
    }

    /// 启动服务进程
    async fn start(&mut self, def: &ServiceDefinition) -> Result<(), String> {
        // 1. exe 存在性校验
        if !def.exe.is_file() {
            self.update(|s| s.status = ServiceStatus::Failed);
            return Err(format!("exe 不存在: {}", def.exe.display()));
        }
        // 2. 工作目录存在性校验
        if let Some(wd) = &def.working_dir {
            if !wd.is_dir() {
                self.update(|s| s.status = ServiceStatus::Failed);
                return Err(format!("工作目录不存在: {}", wd.display()));
            }
        }
        self.update(|s| s.status = ServiceStatus::Starting);

        // 3. 构建 CreateProcess 命令（禁止 cmd /c 拼接，argv 语义由 Rust Command 保证）
        let mut cmd = Command::new(&def.exe);
        cmd.args(def.render_args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(wd) = &def.working_dir {
            cmd.current_dir(wd);
        }
        for e in &def.env {
            // v2.2.0：机密占位符 → 从加密存储解密注入真实值
            let value = if e.secret && e.value == crate::secrets_env::ENCRYPTED_PLACEHOLDER {
                crate::secrets_env::load_secret(&self.dirs, &self.id, &e.name).unwrap_or_default()
            } else {
                e.value.clone()
            };
            cmd.env(&e.name, &value);
        }
        let flags = creation_flags(&def.console);
        #[cfg(windows)]
        cmd.creation_flags(flags.creation_flags);

        // 4. 派生进程
        let mut child = cmd.spawn().map_err(|e| {
            self.update(|s| s.status = ServiceStatus::Failed);
            format!("启动进程失败: {e}")
        })?;
        let pid = child.id();

        // 5. 创建 Job 并关联（KILL_ON_JOB_CLOSE 保证 daemon 崩溃后清理子进程树）
        let job = match Job::create().and_then(|j| {
            j.assign(&child)?;
            Ok(j)
        }) {
            Ok(j) => j,
            Err(e) => {
                let _ = child.kill();
                self.update(|s| s.status = ServiceStatus::Failed);
                return Err(format!("关联 Job Object 失败: {e}"));
            }
        };

        // 6. stdout/stderr → 每服务日志文件
        let log_path = self.dirs.service_log(&self.id.to_string());
        if let Some(out) = child.stdout.take() {
            spawn_log_thread(out, log_path.clone(), "stdout");
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_thread(err, log_path, "stderr");
        }

        // 7. 更新状态
        self.child = Some(child);
        self._job = Some(job);
        self.current_def = Some(def.clone());
        self.cmd_unhealthy_streak = 0;
        let now = Utc::now();
        self.update(|s| {
            s.status = ServiceStatus::Running;
            s.pid = Some(pid);
            s.started_at = Some(now);
            s.last_exit_code = None;
        });
        tracing::info!(service = %self.id, pid, "服务已启动");
        Ok(())
    }

    /// 停止：直接 TerminateJobObject 强杀进程树（不做优雅等待）
    ///
    /// 控制台类 CLI 服务普遍不响应 taskkill 的关闭消息，等待宽限期只会拖慢
    /// 停止与退出流程 —— 按产品决策直接批量强杀，Job Object 保证不留孤儿进程。
    async fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            self.update(|s| s.status = ServiceStatus::Stopped);
            return Ok(());
        };
        self.update(|s| s.status = ServiceStatus::Stopping);

        let kill_ms = self
            .current_def
            .as_ref()
            .map(|d| d.stop.kill_timeout_ms)
            .unwrap_or(10_000);

        // 强杀进程树（job 覆盖子进程及其全部后代）
        if let Some(job) = &self._job {
            if let Err(e) = job.terminate() {
                tracing::error!(service = %self.id, "TerminateJobObject 失败: {e}");
            }
        }

        // 等待退出确认（100ms 快速轮询；TerminateJobObject 后通常毫秒级完成）
        let deadline = Instant::now() + Duration::from_millis(kill_ms);
        let mut exited = false;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !exited {
            tracing::error!(service = %self.id, "force_kill_failed：进程树未能终止");
        }

        let code = child.try_wait().ok().flatten().and_then(|s| s.code());
        self._job = None; // Drop 触发 KILL_ON_JOB_CLOSE 清理残留
        self.clear_metrics();
        self.update(|s| {
            s.status = ServiceStatus::Stopped;
            s.pid = None;
            s.last_exit_code = code;
            s.cpu_percent = None;
            s.mem_bytes = None;
            s.mem_percent = None;
            s.gpu_percent = None;
            s.gpu_mem_bytes = None;
            s.disk_read_bytes_per_sec = None;
            s.disk_write_bytes_per_sec = None;
            s.net_rx_bytes_per_sec = None;
            s.net_tx_bytes_per_sec = None;
        });
        tracing::info!(service = %self.id, "服务已停止");
        Ok(())
    }

    /// 进程退出后的状态记录
    fn on_exit(&mut self, code: i32) {
        self.child = None;
        self._job = None;
        self.clear_metrics();
        self.update(|s| {
            s.status = ServiceStatus::Failed;
            s.pid = None;
            s.last_exit_code = Some(code);
            s.cpu_percent = None;
            s.mem_bytes = None;
            s.mem_percent = None;
            s.gpu_percent = None;
            s.gpu_mem_bytes = None;
            s.disk_read_bytes_per_sec = None;
            s.disk_write_bytes_per_sec = None;
            s.net_rx_bytes_per_sec = None;
            s.net_tx_bytes_per_sec = None;
        });
    }

    /// 采样 CPU / 内存 / GPU / 磁盘 / 网络 并写入运行时状态（进程刚退出时静默跳过）
    fn sample_metrics(&mut self) {
        let Some(child) = &self.child else { return };
        let pid = child.id();
        // CPU/内存/磁盘/网络全部聚合 Job 进程树：包装脚本、主-多进程型服务的
        // 根进程常年空闲，仅采根进程会恒显 0（v2.4.0 修复 CPU 恒 0.0%）
        let pids = self.job_tree_pids(pid);
        let now = Instant::now();

        // CPU：按 PID 逐个差分（新出现的子进程本窗口计 0，避免携带历史累计值造成毛刺）
        let mut cpu_tree: HashMap<u32, u64> = HashMap::new();
        for p in &pids {
            if let Ok(s) = process::snapshot(*p) {
                cpu_tree.insert(*p, s.cpu_time_100ns);
            }
        }
        let mut cpu_delta_100ns = 0u64;
        for (p, now_t) in &cpu_tree {
            if let Some(prev_t) = self.last_cpu_tree.get(p) {
                if now_t > prev_t {
                    cpu_delta_100ns += now_t - prev_t;
                }
            }
        }
        let mem = self.job_tree_mem_bytes(&pids);
        let io = self.job_tree_io_bytes(&pids);
        // 网络：采集器内部按连接启用统计并返回本窗口收发增量
        let net_delta = self.net.sample(&pids);
        let gpu = self.gpu.sample(&pids);

        let elapsed = self.last_sample_at.map(|t| now - t);
        let cores = std::thread::available_parallelism()
            .map(|n| n.get() as u64)
            .unwrap_or(1);
        let cpu_pct = elapsed.and_then(|el| compute_tree_cpu_percent(cpu_delta_100ns, el, cores));
        // 速率类指标：差分 / 间隔；首次采样或本次无数据时为 None（沿用上次值）
        let (disk_read, disk_write) = match (elapsed, self.last_io, io) {
            (Some(el), Some((pr, pw)), Some((r, w))) => (
                compute_rate_per_sec(pr, r, el),
                compute_rate_per_sec(pw, w, el),
            ),
            _ => (None, None),
        };
        // 网络速率：窗口增量 / 间隔；间隔异常过短时为 None（沿用上次值）
        let (net_rx, net_tx) = match elapsed {
            Some(el) => (
                compute_rate_per_sec(0, net_delta.in_bytes, el),
                compute_rate_per_sec(0, net_delta.out_bytes, el),
            ),
            None => (None, None),
        };
        // 内存占系统物理内存百分比
        let mem_pct = mem.and_then(|b| compute_mem_percent(b, sysinfo::total_phys_bytes()));

        self.last_sample_at = Some(now);
        self.last_cpu_tree = cpu_tree;
        self.last_io = io;

        self.update(|s| {
            // CPU% 无基线时保持上次值，避免界面跳动（速率类指标同策略）
            if let Some(p) = cpu_pct {
                s.cpu_percent = Some(p);
            }
            s.mem_bytes = mem;
            if let Some(p) = mem_pct {
                s.mem_percent = Some(p);
            }
            if let Some(r) = disk_read {
                s.disk_read_bytes_per_sec = Some(r);
            }
            if let Some(w) = disk_write {
                s.disk_write_bytes_per_sec = Some(w);
            }
            if let Some(r) = net_rx {
                s.net_rx_bytes_per_sec = Some(r);
            }
            if let Some(t) = net_tx {
                s.net_tx_bytes_per_sec = Some(t);
            }
            if let Some(g) = gpu {
                s.gpu_percent = Some(g.percent);
                s.gpu_mem_bytes = Some(g.mem_bytes);
            }
        });
        // v2.2.0：内存告警（阈值 + 10 分钟冷却，走既有通知通道）
        if let Some(def) = &self.current_def {
            if crate::metrics::mem_alert_triggered(def.mem_alert_mb, mem, self.last_mem_alert) {
                self.last_mem_alert = Some(now);
                let mb = mem.unwrap_or(0) / 1024 / 1024;
                let limit = def.mem_alert_mb.unwrap_or(0);
                self.emit(
                    EventTopic::ServiceHealth,
                    serde_json::json!({"mem_alert": true, "mem_bytes": mem}),
                );
                notify::notify_service_failure(
                    &self.dirs,
                    self.as_service,
                    "内存告警",
                    &format!(
                        "{}：内存 {} MB 已超过阈值 {} MB",
                        self.def_name(),
                        mb,
                        limit
                    ),
                );
            }
        }
    }

    /// Job 进程树 PID 列表；Job 查询失败时退化为仅根进程
    fn job_tree_pids(&self, root_pid: u32) -> Vec<u32> {
        match &self._job {
            Some(job) => job.process_ids().unwrap_or_else(|_| vec![root_pid]),
            None => vec![root_pid],
        }
    }

    /// Job 进程树的内存工作集之和
    fn job_tree_mem_bytes(&self, pids: &[u32]) -> Option<u64> {
        let mut total = 0u64;
        let mut any = false;
        for p in pids {
            if let Ok(snap) = process::snapshot(*p) {
                total += snap.working_set_bytes;
                any = true;
            }
        }
        any.then_some(total)
    }

    /// Job 进程树的磁盘 I/O 累计（读, 写）；全部 PID 读取失败时返回 None
    fn job_tree_io_bytes(&self, pids: &[u32]) -> Option<(u64, u64)> {
        let mut read = 0u64;
        let mut write = 0u64;
        let mut any = false;
        for p in pids {
            if let Ok(snap) = process::io_snapshot(*p) {
                read = read.saturating_add(snap.read_bytes);
                write = write.saturating_add(snap.write_bytes);
                any = true;
            }
        }
        any.then_some((read, write))
    }

    /// 清空采样基线（进程退出后基线失效）
    fn clear_metrics(&mut self) {
        self.last_cpu_tree.clear();
        self.last_sample_at = None;
        self.last_io = None;
        self.net.reset();
    }

    /// 当前定义的服务名（用于通知文案；无定义时回退为 ID）
    fn def_name(&self) -> String {
        self.current_def
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| self.id.to_string())
    }

    /// 崩溃诊断归档（v2.2.0）
    ///
    /// 脱敏原则：定义快照只含名称/exe/参数/目录等，环境变量仅记名称与机密
    /// 标记、绝不落值；写入失败不影响崩溃自动重启主流程。
    fn write_crash_report(&self, code: i32) {
        let Some(def) = &self.current_def else { return };
        let def_json = serde_json::json!({
            "name": def.name,
            "exe": def.exe.display().to_string(),
            "args": def.args,
            "working_dir": def.working_dir.as_ref().map(|p| p.display().to_string()),
            "console": def.console,
            "restart_policy": def.restart.policy,
            "env_names": def
                .env
                .iter()
                .map(|e| serde_json::json!({"name": e.name, "secret": e.secret}))
                .collect::<Vec<_>>(),
        });
        let log_tail = crate::crashreport::tail_of_file(
            &self.dirs.service_log(&self.id.to_string()),
            crate::crashreport::LOG_TAIL_LINES,
        );
        if let Err(e) = crate::crashreport::write_report(
            &self.dirs,
            &self.id.to_string(),
            &def.name,
            code,
            &Utc::now().to_rfc3339(),
            def_json,
            &log_tail,
        ) {
            tracing::warn!(service = %self.id, "崩溃诊断归档失败: {e}");
        }
    }

    /// 命令探活（v2.2.0，仅 HealthKind::Command；其他 kind 保持原快路径不变）
    ///
    /// 连续不健康达到 failure_threshold 时终止进程，复用既有"崩溃 → 通知 →
    /// 自动重启（退避 + 熔断）"路径，不另起重启逻辑。
    async fn check_command_health(&mut self) {
        let Some(def) = self.current_def.clone() else {
            return;
        };
        let cli_companion_domain::HealthKind::Command { program, args } = &def.health.kind else {
            return;
        };
        match crate::health::check_command(program, args).await {
            crate::health::CheckOutcome::Healthy => {
                self.cmd_unhealthy_streak = 0;
                self.update(|s| s.last_health = Some("cmd-ok".into()));
            }
            crate::health::CheckOutcome::Unhealthy(msg) => {
                self.cmd_unhealthy_streak += 1;
                let streak = self.cmd_unhealthy_streak;
                self.update(|s| s.last_health = Some(format!("cmd-unhealthy({msg})")));
                self.emit(
                    EventTopic::ServiceHealth,
                    serde_json::json!({"health": "unhealthy", "message": msg, "streak": streak}),
                );
                if streak >= def.health.failure_threshold.max(1) {
                    tracing::warn!(
                        service = %self.id,
                        "命令探活连续 {streak} 次不健康，终止进程走自愈"
                    );
                    self.cmd_unhealthy_streak = 0;
                    if let Some(job) = &self._job {
                        let _ = job.terminate();
                    }
                }
            }
        }
    }

    /// 熔断器 + 策略判断：是否允许自动重启
    fn should_auto_restart(&mut self, def: &ServiceDefinition) -> bool {
        if def.restart.policy == RestartPolicy::Never {
            return false;
        }
        // 清理 10 分钟窗口外的时间戳
        let cutoff = Instant::now() - Duration::from_secs(600);
        self.restart_times.retain(|t| *t > cutoff);
        if self.restart_times.len() >= def.restart.max_attempts_10m as usize {
            tracing::error!(service = %self.id, "10 分钟内重启超过 {} 次，触发熔断", def.restart.max_attempts_10m);
            self.update(|s| s.last_health = Some("circuit-breaker".into()));
            self.emit(
                EventTopic::ServiceRestartAttempt,
                serde_json::json!({"ok": false, "circuit_breaker": true}),
            );
            notify::notify_service_failure(
                &self.dirs,
                self.as_service,
                "服务已熔断",
                &format!(
                    "{}：10 分钟内重启超过 {} 次，已暂停自动重启",
                    self.def_name(),
                    def.restart.max_attempts_10m
                ),
            );
            return false;
        }
        true
    }

    /// 退避等待（可被 Stop 命令打断）
    async fn wait_backoff(&mut self, def: &ServiceDefinition) -> bool {
        let Backoff {
            initial_ms,
            max_ms,
            multiplier,
        } = def.restart.backoff;
        let n = self.restart_times.len() as u32;
        let delay = initial_ms
            .saturating_mul((multiplier as u64).saturating_pow(n.min(10)))
            .min(max_ms);
        self.restart_times.push(Instant::now());
        let recent = self.restart_times.len() as u32;
        self.update(|s| {
            s.restarts_recent_10m = recent;
            s.restart_count += 1;
            s.status = ServiceStatus::Failed;
        });
        tracing::info!(service = %self.id, "退避 {delay}ms 后重试");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(delay)) => true,
            cmd = self.rx.recv() => {
                // 退避期间收到 Stop：取消重启
                if let Some(ActorCmd::Stop { reply }) = cmd {
                    let _ = reply.send(Ok(()));
                }
                false
            }
        }
    }
}

/// 把子进程输出线程写入日志文件（轮转：>10MB 时更名为 .old）
fn spawn_log_thread(
    stream: impl std::io::Read + Send + 'static,
    log_path: PathBuf,
    label: &'static str,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        let reader = BufReader::new(stream);
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(meta) = std::fs::metadata(&log_path) {
                if meta.len() > LOG_ROTATE_SIZE {
                    let old = log_path.with_extension("old");
                    let _ = std::fs::remove_file(&old);
                    let _ = std::fs::rename(&log_path, &old);
                }
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let _ = writeln!(f, "[{label}] {line}");
            }
        }
    });
}

/// 等待进程退出的轮询工具（供集成测试复用）
#[allow(dead_code)] // 供集成测试使用
pub async fn wait_until_stopped(state: &SharedState, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(st) = state.lock() {
            if st.status == ServiceStatus::Stopped {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// 供 manager 查询的便捷类型别名
pub type ActorMap = HashMap<ServiceId, ActorHandle>;
