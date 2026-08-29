//! Windows 平台能力封装
//!
//! - [`job`]：Job Object（进程树管理与 KILL_ON_JOB_CLOSE）
//! - [`lock`]：单例文件锁
//! - [`dpapi`]：DPAPI 加密（WebDAV 凭据存储）
//! - [`console`]：控制台创建标志映射
//! - [`process`]：进程指标采集（CPU 时间 / 内存工作集）

pub mod console;
pub mod dpapi;
pub mod job;
pub mod lock;
pub mod process;

/// daemon 命名管道名称
pub const PIPE_NAME: &str = r"\\.\pipe\cli-companion-daemon";
