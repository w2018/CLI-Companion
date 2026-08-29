//! 机密环境变量加密落盘（v2.2.0）
//!
//! 服务定义中 `secret=true` 的环境变量值不再明文存 services.json：
//! - 保存（create/update）时真实值经 DPAPI 加密写入 secrets.json，
//!   services.json 只留占位符 [`ENCRYPTED_PLACEHOLDER`]
//! - 启动进程时按占位符从 secrets.json 解密注入真实值
//! - 导出/WebDAV 同步的配置文件从此不含机密明文
//! - 旧配置的明文机密由 [`migrate_existing`] 在 daemon 启动时一次性迁移

use crate::app_config::{load_secrets, save_secrets, Secrets};
use crate::dirs::DataDirs;
use cli_companion_domain::{ServiceDefinition, ServiceId};
use std::collections::BTreeMap;

/// services.json 中机密值的占位符（用户手输这个串的概率可忽略）
pub const ENCRYPTED_PLACEHOLDER: &str = "__encrypted__";

/// 机密键名
pub fn secret_key(service_id: &ServiceId, env_name: &str) -> String {
    format!("svc:{service_id}:{env_name}")
}

/// 读取某个机密环境变量的真实值（解密失败返回 None）
pub fn load_secret(dirs: &DataDirs, service_id: &ServiceId, env_name: &str) -> Option<String> {
    let secrets = load_secrets(dirs);
    let blob = secrets.env_secrets.get(&secret_key(service_id, env_name))?;
    cli_companion_platform::dpapi::unprotect(blob).ok()
}

fn encrypt_plain(plain: &str) -> Result<String, std::io::Error> {
    let protected = cli_companion_platform::dpapi::protect(plain)?;
    Ok(protected.to_storage_string())
}

