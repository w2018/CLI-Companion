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
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncReport {
    pub action: String,
    pub message: String,
    pub conflict_file: Option<String>,
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
            _ => ErrorCode::WebdavProtocol,
        };
        RpcError::new(code, e.to_string())
    }

    /// 执行一次同步（手动或周期调度共用；busy 锁防并发）
    pub async fn run(self: Arc<Self>, state: AppState) -> Result<Value, RpcError> {
        let _guard = self.busy.lock().await;
        let (client, settings) = Self::build_client(&state).await?;
        let remote_path = Self::remote_file_path(&settings);

        // 确保远端目录存在（best effort）
        let dir = settings.remote_dir.trim_matches('/');
        if let Err(e) = client.mkcol(dir).await {
            tracing::debug!("mkcol 目录（可能已存在）: {e}");
        }

        // 本地快照
        let local_bytes = std::fs::read(state.dirs.services_json())
            .map_err(|e| RpcError::new(ErrorCode::Internal, format!("读取本地配置失败: {e}")))?;
        let local_sha = Self::sha256_hex(&local_bytes);
        let mut sync_st = Self::load_state(&state);

        let result: Result<SyncReport, RpcError> = async {
            match client
                .propfind(&remote_path)
                .await
                .map_err(|e| Self::map_webdav_err(&e))?
            {
                // 远端不存在 → 首次上传
                None => {
                    let etag = client
                        .put(&remote_path, &local_bytes, None)
                        .await
                        .map_err(|e| Self::map_webdav_err(&e))?;
                    sync_st.last_synced_etag = Some(etag);
                    sync_st.last_synced_local_sha = Some(local_sha);
                    Ok(SyncReport {
                        action: "upload".into(),
                        message: "远端无配置，已上传本地快照".into(),
                        conflict_file: None,
                    })
                }
                Some(_) => {
                    // 远端存在：直接 GET 全量内容做变更检测。
                    // 不依赖 ETag 做变更检测 —— 各服务器 ETag 支持参差（坚果云
                    // PROPFIND 的 ETag 在响应体），内容比较最可靠；配置文件小，开销可接受。
                    let (remote_bytes, remote_etag) = match client.get(&remote_path).await {
                        Ok(r) => r,
                        Err(WebdavError::NotFound(_)) => {
                            // 探测存在但 GET 404（竞态）→ 视为首轮上传
                            let etag = client
                                .put(&remote_path, &local_bytes, None)
                                .await
                                .map_err(|e| Self::map_webdav_err(&e))?;
                            sync_st.last_synced_etag = Some(etag);
                            sync_st.last_synced_local_sha = Some(local_sha);
                            return Ok(SyncReport {
                                action: "upload".into(),
                                message: "远端无配置，已上传本地快照".into(),
                                conflict_file: None,
                            });
                        }
                        Err(e) => return Err(Self::map_webdav_err(&e)),
                    };

                    if remote_bytes == local_bytes {
                        // 内容一致：仅刷新基线
                        sync_st.last_synced_etag = remote_etag;
                        sync_st.last_synced_local_sha = Some(local_sha);
                        Ok(SyncReport {
                            action: "noop".into(),
                            message: "本地与远端一致".into(),
                            conflict_file: None,
                        })
                    } else if sync_st.last_synced_local_sha.as_deref() == Some(local_sha.as_str()) {
                        // 本地自上次同步后未改 → 应用远端（先校验 schema）
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
                            message: "已应用远端配置".into(),
                            conflict_file: None,
                        })
                    } else {
                        // 双方都改了 → LWW：远端另存为冲突文件，上传本地
                        let ts = Utc::now().format("%Y%m%d-%H%M%S");
                        let conflict_name = format!("services.conflict.{ts}.json");
                        let conflict_path = state.dirs.cache.join(&conflict_name);
                        std::fs::write(&conflict_path, &remote_bytes).map_err(|e| {
                            RpcError::new(ErrorCode::Internal, format!("写入冲突文件失败: {e}"))
                        })?;
                        let etag = client
                            .put(&remote_path, &local_bytes, None)
                            .await
                            .map_err(|e| Self::map_webdav_err(&e))?;
                        sync_st.last_synced_etag = Some(etag);
                        sync_st.last_synced_local_sha = Some(local_sha);
                        Ok(SyncReport {
                            action: "conflict_lww".into(),
                            message: "检测到双向修改：已保留本地版本，远端版本另存为冲突文件"
                                .into(),
                            conflict_file: Some(conflict_path.display().to_string()),
                        })
                    }
                }
            }
        }
        .await;

        // 记录状态
        match &result {
            Ok(report) => {
                sync_st.last_run = Some(Utc::now().to_rfc3339());
                sync_st.last_direction = Some(report.action.clone());
                sync_st.last_action = Some(report.message.clone());
                sync_st.last_error = None;
            }
            Err(e) => {
                sync_st.last_run = Some(Utc::now().to_rfc3339());
                sync_st.last_error = Some(e.message.clone());
            }
        }
        Self::save_state(&state, &sync_st);
        result.map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
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
