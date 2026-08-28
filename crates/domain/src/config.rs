//! services.json 顶层配置（开发文档 §5.2、§12.2）

use crate::service::ServiceDefinition;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// services.json 的顶层结构
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ServicesConfig {
    /// schema 版本；缺失视为 0 并自动迁移
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub services: Vec<ServiceDefinition>,
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            version: crate::SCHEMA_VERSION,
            services: Vec::new(),
        }
    }
}

/// 配置解析/校验错误
#[derive(Debug)]
pub enum ConfigError {
    /// JSON 解析失败
    Parse(serde_json::Error),
    /// 版本高于当前支持（禁止静默降级）
    VersionTooNew { found: u32, supported: u32 },
    /// 其他校验失败
    Validation(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse(e) => write!(f, "配置 JSON 解析失败: {e}"),
            ConfigError::VersionTooNew { found, supported } => {
                write!(
                    f,
                    "配置版本 {found} 高于当前支持的 {supported}，禁止自动降级"
                )
            }
            ConfigError::Validation(msg) => write!(f, "配置校验失败: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ServicesConfig {
    /// 从 JSON 解析并迁移到当前版本（未知字段拒绝）
    pub fn from_json(raw: &str) -> Result<Self, ConfigError> {
        let cfg: ServicesConfig = serde_json::from_str(raw).map_err(ConfigError::Parse)?;
        crate::migration::migrate_to_current(cfg)
    }

    /// 序列化为美化 JSON
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// 按 ID 查找服务
    pub fn find(&self, id: &Uuid) -> Option<&ServiceDefinition> {
        self.services.iter().find(|s| &s.id == id)
    }

    /// 校验服务名不重复、字段合法
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for svc in &self.services {
            if svc.name.trim().is_empty() {
                return Err(ConfigError::Validation(format!("服务 {} 名称为空", svc.id)));
            }
            if svc.exe.as_os_str().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "服务 {} 未配置 exe",
                    svc.id
                )));
            }
            if !seen.insert(&svc.name) {
                return Err(ConfigError::Validation(format!("服务名重复: {}", svc.name)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Arg;

    /// 开发文档 §12.2 示例配置的最小变体，作为回归 fixture
    const SAMPLE: &str = r#"{
      "version": 1,
      "services": [
        {
          "id": "a2b9c0d1-0000-4000-8000-000000000001",
          "name": "示例代理",
          "description": "演示运行时参数",
          "enabled": true,
          "autostart": true,
          "exe": "C:/Program Files/Example/agent.exe",
          "args": [
            {"id":"a1","key":"--bind","value":"127.0.0.1:7000","enabled":true,"kind":"option"},
            {"id":"a2","key":"--verbose","value":null,"enabled":false,"kind":"flag"}
          ],
          "working_dir": "C:/Program Files/Example/data",
          "env": [],
          "run_as": {"kind":"current_user"},
          "console": {"mode":"new_console_visible","startup":"normal"},
          "stop": {"signal":"ctrl_c","graceful_timeout_ms":15000,"kill_timeout_ms":10000},
          "health": {"kind":"process","interval_ms":5000,"failure_threshold":3,"success_threshold":1},
          "restart": {"policy":"always","max_attempts_10m":10,"backoff":{"initial_ms":2000,"max_ms":300000,"multiplier":2}},
          "created_at": "2026-08-28T08:00:00+08:00",
          "updated_at": "2026-08-28T08:00:00+08:00"
        }
      ]
    }"#;

    #[test]
    fn 示例配置可解析并校验通过() {
        let cfg = ServicesConfig::from_json(SAMPLE).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.services.len(), 1);
        cfg.validate().unwrap();
        assert_eq!(cfg.services[0].name, "示例代理");
    }

    #[test]
    fn 未知字段被拒绝() {
        let bad = r#"{"version":1,"services":[],"unknown_field":123}"#;
        let err = ServicesConfig::from_json(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn 版本缺失视为0并迁移() {
        let legacy = r#"{"services":[]}"#;
        let cfg = ServicesConfig::from_json(legacy).unwrap();
        assert_eq!(cfg.version, crate::SCHEMA_VERSION);
    }

    #[test]
    fn 更高版本被拒绝() {
        let newer = r#"{"version":99,"services":[]}"#;
        let err = ServicesConfig::from_json(newer).unwrap_err();
        assert!(matches!(err, ConfigError::VersionTooNew { .. }));
    }

    #[test]
    fn 重复服务名校验失败() {
        let mut cfg = ServicesConfig::default();
        let mut s1 = ServiceDefinition::new("同名", "a.exe".into());
        s1.id = Uuid::new_v4();
        let mut s2 = ServiceDefinition::new("同名", "b.exe".into());
        s2.id = Uuid::new_v4();
        s2.args = Vec::<Arg>::new();
        cfg.services.push(s1);
        cfg.services.push(s2);
        assert!(cfg.validate().is_err());
    }
}
