//! 运行时目录布局（开发文档 §1.2、§12.1）
//!
//! 默认以 exe 所在目录为根：`<root>/{config,data,logs,cache}`。
//! 开发时可使用 `--data-dir` 覆盖。

use std::path::{Path, PathBuf};

/// 数据根目录命令行覆盖（--data-dir <dir>）
pub fn data_dir_override() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

/// 默认数据根目录选择策略：
/// 1. 便携模式：exe 同目录存在 portable.marker → 用 exe 目录
/// 2. 开发布局：exe 同目录已有 config/（历史开发数据）→ 沿用
/// 3. 默认：%LOCALAPPDATA%\CLICompanion（Program Files 安装时必然不可写，
///    可变数据绝不能放安装目录 —— 修复安装版 daemon 启动即退出的问题）
fn default_root() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("portable.marker").is_file() {
                return dir.to_path_buf();
            }
            if dir.join("config").is_dir() {
                return dir.to_path_buf();
            }
        }
    }
    std::env::var("LOCALAPPDATA")
        .map(|v| PathBuf::from(v).join("CLICompanion"))
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Debug, Clone)]
pub struct DataDirs {
    pub root: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
    /// 每个服务的日志目录
    pub service_logs: PathBuf,
    /// 存放受管二进制应用的目录（可随 WebDAV 同步）
    pub cli: PathBuf,
}

impl DataDirs {
    /// 解析目录：优先 root 覆盖，其次按默认策略选择
    pub fn resolve(override_root: Option<PathBuf>) -> Self {
        let root = override_root.unwrap_or_else(default_root);
        let config = root.join("config");
        let data = root.join("data");
        let logs = root.join("logs");
        let cache = root.join("cache");
        let service_logs = logs.join("services");
        let cli = root.join("cli");
        // 确保目录存在
        for dir in [&config, &data, &logs, &cache, &service_logs, &cli] {
            let _ = std::fs::create_dir_all(dir);
        }
        Self {
            root,
            config,
            data,
            logs,
            cache,
            service_logs,
            cli,
        }
    }

    pub fn services_json(&self) -> PathBuf {
        self.config.join("services.json")
    }
    pub fn app_json(&self) -> PathBuf {
        self.config.join("app.json")
    }
    pub fn secrets_json(&self) -> PathBuf {
        self.config.join("secrets.json")
    }
    pub fn sync_state_json(&self) -> PathBuf {
        self.data.join("sync-state.json")
    }
    pub fn daemon_lock(&self) -> PathBuf {
        self.data.join("daemon.lock")
    }
    pub fn service_log(&self, id: &str) -> PathBuf {
        self.service_logs.join(format!("{id}.log"))
    }
}

/// 原子写入：写临时文件后重命名覆盖（开发文档 §5.2）
pub fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    // Windows 上 rename 覆盖已存在目标（MOVEFILE_REPLACE_EXISTING 语义）
    std::fs::rename(&tmp, path)?;
    Ok(())
}
