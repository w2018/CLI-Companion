//! 服务定义（开发文档 §5.1）

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// 服务唯一 ID
pub type ServiceId = Uuid;

/// 单个受管 CLI 服务的完整定义
///
/// 字段与开发文档 §5.1 的 services.v1 schema 一一对应。
/// 未知字段默认拒绝（deny_unknown_fields），保证配置前向兼容由 version 迁移显式处理。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ServiceDefinition {
    pub id: ServiceId,
    pub name: String,
    /// 仅供 GUI 展示的说明
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub autostart: bool,
    /// 可执行文件路径（支持相对/环境变量展开，见 path 模块）
    pub exe: PathBuf,
    /// 有序参数数组，保留 CLI 语义
    #[serde(default)]
    pub args: Vec<Arg>,
    /// 多值参数分隔符，默认空格
    #[serde(default = "default_delimiter")]
    pub argument_delimiter: String,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub run_as: RunAs,
    #[serde(default)]
    pub console: ConsoleConfig,
    #[serde(default)]
    pub stop: StopConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub restart: RestartConfig,
    #[serde(default)]
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

fn default_delimiter() -> String {
    " ".to_string()
}

/// 单个命令行参数
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Arg {
    pub id: String,
    /// 参数键，如 "--port"
    pub key: String,
    /// 参数值；flag 类型为 None
    #[serde(default)]
    pub value: Option<String>,
    /// false 时跳过渲染，但保留在定义中
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub kind: ArgKind,
    /// GUI 展示用说明
    #[serde(default)]
    pub description: String,
}

/// 参数种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    /// 带 key + value 的选项
    Option,
    /// 只有 key 的开关
    Flag,
    /// 无 key 的位置参数
    Positional,
}

/// 环境变量
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
    /// 标记为机密的变量不同步、不写日志
    #[serde(default)]
    pub secret: bool,
}

/// 运行身份（V1 仅支持当前用户）；序列化为 {"kind":"current_user"}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunAs {
    CurrentUser,
}

impl Default for RunAs {
    fn default() -> Self {
        RunAs::CurrentUser
    }
}

/// 控制台配置：窗口语义由 mode + startup 组合表达（开发文档 §1.3）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ConsoleConfig {
    pub mode: ConsoleMode,
    pub startup: WindowStartup,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self { mode: ConsoleMode::NewConsoleVisible, startup: WindowStartup::Normal }
    }
}

/// 控制台模式（映射 CreateProcess 标志，见 daemon 进程模块）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleMode {
    /// CREATE_NEW_CONSOLE + 可见
    NewConsoleVisible,
    /// CREATE_NEW_CONSOLE + 隐藏
    NewConsoleHidden,
    /// DETACHED_PROCESS
    NoConsole,
}

/// 窗口初始显示状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStartup {
    Normal,
    Minimized,
    Hidden,
}

/// 停止策略
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct StopConfig {
    pub signal: StopSignal,
    pub graceful_timeout_ms: u64,
    pub kill_timeout_ms: u64,
}

impl Default for StopConfig {
    fn default() -> Self {
        // 优雅停止默认 1.5 秒：Windows 上 taskkill（WM_CLOSE）对控制台程序
        // 基本无效，久等无意义；超时后立即 Job Object 强杀进程树，
        // 保证"停止很快"。需要更长优雅期的服务可单独配置。
        Self { signal: StopSignal::CtrlC, graceful_timeout_ms: 1_500, kill_timeout_ms: 10_000 }
    }
}

/// 停止信号
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSignal {
    CtrlC,
}

/// 健康检查配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct HealthConfig {
    pub kind: HealthKind,
    pub interval_ms: u64,
    pub failure_threshold: u32,
    pub success_threshold: u32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            kind: HealthKind::Process,
            interval_ms: 5_000,
            failure_threshold: 3,
            success_threshold: 1,
        }
    }
}

/// 健康检查种类
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthKind {
    /// 进程存在 + 启动延迟（默认）
    Process,
    /// TCP 端口探活
    Tcp { host: String, port: u16 },
    /// HTTP /health 探活
    Http { url: String },
}

/// 重启策略
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RestartConfig {
    pub policy: RestartPolicy,
    /// 10 分钟窗口内最大重启次数（熔断器）
    pub max_attempts_10m: u32,
    pub backoff: Backoff,
}

impl Default for RestartConfig {
    fn default() -> Self {
        Self {
            policy: RestartPolicy::OnFailure,
            max_attempts_10m: 10,
            backoff: Backoff::default(),
        }
    }
}

/// 重启策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// daemon 启动时总是尝试启动
    Always,
    /// 仅当上次退出非 clean 时启动
    OnFailure,
    Never,
}

/// 指数退避参数
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Backoff {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub multiplier: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { initial_ms: 2_000, max_ms: 300_000, multiplier: 2 }
    }
}

impl ServiceDefinition {
    /// 创建新服务定义（自动生成 ID 与时间戳）
    pub fn new(name: impl Into<String>, exe: PathBuf) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: String::new(),
            enabled: true,
            autostart: false,
            exe,
            args: Vec::new(),
            argument_delimiter: default_delimiter(),
            working_dir: None,
            env: Vec::new(),
            run_as: RunAs::default(),
            console: ConsoleConfig::default(),
            stop: StopConfig::default(),
            health: HealthConfig::default(),
            restart: RestartConfig::default(),
            labels: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// 渲染最终命令行参数列表：跳过 enabled=false 的项
    ///
    /// 仅返回传给 Command::args 的参数值；key/value 的拼接与转义由
    /// Windows 层的 argv 语义处理，此处不做字符串拼接。
    pub fn render_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        for arg in &self.args {
            if !arg.enabled {
                continue;
            }
            match arg.kind {
                ArgKind::Flag | ArgKind::Option => out.push(arg.key.clone()),
                ArgKind::Positional => {}
            }
            if arg.kind == ArgKind::Option {
                if let Some(v) = &arg.value {
                    out.push(v.clone());
                }
            }
            if arg.kind == ArgKind::Positional {
                if let Some(v) = &arg.value {
                    out.push(v.clone());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 渲染参数跳过禁用项() {
        let mut svc = ServiceDefinition::new("测试", r"C:\Tools\agent.exe".into());
        svc.args = vec![
            Arg {
                id: "a1".into(),
                key: "--bind".into(),
                value: Some("127.0.0.1:7000".into()),
                enabled: true,
                kind: ArgKind::Option,
                description: String::new(),
            },
            Arg {
                id: "a2".into(),
                key: "--verbose".into(),
                value: None,
                enabled: false,
                kind: ArgKind::Flag,
                description: String::new(),
            },
            Arg {
                id: "a3".into(),
                key: String::new(),
                value: Some("serve".into()),
                enabled: true,
                kind: ArgKind::Positional,
                description: String::new(),
            },
        ];
        assert_eq!(svc.render_args(), vec!["--bind", "127.0.0.1:7000", "serve"]);
    }

    #[test]
    fn 服务定义序列化往返() {
        let svc = ServiceDefinition::new("本地代理", r"C:\Tools\agent.exe".into());
        let json = serde_json::to_string(&svc).unwrap();
        let back: ServiceDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, svc);
    }
}
