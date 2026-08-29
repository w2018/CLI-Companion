//! CLI Companion 守护进程库（供 bin 入口与集成测试使用）

pub mod actor;
pub mod app_config;
pub mod backup;
pub mod crashreport;
pub mod dirs;
pub mod events;
pub mod health;
pub mod manager;
pub mod metrics;
pub mod notify;
pub mod rpc;
pub mod secrets_env;
pub mod service;
pub mod state;
pub mod sync;
