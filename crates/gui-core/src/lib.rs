//! GUI 核心层：前端 ↔ daemon 的唯一桥接
//!
//! 前端不直接访问命名管道（WebView 无法访问 Win32 管道），
//! 一切 RPC 经由 [`commands::daemon_rpc`] Tauri 命令转发。

pub mod commands;
pub mod connection;
