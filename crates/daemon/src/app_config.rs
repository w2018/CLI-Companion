//! app.json（GUI/daemon 偏好 + WebDAV 设置）与 secrets.json（DPAPI 凭据）

use crate::dirs::DataDirs;
use cli_companion_platform::dpapi;
use serde::{Deserialize, Serialize};

/// 应用设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct AppConfig {
    pub version: u32,
    pub general: GeneralSettings,
    pub webdav: WebdavSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            general: GeneralSettings::default(),
            webdav: WebdavSettings::default(),
        }
    }
}

/// 通用设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GeneralSettings {
    pub language: String,
    pub theme: String,
    /// 关闭窗口时隐藏到托盘
    pub close_to_tray: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            theme: "system".into(),
            close_to_tray: true,
        }
    }
}

/// WebDAV 同步设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct WebdavSettings {
    pub enabled: bool,
    /// 服务器根 URL，如 https://dav.example.com/dav/
    pub url: String,
    pub username: String,
    /// 远端目录（相对 url），如 cli-companion
    pub remote_dir: String,
    pub sync_interval_minutes: u32,
    pub verify_tls: bool,
    /// 是否同步配置文件（services.json / app.json）
    pub sync_config: bool,
    /// 是否同步 cli 目录中的二进制应用（递归子目录与文件）
    pub sync_cli_apps: bool,
}

impl Default for WebdavSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            username: String::new(),
            remote_dir: "cli-companion".into(),
            sync_interval_minutes: 15,
            verify_tls: true,
            sync_config: true,
            sync_cli_apps: false,
        }
    }
}

/// secrets.json：DPAPI 加密的凭据（不参与同步）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct Secrets {
    /// WebDAV 密码，"dpapi:<hex>" 格式
    pub webdav_password_dpapi: Option<String>,
}

impl Secrets {
    /// 保存 WebDAV 密码（DPAPI 加密后写入）
    pub fn set_webdav_password(&mut self, plain: &str) -> Result<(), std::io::Error> {
        let blob = dpapi::protect(plain)?;
        self.webdav_password_dpapi = Some(blob.to_storage_string());
        Ok(())
    }

    /// 读取 WebDAV 密码明文
    pub fn webdav_password(&self) -> Option<String> {
        self.webdav_password_dpapi
            .as_deref()
            .and_then(|s| dpapi::unprotect(s).ok())
    }
}

/// 加载 app.json；不存在或损坏时返回默认值
pub fn load_app(dirs: &DataDirs) -> AppConfig {
    match std::fs::read_to_string(dirs.app_json()) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!("app.json 解析失败，使用默认值: {e}");
            AppConfig::default()
        }),
        Err(_) => AppConfig::default(),
    }
}

/// 加载 secrets.json
pub fn load_secrets(dirs: &DataDirs) -> Secrets {
    std::fs::read_to_string(dirs.secrets_json())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// 原子保存 app.json
pub fn save_app(dirs: &DataDirs, app: &AppConfig) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(app)?;
    crate::dirs::atomic_write(&dirs.app_json(), &json)
}

/// 原子保存 secrets.json（文件权限依赖目录 ACL）
pub fn save_secrets(dirs: &DataDirs, secrets: &Secrets) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(secrets)?;
    crate::dirs::atomic_write(&dirs.secrets_json(), &json)
}
