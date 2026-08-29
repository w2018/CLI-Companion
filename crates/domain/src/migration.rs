//! 配置版本迁移（开发文档 §5.2）
//!
//! V1 仅有版本 0（缺失）→ 1 一条迁移路径。
//! 破坏性变更必须新增版本号与迁移函数，禁止从更高版本静默降级。

use crate::config::{ConfigError, ServicesConfig};
use crate::SCHEMA_VERSION;

/// 将任意历史版本配置迁移到当前版本
pub fn migrate_to_current(mut cfg: ServicesConfig) -> Result<ServicesConfig, ConfigError> {
    if cfg.version > SCHEMA_VERSION {
        return Err(ConfigError::VersionTooNew {
            found: cfg.version,
            supported: SCHEMA_VERSION,
        });
    }
    // 逐版本升级：0 → 1 → 2
    if cfg.version < 1 {
        cfg = migrate_0_to_1(cfg);
    }
    if cfg.version < 2 {
        cfg = migrate_1_to_2(cfg);
    }
    Ok(cfg)
}

/// 版本 0（无 version 字段的初版）→ 版本 1
///
/// 版本 0 与版本 1 结构相同，仅补写版本号；
/// 后续版本在此处按顺序追加 migrate_2_to_3 等函数。
fn migrate_0_to_1(mut cfg: ServicesConfig) -> ServicesConfig {
    cfg.version = 1;
    cfg
}

/// 版本 1 → 版本 2：新增可选字段（mem_alert_mb / HealthKind::Command）均带
/// serde default，旧数据无需变换，仅补写版本号。
fn migrate_1_to_2(mut cfg: ServicesConfig) -> ServicesConfig {
    cfg.version = 2;
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不允许降级() {
        let cfg = ServicesConfig {
            version: 3,
            services: vec![],
        };
        let err = migrate_to_current(cfg).unwrap_err();
        assert!(matches!(err, ConfigError::VersionTooNew { found: 3, .. }));
    }

    #[test]
    fn v1迁移到当前版本仅补版本号() {
        let cfg = ServicesConfig {
            version: 1,
            services: vec![],
        };
        let up = migrate_to_current(cfg).unwrap();
        assert_eq!(up.version, crate::SCHEMA_VERSION);
    }
}
