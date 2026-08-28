//! CLI Companion 领域模型
//!
//! - [`service`]：ServiceDefinition 及其子结构（参数、环境、控制台、健康检查、重启策略）
//! - [`config`]：services.json / app.json 顶层配置
//! - [`runtime`]：运行时状态（非持久化事实，daemon 内部使用）
//! - [`migration`]：配置版本迁移
//! - [`path`]：路径可移植性策略

pub mod config;
pub mod migration;
pub mod path;
pub mod runtime;
pub mod service;

pub use config::{ConfigError, ServicesConfig};
pub use runtime::{RuntimeState, ServiceStatus};
pub use service::{
    Arg, ArgKind, Backoff, ConsoleConfig, ConsoleMode, EnvVar, HealthConfig, HealthKind,
    RestartConfig, RestartPolicy, RunAs, ServiceDefinition, ServiceId, StopConfig, WindowStartup,
};

/// 当前配置 schema 版本
pub const SCHEMA_VERSION: u32 = 1;
