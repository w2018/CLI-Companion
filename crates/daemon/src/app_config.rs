//! app.json（GUI/daemon 偏好 + WebDAV 设置）与 secrets.json（DPAPI 凭据）

use crate::dirs::DataDirs;
use cli_companion_platform::dpapi;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 应用设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct AppConfig {
    pub version: u32,
    pub general: GeneralSettings,
    pub webdav: WebdavSettings,
    /// v2.2.0：本机只读状态页
    pub status_page: StatusPageSettings,
    /// v2.6.0：应用功能 · 内置 FTP 服务
    pub ftp: FtpSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            general: GeneralSettings::default(),
            webdav: WebdavSettings::default(),
            status_page: StatusPageSettings::default(),
            ftp: FtpSettings::default(),
        }
    }
}

/// 本机只读状态页（v2.2.0）：仅绑定 127.0.0.1，只读展示，无任何操作能力
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct StatusPageSettings {
    pub enabled: bool,
    pub port: u16,
}

impl Default for StatusPageSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 8765,
        }
    }
}

/// v2.6.0：应用功能 · 内置 FTP 服务设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct FtpSettings {
    /// 总开关（关闭时全部监听器下线）
    pub enabled: bool,
    /// 开机自启：daemon 启动时若 autostart=true 则自动运行 FTP；
    /// autostart=false 时 daemon 启动会强制 enabled=false（用户需手动启用）
    pub autostart: bool,
    /// 被动模式数据端口区间（含端点，全局共享）；0-0 = 系统分配临时端口（测试用）
    pub passive_port_start: u16,
    pub passive_port_end: u16,
    /// 多监听器：每个端口一个站点，各自绑定根目录
    pub listeners: Vec<FtpListener>,
    /// FTP 用户（全局，跨监听器共享账号）
    pub users: Vec<FtpUser>,
}

impl Default for FtpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            autostart: false,
            passive_port_start: 50_000,
            passive_port_end: 50_100,
            listeners: Vec::new(),
            users: Vec::new(),
        }
    }
}

/// FTP 监听器：一个控制端口 + 该端口的根目录（监狱根）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct FtpListener {
    /// 显示名称（GUI 用，如「文件分发」）
    pub name: String,
    /// 控制连接端口（FTP 标准 21，可自定义；多监听器端口不得重复）
    pub port: u16,
    /// 该端口的根目录（用户登录后被限制在此目录内）
    pub root: PathBuf,
    pub enabled: bool,
}

impl Default for FtpListener {
    fn default() -> Self {
        Self {
            name: String::new(),
            port: 21,
            root: PathBuf::new(),
            enabled: true,
        }
    }
}

/// FTP 用户：细粒度权限 + 多目录授权
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct FtpUser {
    pub username: String,
    /// 授权目录列表（绝对路径）：登录监听器要求其根目录在列表内；
    /// 其余授权目录以虚拟子目录挂载进会话根视图
    pub allowed_roots: Vec<PathBuf>,
    pub permissions: FtpPermissions,
    pub enabled: bool,
}

impl Default for FtpUser {
    fn default() -> Self {
        Self {
            username: String::new(),
            allowed_roots: Vec::new(),
            permissions: FtpPermissions::default(),
            enabled: true,
        }
    }
}

/// 文件/目录操作权限（按用户设置；默认只读）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
pub struct FtpPermissions {
    /// 浏览/列目录（LIST/NLST/MLSD/CWD/CDUP）
    pub list: bool,
    /// 下载（RETR/SIZE/MDTM）
    pub download: bool,
    /// 上传/写入（STOR/APPE）
    pub upload: bool,
    /// 删除文件/目录（DELE/RMD）
    pub delete: bool,
    /// 重命名/移动（RNFR/RNTO）
    pub rename: bool,
    /// 创建目录（MKD）
    pub mkdir: bool,
}

impl Default for FtpPermissions {
    fn default() -> Self {
        Self {
            list: true,
            download: true,
            upload: false,
            delete: false,
            rename: false,
            mkdir: false,
        }
    }
}

