//! services.json 自动备份与回滚（v2.2.0）
//!
//! 每次保存前快照当前配置到 `data/backups/`，保留最近 [`MAX_BACKUPS`] 份；
//! 回滚 = 读备份 → 走正常 save_services（本身又会先快照当前，双保险）。

use crate::dirs::DataDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 保留的备份数上限
pub const MAX_BACKUPS: usize = 20;

/// 备份文件名前缀（同时用于防路径穿越：只接受 `<PREFIX>*.json`）
const PREFIX: &str = "services-";

/// 单个备份的元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    /// 文件名（services-<时间戳>.json）
    pub name: String,
    /// 快照时间（从文件名解析，RFC 3339）
    pub ts: String,
    /// 文件大小（字节）
    pub size: u64,
}

/// 保存前快照当前 services.json（文件不存在时静默跳过；失败只记日志不影响保存）
pub fn snapshot_before_save(dirs: &DataDirs) {
    let src = dirs.services_json();
    let Ok(content) = std::fs::read_to_string(&src) else {
        return;
    };
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let dest = dirs.backups().join(format!("{PREFIX}{ts}.json"));
    let result =
        std::fs::create_dir_all(dirs.backups()).and_then(|_| std::fs::write(&dest, content));
    if let Err(e) = result {
        tracing::warn!("配置备份失败: {e}");
        return;
    }
    prune_old(dirs);
}

/// 删除超出上限的旧备份（按文件名时间戳升序，删除最旧）
fn prune_old(dirs: &DataDirs) {
    let Ok(entries) = std::fs::read_dir(dirs.backups()) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort(); // 文件名含毫秒时间戳，字典序 = 时间序
    while names.len() > MAX_BACKUPS {
        let oldest = names.remove(0);
        let _ = std::fs::remove_file(dirs.backups().join(&oldest));
    }
}

/// 列出全部备份（最新在前）
pub fn list(dirs: &DataDirs) -> Vec<BackupMeta> {
    let Ok(entries) = std::fs::read_dir(dirs.backups()) else {
        return Vec::new();
    };
    let mut metas: Vec<BackupMeta> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            (name, size)
        })
        .filter(|(name, _)| name.starts_with(PREFIX) && name.ends_with(".json"))
        .map(|(name, size)| BackupMeta {
            ts: filename_to_rfc3339(&name),
            name,
            size,
        })
        .collect();
    metas.sort_by(|a, b| b.name.cmp(&a.name));
    metas
}

/// 读取备份内容；name 仅接受 `<PREFIX>*.json`（防路径穿越）
pub fn read(dirs: &DataDirs, name: &str) -> Result<String, String> {
    let path = validated_path(dirs, name)?;
    std::fs::read_to_string(&path).map_err(|e| format!("读取备份失败: {e}"))
}

/// 备份文件的安全路径解析
fn validated_path(dirs: &DataDirs, name: &str) -> Result<PathBuf, String> {
    if !name.starts_with(PREFIX)
        || !name.ends_with(".json")
        || name.contains('\\')
        || name.contains('/')
        || name.contains("..")
    {
        return Err(format!("非法备份名: {name}"));
    }
    Ok(dirs.backups().join(name))
}

/// 文件名 `services-20260829-153000.123.json` → RFC 3339 本地时间
fn filename_to_rfc3339(name: &str) -> String {
    let stem = name.trim_start_matches(PREFIX).trim_end_matches(".json");
    use chrono::TimeZone;
    match chrono::NaiveDateTime::parse_from_str(stem, "%Y%m%d-%H%M%S%.3f") {
        Ok(naive) => chrono::Local
            .from_local_datetime(&naive)
            .single()
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| stem.to_string()),
        Err(_) => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dirs() -> DataDirs {
        DataDirs::resolve(Some(std::env::temp_dir().join(format!(
            "cc-backup-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))))
    }

    #[test]
    fn 保存前快照并保留上限() {
        let dirs = test_dirs();
        // 无 services.json → 无快照
        snapshot_before_save(&dirs);
        assert!(list(&dirs).is_empty());
        // 写 25 份 → 只留最新 20 份（间隔 2ms 保证时间戳文件名唯一）
        for i in 0..25 {
            std::fs::write(dirs.services_json(), format!("{{\"v\":{i}}}")).unwrap();
            snapshot_before_save(&dirs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let all = list(&dirs);
        assert_eq!(all.len(), MAX_BACKUPS);
        // 最新在前：第一个的名字字典序最大
        let mut sorted = all.clone();
        sorted.sort_by(|a, b| b.name.cmp(&a.name));
        assert_eq!(all[0].name, sorted[0].name);
    }

    #[test]
    fn 读取与非法名拒绝() {
        let dirs = test_dirs();
        std::fs::write(dirs.services_json(), "{\"ok\":true}").unwrap();
        snapshot_before_save(&dirs);
        let name = list(&dirs)[0].name.clone();
        assert!(read(&dirs, &name).unwrap().contains("\"ok\":true"));
        assert!(read(&dirs, "../secrets.json").is_err());
        assert!(read(&dirs, "app-1.json").is_err());
    }

    #[test]
    fn 文件名时间戳可解析() {
        let ts = filename_to_rfc3339("services-20260829-153000.123.json");
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok());
    }
}
