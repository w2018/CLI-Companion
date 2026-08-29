//! 服务崩溃诊断报告（v2.2.0）
//!
//! 服务意外退出时归档"事故现场"到 `data/crashreports/<服务>-<时间戳>/`：
//! - `info.json`：退出码、时间、重启计数、脱敏后的服务定义（不含任何环境变量值）
//! - `log-tail.txt`：服务日志最后 100 行
//! 只增不改：写入失败仅记日志，绝不影响崩溃自动重启主流程。

use crate::dirs::DataDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 日志尾部行数
pub const LOG_TAIL_LINES: usize = 100;

/// 单份报告的元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashReportMeta {
    /// 目录名（<服务名>-<时间戳>）
    pub name: String,
    /// 崩溃时间（RFC 3339）
    pub ts: String,
    /// 服务名
    pub service: String,
    /// 退出码
    pub exit_code: i32,
}

/// 报告完整内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashReport {
    pub info: serde_json::Value,
    pub log_tail: String,
}

/// 写入一份崩溃报告；返回目录名
///
/// `def_json`：调用方提供的脱敏服务定义（不得包含机密值，由调用方负责）。
pub fn write_report(
    dirs: &DataDirs,
    service_id: &str,
    service_name: &str,
    exit_code: i32,
    ts_rfc3339: &str,
    def_json: serde_json::Value,
    log_tail: &str,
) -> std::io::Result<String> {
    let ts_compact = ts_rfc3339.replace([':', '-', '.'], "");
    let dir_name = format!(
        "{}-{}",
        sanitize(service_name),
        &ts_compact[..20.min(ts_compact.len())]
    );
    let dir = dirs.crash_reports().join(&dir_name);
    std::fs::create_dir_all(&dir)?;
    let info = serde_json::json!({
        "service_id": service_id,
        "service": service_name,
        "exit_code": exit_code,
        "ts": ts_rfc3339,
        "definition": def_json,
    });
    std::fs::write(dir.join("info.json"), serde_json::to_string_pretty(&info)?)?;
    std::fs::write(dir.join("log-tail.txt"), log_tail)?;
    Ok(dir_name)
}

/// 列出全部报告（最新在前）
pub fn list(dirs: &DataDirs) -> Vec<CrashReportMeta> {
    let Ok(entries) = std::fs::read_dir(dirs.crash_reports()) else {
        return Vec::new();
    };
    let mut metas: Vec<CrashReportMeta> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let info: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path().join("info.json")).ok()?)
                    .ok()?;
            Some(CrashReportMeta {
                service: info.get("service")?.as_str()?.to_string(),
                exit_code: info.get("exit_code")?.as_i64()? as i32,
                ts: info.get("ts")?.as_str()?.to_string(),
                name,
            })
        })
        .collect();
    metas.sort_by(|a, b| b.name.cmp(&a.name));
    metas
}

/// 读取单份报告；name 仅接受安全目录名（防路径穿越）
pub fn get(dirs: &DataDirs, name: &str) -> Result<CrashReport, String> {
    if name.contains('\\') || name.contains('/') || name.contains("..") || name.is_empty() {
        return Err(format!("非法报告名: {name}"));
    }
    let dir = dirs.crash_reports().join(name);
    let info_raw = std::fs::read_to_string(dir.join("info.json"))
        .map_err(|e| format!("读取诊断报告失败: {e}"))?;
    let info: serde_json::Value =
        serde_json::from_str(&info_raw).map_err(|e| format!("诊断报告损坏: {e}"))?;
    let log_tail = std::fs::read_to_string(dir.join("log-tail.txt")).unwrap_or_default();
    Ok(CrashReport { info, log_tail })
}

/// 读文件最后 n 行（无文件返回空串）
pub fn tail_of_file(path: &PathBuf, n: usize) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// 服务名 → 安全目录名（替换 Windows 非法字符）
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dirs() -> DataDirs {
        DataDirs::resolve(Some(std::env::temp_dir().join(format!(
            "cc-crash-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))))
    }

    #[test]
    fn 写入列出读取往返() {
        let dirs = test_dirs();
        let name = write_report(
            &dirs,
            "svc-1",
            "本地代理",
            -1,
            "2026-08-29T12:00:00.123Z",
            serde_json::json!({"name": "本地代理", "exe": "a.exe"}),
            "line1\nline2",
        )
        .unwrap();
        let all = list(&dirs);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].service, "本地代理");
        assert_eq!(all[0].exit_code, -1);
        let rep = get(&dirs, &name).unwrap();
        assert_eq!(rep.info["exit_code"], -1);
        assert!(rep.log_tail.contains("line2"));
    }

    #[test]
    fn 非法名与空目录安全() {
        let dirs = test_dirs();
        assert!(list(&dirs).is_empty());
        assert!(get(&dirs, "../evil").is_err());
        assert!(get(&dirs, "a\\b").is_err());
    }

    #[test]
    fn 服务名非法字符被替换() {
        assert_eq!(sanitize("a/b\\c:d*e?f\"g<h>i|j"), "a-b-c-d-e-f-g-h-i-j");
        assert_eq!(sanitize("  .. "), "unnamed");
        assert_eq!(sanitize("正常名字"), "正常名字");
    }

    #[test]
    fn 日志尾部截取() {
        let dirs = test_dirs();
        let p = dirs.data.join("tail-test.log");
        std::fs::write(
            &p,
            (1..=150)
                .map(|i| format!("L{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let tail = tail_of_file(&p, 100);
        assert!(tail.starts_with("L51"));
        assert!(tail.ends_with("L150"));
        assert_eq!(tail_of_file(&dirs.data.join("no-such.log"), 10), "");
    }
}
