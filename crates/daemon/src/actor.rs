//! per-service actor：每个受管服务一个串行任务（开发文档 §3.3）
//!
//! - 所有 start/stop/restart 命令进入邮箱串行执行，避免并发竞争
//! - Job Object 保证停止时清理整个进程树
//! - 崩溃自动重启：指数退避 + 10 分钟熔断器

use crate::dirs::DataDirs;
use crate::events::{make_event, EventTx};
use chrono::Utc;
use cli_companion_domain::{
    Backoff, RestartPolicy, RuntimeState, ServiceDefinition, ServiceId, ServiceStatus,
};
use cli_companion_platform::console::creation_flags;
use cli_companion_platform::job::Job;
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
}

/// 启动 actor 任务并返回句柄
pub fn spawn_actor(id: ServiceId, dirs: DataDirs, events: Arc<EventTx>) -> ActorHandle {
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
                            // 按策略尝试自动重启（熔断器 + 退避等待均满足才启动）
                            if let Some(def) = self.current_def.clone() {
                                if self.should_auto_restart(&def) && self.wait_backoff(&def).await {
                                    tracing::info!(service = %self.id, "自动重启服务");
                                    match self.start(&def).await {
                                        Ok(()) => self.emit(
                                            EventTopic::ServiceStarted,
                                            serde_json::json!({"auto": true}),
                                        ),
                                        Err(e) => {
                                            tracing::error!(service = %self.id, "重启失败: {e}");
                                            self.emit(
                                                EventTopic::ServiceRestartAttempt,
                                                serde_json::json!({"ok": false, "error": e}),
                                            );
                                        }
                                    }
                                }
                            }
                        }
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
            cmd.env(&e.name, &e.value);
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

    /// 停止：taskkill（优雅）→ 超时 → TerminateJobObject（强杀进程树）
    async fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            self.update(|s| s.status = ServiceStatus::Stopped);
            return Ok(());
        };
        let pid = child.id();
        self.update(|s| s.status = ServiceStatus::Stopping);

        let (graceful_ms, kill_ms) = self
            .current_def
            .as_ref()
            .map(|d| (d.stop.graceful_timeout_ms, d.stop.kill_timeout_ms))
            .unwrap_or((15_000, 10_000));

        // 1. 优雅尝试：taskkill /T 发送关闭消息（控制台程序可能忽略，属正常）
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/pid", &pid.to_string(), "/T"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .output();
        }

        // 2. 等待优雅退出（100ms 快速轮询，尽早发现已退出的进程）
        let deadline = Instant::now() + Duration::from_millis(graceful_ms);
        let mut exited = false;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 3. 强杀进程树
        if !exited {
            tracing::warn!(service = %self.id, "优雅停止超时，强制终止进程树");
            if let Some(job) = &self._job {
                if let Err(e) = job.terminate() {
                    tracing::error!(service = %self.id, "TerminateJobObject 失败: {e}");
                }
            }
            let kill_deadline = Instant::now() + Duration::from_millis(kill_ms);
            while Instant::now() < kill_deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    exited = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !exited {
                tracing::error!(service = %self.id, "force_kill_failed：进程树未能终止");
            }
        }

        let code = child.try_wait().ok().flatten().and_then(|s| s.code());
        self._job = None; // Drop 触发 KILL_ON_JOB_CLOSE 清理残留
        self.update(|s| {
            s.status = ServiceStatus::Stopped;
            s.pid = None;
            s.last_exit_code = code;
        });
        tracing::info!(service = %self.id, "服务已停止");
        Ok(())
    }

    /// 进程退出后的状态记录
    fn on_exit(&mut self, code: i32) {
        self.child = None;
        self._job = None;
        self.update(|s| {
            s.status = ServiceStatus::Failed;
            s.pid = None;
            s.last_exit_code = Some(code);
        });
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
