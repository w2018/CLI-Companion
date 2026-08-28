//! Windows 平台能力封装
//!
//! - [`job`]：Job Object（进程树管理与 KILL_ON_JOB_CLOSE）
//! - [`lock`]：单例文件锁
//! - [`dpapi`]：DPAPI 加密（WebDAV 凭据存储）
//! - [`console`]：控制台创建标志映射

pub mod console;
pub mod dpapi;
pub mod job;
pub mod lock;

/// daemon 命名管道名称
pub const PIPE_NAME: &str = r"\\.\pipe\cli-companion-daemon";
