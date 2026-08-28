//! WebDAV 配置同步（开发文档 §8）
//!
//! 策略：本地最后写胜出（LWW）；双方修改时远端内容另存为冲突文件后上传本地。
//! 幂等键：last_synced_etag + last_synced_local_sha。

use crate::state::AppState;
use chrono::Utc;
use cli_companion_domain::ServicesConfig;
use cli_companion_protocol::error::ErrorCode;
use cli_companion_protocol::RpcError;
use cli_companion_webdav::{WebdavClient, WebdavError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 同步引擎（busy 锁保证单一同步任务）
#[derive(Default)]
pub struct SyncEngine {
    busy: Mutex<()>,
}

/// 同步状态持久化（data/sync-state.json）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SyncState {
    pub last_run: Option<String>,
    pub last_direction: Option<String>,
    pub last_action: Option<String>,
    pub last_error: Option<String>,
    pub last_synced_etag: Option<String>,
    pub last_synced_local_sha: Option<String>,
}

/// 同步报告
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncReport {
    pub action: String,
    pub message: String,
    pub conflict_file: Option<String>,
    /// cli 目录上传数
    pub uploaded: u32,
    /// cli 目录下载数
    pub downloaded: u32,
    /// cli 目录跳过数（内容一致）
    pub skipped: u32,
}

/// cli 目录同步统计
#[derive(Debug, Default)]
struct CliSyncStats {
    uploaded: u32,
    downloaded: u32,
    skipped: u32,
}

impl SyncEngine {
    pub fn new() -> Self {
        Self {
            busy: Mutex::new(()),
        }
    }

    fn remote_file_path(settings: &crate::app_config::WebdavSettings) -> String {
        let dir = settings.remote_dir.trim_matches('/');
        format!("{dir}/services.json")
    }