/// 保存前拦截：把机密值写入 secrets.json，services.json 中置占位符
///
/// 语义：占位符 = 不变；空串 = 清除；其他 = 更新。
/// 同时清理该服务已不存在变量名的孤儿键。
pub fn sanitize_service_secrets(
    dirs: &DataDirs,
    service_id: &ServiceId,
    svc: &mut ServiceDefinition,
) -> Result<(), String> {
    let mut secrets = load_secrets(dirs);
    let prefix = format!("svc:{service_id}:");
    let mut live_keys = std::collections::BTreeSet::new();
    for e in svc.env.iter_mut().filter(|e| e.secret) {
        let key = secret_key(service_id, &e.name);
        if e.value == ENCRYPTED_PLACEHOLDER {
            // 不变：占位符原样保留；若 secrets 缺失则该值实际丢失（记录日志）
            if !secrets.env_secrets.contains_key(&key) {
                tracing::warn!(service = %service_id, env = %e.name, "机密占位符无对应存储值");
            }
        } else if e.value.is_empty() {
            secrets.env_secrets.remove(&key); // 清除
        } else {
            let blob = encrypt_plain(&e.value).map_err(|er| format!("DPAPI 加密失败: {er}"))?;
            secrets.env_secrets.insert(key, blob);
            e.value = ENCRYPTED_PLACEHOLDER.to_string();
        }
        live_keys.insert(secret_key(service_id, &e.name));
        let _ = &prefix;
    }
    // 清孤儿：该服务下已不存在的变量名
    let orphans: Vec<String> = secrets
        .env_secrets
        .range(format!("svc:{service_id}:")..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .map(|(k, _)| k.clone())
        .filter(|k| !live_keys.contains(k))
        .collect();
    for k in orphans {
        secrets.env_secrets.remove(&k);
    }
    save_secrets(dirs, &secrets).map_err(|e| format!("保存机密失败: {e}"))
}

/// 删除服务时清理其全部机密
pub fn prune_service(dirs: &DataDirs, service_id: &ServiceId) {
    let mut secrets = load_secrets(dirs);
    let prefix = format!("svc:{service_id}:");
    let orphans: Vec<String> = secrets
        .env_secrets
        .range(prefix.clone()..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .map(|(k, _)| k.clone())
        .collect();
    if orphans.is_empty() {
        return;
    }
    for k in orphans {
        secrets.env_secrets.remove(&k);
    }
    if let Err(e) = save_secrets(dirs, &secrets) {
        tracing::warn!("清理服务机密失败: {e}");
    }
}

/// 启动迁移：把 services.json 中明文机密搬到 secrets.json（幂等）
///
/// 通过 save_services 落盘（自动先快照旧配置）。
pub async fn migrate_existing(state: &crate::state::AppState) {
    let cfg = state.services().await;
    let mut changed = false;
    let mut cfg = cfg;
    for svc in cfg.services.iter_mut() {
        for e in svc.env.iter_mut().filter(|e| e.secret) {
            if !e.value.is_empty() && e.value != ENCRYPTED_PLACEHOLDER {
                changed = true;
            }
        }
    }
    if !changed {
        return;
    }
    let mut secrets = load_secrets(&state.dirs);
    for svc in cfg.services.iter_mut() {
        for e in svc.env.iter_mut().filter(|e| e.secret) {
            if e.value.is_empty() || e.value == ENCRYPTED_PLACEHOLDER {
                continue;
            }
            match encrypt_plain(&e.value) {
                Ok(blob) => {
                    secrets
                        .env_secrets
                        .insert(secret_key(&svc.id, &e.name), blob);
                    e.value = ENCRYPTED_PLACEHOLDER.to_string();
                    tracing::info!(service = %svc.id, env = %e.name, "已迁移明文机密到加密存储");
                }
                Err(er) => tracing::warn!(service = %svc.id, env = %e.name, "机密迁移失败: {er}"),
            }
        }
    }
    if let Err(err) = save_secrets(&state.dirs, &secrets) {
        tracing::warn!("迁移机密存储失败: {err}");
        return;
    }
    if let Err(err) = state.save_services(cfg).await {
        tracing::warn!("迁移后写回配置失败: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dirs() -> DataDirs {
        DataDirs::resolve(Some(std::env::temp_dir().join(format!(
            "cc-secret-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))))
    }

    fn svc_with_secret(value: &str) -> ServiceDefinition {
        let mut s = ServiceDefinition::new("测试", "a.exe".into());
        s.env = vec![cli_companion_domain::EnvVar {
            name: "TOKEN".into(),
            value: value.into(),
            secret: true,
        }];
        s
    }

    #[test]
    fn 机密保存后落盘为占位符且可解密回读() {
        let dirs = test_dirs();
        let id = uuid::Uuid::new_v4();
        let mut svc = svc_with_secret("super-secret-123");
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        // services.json 侧：占位符
        assert_eq!(svc.env[0].value, ENCRYPTED_PLACEHOLDER);
        // secrets 侧：解密回原值
        assert_eq!(
            load_secret(&dirs, &id, "TOKEN").as_deref(),
            Some("super-secret-123")
        );
    }

    #[test]
    fn 占位符表示不变_空串清除_孤儿清理() {
        let dirs = test_dirs();
        let id = uuid::Uuid::new_v4();
        let mut svc = svc_with_secret("v1");
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        // 再加一个变量后只保留 TOKEN：OLD 成为孤儿被清理
        svc.env.push(cli_companion_domain::EnvVar {
            name: "OLD".into(),
            value: "gone".into(),
            secret: true,
        });
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        assert!(load_secret(&dirs, &id, "OLD").is_some());
        svc.env.pop();
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        assert!(load_secret(&dirs, &id, "OLD").is_none());
        // 占位符 = 不变
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        assert_eq!(load_secret(&dirs, &id, "TOKEN").as_deref(), Some("v1"));
        // 空串 = 清除
        svc.env[0].value = String::new();
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        assert_eq!(load_secret(&dirs, &id, "TOKEN"), None);
    }

    #[test]
    fn 非机密变量不受影响() {
        let dirs = test_dirs();
        let id = uuid::Uuid::new_v4();
        let mut svc = svc_with_secret("x");
        svc.env.push(cli_companion_domain::EnvVar {
            name: "PLAIN".into(),
            value: "明文保留".into(),
            secret: false,
        });
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        assert_eq!(svc.env[1].value, "明文保留");
    }

    #[test]
    fn 删除服务清理其机密() {
        let dirs = test_dirs();
        let id = uuid::Uuid::new_v4();
        let mut svc = svc_with_secret("v");
        sanitize_service_secrets(&dirs, &id, &mut svc).unwrap();
        prune_service(&dirs, &id);
        assert!(load_secret(&dirs, &id, "TOKEN").is_none());
    }
}
