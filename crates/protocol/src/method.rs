//! JSON-RPC 方法枚举

use serde::{Deserialize, Serialize};

/// 全部 RPC 方法，序列化为 "system.ping" 风格的字符串
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Method {
    // 系统
    #[serde(rename = "system.ping")]
    SystemPing,
    #[serde(rename = "system.info")]
    SystemInfo,

    // 配置
    #[serde(rename = "config.get")]
    ConfigGet,
    #[serde(rename = "config.update")]
    ConfigUpdate,
    #[serde(rename = "config.import")]
    ConfigImport,
    #[serde(rename = "config.export")]
    ConfigExport,

    // 服务
    #[serde(rename = "service.list")]
    ServiceList,
    #[serde(rename = "service.create")]
    ServiceCreate,
    #[serde(rename = "service.update")]
    ServiceUpdate,
    #[serde(rename = "service.delete")]
    ServiceDelete,
    #[serde(rename = "service.start")]
    ServiceStart,
    #[serde(rename = "service.stop")]
    ServiceStop,
    #[serde(rename = "service.restart")]
    ServiceRestart,
    #[serde(rename = "service.logs")]
    ServiceLogs,
    #[serde(rename = "service.logs.clear")]
    ServiceLogsClear,
    /// 全部服务的资源指标（CPU / 内存）
    #[serde(rename = "service.metrics")]
    ServiceMetrics,

    // 守护进程
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown,
    #[serde(rename = "daemon.logs")]
    DaemonLogs,
    #[serde(rename = "daemon.logs.clear")]
    DaemonLogsClear,

    // 同步
    #[serde(rename = "sync.status")]
    SyncStatus,
    #[serde(rename = "sync.run_now")]
    SyncRunNow,
    #[serde(rename = "sync.unlock")]
    SyncUnlock,
    #[serde(rename = "sync.test")]
    SyncTest,

    // 事件
    #[serde(rename = "event.subscribe")]
    EventSubscribe,

    // 备份（v2.2.0）
    #[serde(rename = "backup.list")]
    BackupList,
    #[serde(rename = "backup.restore")]
    BackupRestore,

    // 崩溃诊断（v2.2.0）
    #[serde(rename = "crashreport.list")]
    CrashReportList,
    #[serde(rename = "crashreport.get")]
    CrashReportGet,

    // 应用功能（v2.6.0）
    /// 内置 FTP 服务运行状态
    #[serde(rename = "ftp.status")]
    FtpStatus,
}

impl Method {
    /// 返回协议线上的方法名字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::SystemPing => "system.ping",
            Method::SystemInfo => "system.info",
            Method::ConfigGet => "config.get",
            Method::ConfigUpdate => "config.update",
            Method::ConfigImport => "config.import",
            Method::ConfigExport => "config.export",
            Method::ServiceList => "service.list",
            Method::ServiceCreate => "service.create",
            Method::ServiceUpdate => "service.update",
            Method::ServiceDelete => "service.delete",
            Method::ServiceStart => "service.start",
            Method::ServiceStop => "service.stop",
            Method::ServiceRestart => "service.restart",
            Method::ServiceLogs => "service.logs",
            Method::ServiceLogsClear => "service.logs.clear",
            Method::ServiceMetrics => "service.metrics",
            Method::DaemonShutdown => "daemon.shutdown",
            Method::DaemonLogs => "daemon.logs",
            Method::DaemonLogsClear => "daemon.logs.clear",
            Method::SyncStatus => "sync.status",
            Method::SyncRunNow => "sync.run_now",
            Method::SyncUnlock => "sync.unlock",
            Method::SyncTest => "sync.test",
            Method::EventSubscribe => "event.subscribe",
            Method::BackupList => "backup.list",
            Method::BackupRestore => "backup.restore",
            Method::CrashReportList => "crashreport.list",
            Method::CrashReportGet => "crashreport.get",
            Method::FtpStatus => "ftp.status",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 方法名序列化往返一致() {
        // 每个方法序列化后应得到 "domain.action" 形式，并能反序列化回来
        let all = [
            Method::SystemPing,
            Method::SystemInfo,
            Method::ConfigGet,
            Method::ConfigUpdate,
            Method::ConfigImport,
            Method::ConfigExport,
            Method::ServiceList,
            Method::ServiceCreate,
            Method::ServiceUpdate,
            Method::ServiceDelete,
            Method::ServiceStart,
            Method::ServiceStop,
            Method::ServiceRestart,
            Method::ServiceLogs,
            Method::ServiceLogsClear,
            Method::ServiceMetrics,
            Method::DaemonShutdown,
            Method::DaemonLogs,
            Method::DaemonLogsClear,
            Method::SyncStatus,
            Method::SyncRunNow,
            Method::SyncUnlock,
            Method::SyncTest,
            Method::EventSubscribe,
            Method::BackupList,
            Method::BackupRestore,
            Method::CrashReportList,
            Method::CrashReportGet,
            Method::FtpStatus,
        ];
        for m in all {
            let s = serde_json::to_value(m).unwrap();
            assert_eq!(s, serde_json::Value::String(m.as_str().to_string()));
            let back: Method = serde_json::from_value(s).unwrap();
            assert_eq!(back, m);
        }
    }
}