    fn load_state(state: &AppState) -> SyncState {
        std::fs::read_to_string(state.dirs.sync_state_json())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_state(state: &AppState, st: &SyncState) {
        if let Ok(json) = serde_json::to_string_pretty(st) {
            let _ = crate::dirs::atomic_write(&state.dirs.sync_state_json(), &json);
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 构建 WebDAV 客户端（从配置 + secrets 读凭据）
    async fn build_client(
        state: &AppState,
    ) -> Result<(WebdavClient, crate::app_config::WebdavSettings), RpcError> {
        let app = state.app().await;
        let settings = app.webdav.clone();
        if !settings.enabled {
            return Err(RpcError::new(ErrorCode::Validation, "WebDAV 同步未启用"));
        }
        if settings.url.is_empty() {
            return Err(RpcError::new(ErrorCode::Validation, "WebDAV URL 为空"));
        }
        let password = state
            .config
            .lock()
            .await
            .secrets
            .webdav_password()
            .unwrap_or_default();
        let client = WebdavClient::new(
            &settings.url,
            settings.username.clone(),
            password,
            settings.verify_tls,
        )
        .map_err(|e| RpcError::new(ErrorCode::WebdavProtocol, e.to_string()))?;
        Ok((client, settings))
    }

    fn map_webdav_err(e: &WebdavError) -> RpcError {
        let code = match e {
            WebdavError::Auth => ErrorCode::WebdavAuth,
            WebdavError::Network(_) => ErrorCode::WebdavServer,
            // 409/412 前置条件冲突：远端已被修改或目录状态异常
            WebdavError::PreconditionFailed => ErrorCode::Conflict,
            _ => ErrorCode::WebdavProtocol,
        };
        RpcError::new(code, e.to_string())
    }

    /// 执行一次同步（手动或周期调度共用；busy 锁防并发）
    ///
    /// 按设置分项同步：配置文件（services.json）+ 可选 cli 应用目录（递归）。
    pub async fn run(self: Arc<Self>, state: AppState) -> Result<Value, RpcError> {
        let _guard = self.busy.lock().await;
        let (client, settings) = Self::build_client(&state).await?;
        let mut sync_st = Self::load_state(&state);
        let dir = settings.remote_dir.trim_matches('/');

        // 确保远端根目录存在
        if let Err(e) = client.mkcol(dir).await {
            tracing::debug!("mkcol 根目录（可能已存在）: {e}");
        }

        let mut report = SyncReport::default();
        let mut messages = Vec::new();

        // ===== 1. 同步配置文件 =====
        if settings.sync_config {
            let cfg_report = Self::sync_config(
                &client,
                &state,
                &Self::remote_file_path(&settings),
                &mut sync_st,
            )
            .await?;
            messages.push(cfg_report.message.clone());
            report.action = cfg_report.action.clone();
            report.conflict_file = cfg_report.conflict_file;
        }

        // ===== 2. 同步 cli 应用目录（递归子目录与文件）=====
        if settings.sync_cli_apps {
            let remote_cli = format!("{dir}/cli");
            client
                .mkcol(&remote_cli)
                .await
                .map_err(|e| Self::map_webdav_err(&e))?;
            let stats = Self::sync_cli_tree(&client, dir, "cli", &state.dirs.cli).await?;
            report.uploaded += stats.uploaded;
            report.downloaded += stats.downloaded;
            report.skipped += stats.skipped;
            if (stats.uploaded > 0 || stats.downloaded > 0) && report.action == "noop" {
                report.action = "sync_cli".into();
            }
            messages.push(format!(
                "CLI 应用：上传 {} / 下载 {} / 一致 {}",
                stats.uploaded, stats.downloaded, stats.skipped
            ));
        }

        // 汇总报告
        if report.message.is_empty() {
            report.message = messages.join("；");
        } else {
            report.message = format!("{}；{}", report.message, messages.join("；"));
        }
        if report.action.is_empty() {
            report.action = "noop".into();
        }

        // 记录状态
        sync_st.last_run = Some(Utc::now().to_rfc3339());
        sync_st.last_direction = Some(report.action.clone());
        sync_st.last_action = Some(report.message.clone());
        sync_st.last_error = None;
        Self::save_state(&state, &sync_st);
        Ok(serde_json::to_value(report).unwrap_or(Value::Null))
    }

    /// 同步配置文件（services.json）：内容比较 + LWW 冲突处理
    async fn sync_config(
        client: &WebdavClient,
        state: &AppState,
        remote_path: &str,
        sync_st: &mut SyncState,
    ) -> Result<SyncReport, RpcError> {
        let local_bytes = std::fs::read(state.dirs.services_json())
            .map_err(|e| RpcError::new(ErrorCode::Internal, format!("读取本地配置失败: {e}")))?;
        let local_sha = Self::sha256_hex(&local_bytes);

        match client
            .propfind(remote_path)
            .await
            .map_err(|e| Self::map_webdav_err(&e))?
        {
            // 远端不存在 → 首次上传
            None => {
                let etag = client
                    .put(remote_path, &local_bytes, None)
                    .await
                    .map_err(|e| Self::map_webdav_err(&e))?;
                sync_st.last_synced_etag = Some(etag);
                sync_st.last_synced_local_sha = Some(local_sha);
                Ok(SyncReport {
                    action: "upload".into(),
                    message: "配置：远端无配置，已上传本地快照".into(),
                    conflict_file: None,
                    ..Default::default()
                })
            }
            Some(_) => {
                // 远端存在：直接 GET 全量内容做变更检测（不依赖 ETag，兼容坚果云）
                let (remote_bytes, remote_etag) = match client.get(remote_path).await {
                    Ok(r) => r,
                    Err(WebdavError::NotFound(_)) => {
                        let etag = client
                            .put(remote_path, &local_bytes, None)
                            .await
                            .map_err(|e| Self::map_webdav_err(&e))?;
                        sync_st.last_synced_etag = Some(etag);
                        sync_st.last_synced_local_sha = Some(local_sha);
                        return Ok(SyncReport {
                            action: "upload".into(),
                            message: "配置：远端无配置，已上传本地快照".into(),
                            conflict_file: None,
                            ..Default::default()
                        });
                    }
                    Err(e) => return Err(Self::map_webdav_err(&e)),
                };

                if remote_bytes == local_bytes {
                    sync_st.last_synced_etag = remote_etag;
                    sync_st.last_synced_local_sha = Some(local_sha);
                    Ok(SyncReport {
                        action: "noop".into(),
                        message: "配置：本地与远端一致".into(),
                        conflict_file: None,
                        ..Default::default()
                    })
                } else if sync_st.last_synced_local_sha.as_deref() == Some(local_sha.as_str()) {
                    let text = String::from_utf8_lossy(&remote_bytes).to_string();
                    let cfg = ServicesConfig::from_json(&text).map_err(|e| {
                        RpcError::new(ErrorCode::Validation, format!("远端配置校验失败: {e}"))
                    })?;
                    state
                        .save_services(cfg)
                        .await
                        .map_err(|e| RpcError::new(ErrorCode::Validation, e))?;
                    sync_st.last_synced_etag = remote_etag;
                    sync_st.last_synced_local_sha = Some(Self::sha256_hex(&remote_bytes));
                    Ok(SyncReport {
                        action: "download".into(),
                        message: "配置：已应用远端配置".into(),
                        conflict_file: None,
                        ..Default::default()
                    })
                } else {
                    // 双方都改 → LWW：远端另存为冲突文件，上传本地
                    let ts = Utc::now().format("%Y%m%d-%H%M%S");
                    let conflict_name = format!("services.conflict.{ts}.json");
                    let conflict_path = state.dirs.cache.join(&conflict_name);
                    std::fs::write(&conflict_path, &remote_bytes).map_err(|e| {
                        RpcError::new(ErrorCode::Internal, format!("写入冲突文件失败: {e}"))
                    })?;
                    let etag = client
                        .put(remote_path, &local_bytes, None)
                        .await
                        .map_err(|e| Self::map_webdav_err(&e))?;
                    sync_st.last_synced_etag = Some(etag);
                    sync_st.last_synced_local_sha = Some(local_sha);
                    Ok(SyncReport {
                        action: "conflict_lww".into(),
                        message: "配置：检测到双向修改，已保留本地版本，远端另存为冲突文件".into(),
                        conflict_file: Some(conflict_path.display().to_string()),
                        ..Default::default()
                    })
                }
            }
        }
    }

    /// 递归同步 cli 目录：本地 ↔ 远端（双向，仅新增/变更，不删除）。
    /// 异步递归通过 Box::pin 包装规避 E0733。
    ///
    /// - 本地有远端无 / 大小不同 → 上传
    /// - 远端有本地无 → 下载（目录递归创建）
    /// - 大小一致 → 跳过
    async fn sync_cli_tree(
        client: &WebdavClient,
        remote_root: &str,
        rel: &str,
        local_dir: &std::path::Path,
    ) -> Result<CliSyncStats, RpcError> {
        let mut stats = CliSyncStats::default();
        let remote_dir = if rel.is_empty() {
            remote_root.to_string()
        } else {
            format!("{remote_root}/{rel}")
        };

        // 远端单层条目（目录不存在时 list_dir 返回空列表）
        let remote_entries = client
            .list_dir(&remote_dir)
            .await
            .map_err(|e| Self::map_webdav_err(&e))?;
        let remote_map: std::collections::HashMap<&str, &cli_companion_webdav::RemoteEntry> =
            remote_entries
                .iter()
                .map(|e| (e.name.as_str(), e))
                .collect();

        // 本地条目（目录不存在时跳过，视为空）
        let local_entries: Vec<std::path::PathBuf> = match std::fs::read_dir(local_dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        };

        // 本地 → 远端
        for path in &local_entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let remote_file = format!("{remote_dir}/{name}");
            if path.is_dir() {
                // 确保远端子目录存在
                let _ = client.mkcol(&remote_file).await;
                let sub_stats = Box::pin(Self::sync_cli_tree(
                    client,
                    remote_root,
                    &format!("{rel}/{name}"),
                    path,
                ))
                .await?;
                stats.uploaded += sub_stats.uploaded;
                stats.downloaded += sub_stats.downloaded;
                stats.skipped += sub_stats.skipped;
            } else if path.is_file() {
                let local_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let remote = remote_map.get(name.as_str());
                let need_upload = match remote {
                    None => true,
                    Some(r) => r.size != Some(local_size) || r.size.is_none(),
                };
                if need_upload {
                    let bytes = std::fs::read(path).map_err(|e| {
                        RpcError::new(ErrorCode::Internal, format!("读取本地文件失败: {e}"))
                    })?;
                    client
                        .put(&remote_file, &bytes, None)
                        .await
                        .map_err(|e| Self::map_webdav_err(&e))?;
                    stats.uploaded += 1;
                } else {
                    stats.skipped += 1;
                }
            }
        }

        // 远端 → 本地（仅远端有）
        for remote in &remote_entries {
            let local_path = local_dir.join(&remote.name);
            if local_path.exists() {
                continue;
            }
            let remote_file = format!("{remote_dir}/{}", remote.name);
            if remote.is_dir {
                let _ = std::fs::create_dir_all(&local_path);
                let sub_stats = Box::pin(Self::sync_cli_tree(
                    client,
                    remote_root,
                    &format!("{rel}/{}", remote.name),
                    &local_path,
                ))
                .await?;
                stats.uploaded += sub_stats.uploaded;
                stats.downloaded += sub_stats.downloaded;
                stats.skipped += sub_stats.skipped;
            } else {
                let (bytes, _) = client
                    .get(&remote_file)
                    .await
                    .map_err(|e| Self::map_webdav_err(&e))?;
                std::fs::write(&local_path, &bytes).map_err(|e| {
                    RpcError::new(ErrorCode::Internal, format!("写入本地文件失败: {e}"))
                })?;
                stats.downloaded += 1;
            }
        }
        Ok(stats)
    }

    /// 同步状态查询
    pub async fn status(&self, state: &AppState) -> Result<Value, RpcError> {
        let app = state.app().await;
        let st = Self::load_state(state);
        Ok(json!({
            "enabled": app.webdav.enabled,
            "url": app.webdav.url,
            "username": app.webdav.username,
            "remote_dir": app.webdav.remote_dir,
            "sync_interval_minutes": app.webdav.sync_interval_minutes,
            "password_set": state.config.lock().await.secrets.webdav_password_dpapi.is_some(),
            "state": st,
        }))
    }

    /// 测试连接（不修改任何数据）
    pub async fn test_connection(&self, state: &AppState) -> Result<Value, RpcError> {
        let (client, settings) = Self::build_client(state).await?;
        let remote_path = Self::remote_file_path(&settings);
        match client
            .propfind(&remote_path)
            .await
            .map_err(|e| Self::map_webdav_err(&e))?
        {
            Some(_) => Ok(json!({"ok": true, "message": "连接成功，远端配置存在"})),
            None => Ok(json!({"ok": true, "message": "连接成功，远端暂无配置"})),
        }
    }
}

/// 周期同步调度：仅在 enabled 时按间隔运行（默认 15 分钟）
pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        loop {
            let app = state.app().await;
            let interval =
                std::time::Duration::from_secs(app.webdav.sync_interval_minutes.max(1) as u64 * 60);
            tokio::time::sleep(interval).await;
            let app = state.app().await;
            if !app.webdav.enabled || app.webdav.url.is_empty() {
                continue;
            }
            tracing::info!("周期同步开始");
            if let Err(e) = state.sync.clone().run(state.clone()).await {
                tracing::warn!("周期同步失败: {e}");
            }
        }
    });
}