impl FtpSettings {
    /// 配置校验（保存 app.json 时调用；端口类规则仅在启用时强制）
    pub fn validate(&self) -> Result<(), String> {
        if self.users.len() > 100 {
            return Err("FTP 用户数量过多（上限 100）".into());
        }
        if self.listeners.len() > 16 {
            return Err("FTP 监听器数量过多（上限 16）".into());
        }
        let mut seen_users = std::collections::HashSet::new();
        for u in &self.users {
            if !valid_ftp_username(&u.username) {
                return Err(format!(
                    "FTP 用户名无效（限 1-64 位字母数字与 _.@-）: {:?}",
                    u.username
                ));
            }
            if !seen_users.insert(u.username.to_lowercase()) {
                return Err(format!("FTP 用户名重复: {}", u.username));
            }
            if u.allowed_roots.len() > 32 {
                return Err(format!("FTP 用户 {} 授权目录过多（上限 32）", u.username));
            }
            for p in &u.allowed_roots {
                if p.as_os_str().is_empty() {
                    return Err(format!("FTP 用户 {} 存在空的授权目录", u.username));
                }
            }
        }
        if !self.enabled {
            return Ok(());
        }
        if self.listeners.is_empty() {
            return Err("FTP 已启用但未配置任何监听端口".into());
        }
        let mut seen_ports = std::collections::HashSet::new();
        for l in &self.listeners {
            if l.port == 0 {
                return Err(format!("FTP 监听器 {} 端口不能为 0", l.name));
            }
            if l.root.as_os_str().is_empty() {
                return Err(format!("FTP 监听器 {} 未设置根目录", l.name));
            }
            if !seen_ports.insert(l.port) {
                return Err(format!("FTP 监听端口重复: {}", l.port));
            }
        }
        if self.passive_port_start > self.passive_port_end {
            return Err("FTP 被动端口区间无效（起始大于结束）".into());
        }
        // 0-0 = 临时端口（测试）；单边为 0 视为配置错误
        if (self.passive_port_start == 0) != (self.passive_port_end == 0) {
            return Err("FTP 被动端口区间无效（0 只能与 0 配对，表示系统自动分配）".into());
        }
        if self.passive_port_end != 0 && self.passive_port_end - self.passive_port_start > 1000 {
            return Err("FTP 被动端口区间过大（最多 1000 个端口）".into());
        }
        Ok(())
    }
}

/// 用户名规则：1-64 位 ASCII 字母数字与 `_.@-`
fn valid_ftp_username(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'@' | b'-'))
}

/// 路径比较用的规整化（小写 + 正斜杠 + 去结尾分隔符 + 去verbatim前缀），
/// 仅用于授权匹配/去重，不做文件系统调用
pub fn normalize_path_key(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s
        .strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .unwrap_or(s);
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    s.trim_end_matches('\\').to_lowercase()
}

/// 通用设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GeneralSettings {
    pub language: String,
    pub theme: String,
    /// 关闭窗口时隐藏到托盘
    pub close_to_tray: bool,
    /// 服务崩溃 / 自动重启失败时发送系统 Toast 通知（默认开）
    #[serde(default = "default_true")]
    pub notify_on_failure: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            theme: "system".into(),
            close_to_tray: true,
            notify_on_failure: true,
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
    /// v2.2.0：机密环境变量（键 `svc:<service_id>:<env_name>`，值 "dpapi:<hex>"）
    pub env_secrets: std::collections::BTreeMap<String, String>,
    /// v2.6.0：FTP 用户密码（键 = 用户名，值 "dpapi:<hex>"）
    #[serde(default)]
    pub ftp_passwords: std::collections::BTreeMap<String, String>,
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

    /// 设置 FTP 用户密码（DPAPI 加密后写入）
    pub fn set_ftp_password(&mut self, username: &str, plain: &str) -> Result<(), std::io::Error> {
        let blob = dpapi::protect(plain)?;
        self.ftp_passwords
            .insert(username.to_string(), blob.to_storage_string());
        Ok(())
    }

    /// 读取 FTP 用户密码明文（键大小写不敏感）
    pub fn ftp_password(&self, username: &str) -> Option<String> {
        self.ftp_passwords
            .keys()
            .find(|k| k.eq_ignore_ascii_case(username))
            .and_then(|k| dpapi::unprotect(&self.ftp_passwords[k]).ok())
    }

    /// 仅保留给定用户（大小写不敏感）的密码；返回是否发生变更
    pub fn retain_ftp_users(&mut self, usernames: &[String]) -> bool {
        let keep: Vec<String> = usernames.iter().map(|u| u.to_lowercase()).collect();
        let before = self.ftp_passwords.len();
        self.ftp_passwords
            .retain(|k, _| keep.contains(&k.to_lowercase()));
        before != self.ftp_passwords.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认ftp配置合法() {
        FtpSettings::default().validate().unwrap();
    }

    #[test]
    fn 启用时必须至少一个监听器() {
        let mut ftp = FtpSettings {
            enabled: true,
            ..Default::default()
        };
        assert!(ftp.validate().is_err());
        ftp.listeners.push(FtpListener {
            name: "文件".into(),
            port: 21,
            root: r"D:\ftp".into(),
            enabled: true,
        });
        ftp.validate().unwrap();
    }

    #[test]
    fn 监听器端口不能重复() {
        let ftp = FtpSettings {
            enabled: true,
            listeners: vec![
                FtpListener {
                    name: "a".into(),
                    port: 21,
                    root: r"D:\a".into(),
                    enabled: true,
                },
                FtpListener {
                    name: "b".into(),
                    port: 21,
                    root: r"D:\b".into(),
                    enabled: true,
                },
            ],
            ..Default::default()
        };
        assert!(ftp.validate().is_err());
    }

    #[test]
    fn 用户名规则与唯一性() {
        let mk = |name: &str| FtpUser {
            username: name.into(),
            ..Default::default()
        };
        let too_long = "a".repeat(65);
        let bad = ["", "含 空格", "中文", "a:b", too_long.as_str()];
        for name in bad {
            let ftp = FtpSettings {
                users: vec![mk(name)],
                ..Default::default()
            };
            assert!(ftp.validate().is_err(), "应拒绝用户名 {name:?}");
        }
        let ok = FtpSettings {
            users: vec![mk("alice"), mk("Bob_001@x-y")],
            ..Default::default()
        };
        ok.validate().unwrap();
        // 大小写不敏感的唯一性
        let dup = FtpSettings {
            users: vec![mk("alice"), mk("ALICE")],
            ..Default::default()
        };
        assert!(dup.validate().is_err());
    }

    #[test]
    fn 被动端口区间校验() {
        let base = |s: u16, e: u16| FtpSettings {
            enabled: true,
            passive_port_start: s,
            passive_port_end: e,
            listeners: vec![FtpListener {
                port: 21,
                root: r"D:\a".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        base(50_000, 50_100).validate().unwrap();
        base(0, 0).validate().unwrap(); // 临时端口（测试）
        assert!(base(50_100, 50_000).validate().is_err());
        assert!(base(0, 50_100).validate().is_err());
        assert!(base(50_000, 51_001).validate().is_err());
    }

    #[test]
    fn ftp密码dpapi往返() {
        let mut s = Secrets::default();
        s.set_ftp_password("alice", "p@ss-密码123").unwrap();
        assert_eq!(s.ftp_password("alice").unwrap(), "p@ss-密码123");
        // 键大小写不敏感
        assert_eq!(s.ftp_password("ALICE").unwrap(), "p@ss-密码123");
        assert!(s.ftp_password("bob").is_none());
        // 明文不落盘
        let raw = serde_json::to_string(&s).unwrap();
        assert!(!raw.contains("p@ss-密码123"));
        assert!(raw.contains("dpapi:"));
    }

    #[test]
    fn 仅保留指定用户的密码() {
        let mut s = Secrets::default();
        s.set_ftp_password("alice", "a").unwrap();
        s.set_ftp_password("bob", "b").unwrap();
        assert!(s.retain_ftp_users(&["Alice".into()]));
        assert!(s.ftp_password("alice").is_some());
        assert!(s.ftp_password("bob").is_none());
        assert!(!s.retain_ftp_users(&["Alice".into()])); // 无变更
    }

    #[test]
    fn 路径规整化键大小写与分隔符无关() {
        assert_eq!(
            normalize_path_key(std::path::Path::new(r"D:\Media\Share")),
            normalize_path_key(std::path::Path::new("d:/media/share/"))
        );
        assert_eq!(
            normalize_path_key(std::path::Path::new(r"\\?\C:\x")),
            r"c:\x"
        );
    }
}
