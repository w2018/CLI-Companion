//! WebDAV 客户端（开发文档 §8）
//!
//! 仅实现同步所需的子集：PROPFIND（探测）、GET、PUT（条件提交 If-Match）、MKCOL。
//! 业务层（daemon sync 模块）负责状态机、冲突处理与重试。

use reqwest::{Client, StatusCode};
use std::time::Duration;
use thiserror::Error;
use url::Url;

/// WebDAV 客户端错误
#[derive(Debug, Error)]
pub enum WebdavError {
    #[error("WebDAV 认证失败（401/403）")]
    Auth,
    #[error("资源不存在（404）: {0}")]
    NotFound(String),
    #[error("前置条件失败（412），远端已被其他端修改")]
    PreconditionFailed,
    #[error("资源被锁定（423）")]
    Locked,
    #[error("请求过于频繁（429）")]
    TooManyRequests,
    #[error("服务器错误（{status}）: {url}")]
    Server { status: u16, url: String },
    #[error("网络错误: {0}")]
    Network(#[from] reqwest::Error),
    #[error("URL 无效: {0}")]
    InvalidUrl(String),
}

/// 单文件元信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: Option<u64>,
}

/// 远端目录项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// 条目名（不含目录前缀）
    pub name: String,
    pub is_dir: bool,
    pub etag: Option<String>,
    pub size: Option<u64>,
}

/// 解析 PROPFIND multistatus 响应体（Depth:1 列目录）
///
/// 提取每个 <response> 的 href、是否集合、getetag、getcontentlength。
/// 使用 quick-xml 按 local_name（忽略 namespace 前缀）匹配。
/// 去除 namespace 前缀（"D:response" → "response"）
fn strip_ns(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn parse_multistatus(body: &str) -> Vec<RemoteEntry> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(body);
    reader.trim_text(true);
    let mut entries: Vec<RemoteEntry> = Vec::new();
    let mut buf = Vec::new();
    let mut cur: Option<RemoteEntry> = None;
    let mut cur_field: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let qn = e.local_name();
                let n = strip_ns(qn.as_ref());
                match n {
                    b"response" => {
                        cur = Some(RemoteEntry {
                            name: String::new(),
                            is_dir: false,
                            etag: None,
                            size: None,
                        })
                    }
                    b"href" if cur.is_some() => cur_field = Some("href"),
                    b"getetag" if cur.is_some() => cur_field = Some("etag"),
                    b"getcontentlength" if cur.is_some() => cur_field = Some("size"),
                    b"collection" if cur.is_some() => {
                        if let Some(c) = &mut cur {
                            c.is_dir = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let qn = e.local_name();
                let n = strip_ns(qn.as_ref());
                if n == b"collection" {
                    if let Some(c) = &mut cur {
                        c.is_dir = true;
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(c) = &mut cur {
                    match cur_field {
                        Some("href") => c.name = t.unescape().unwrap_or_default().to_string(),
                        Some("etag") => c.etag = Some(t.unescape().unwrap_or_default().to_string()),
                        Some("size") => {
                            c.size = t.unescape().unwrap_or_default().trim().parse::<u64>().ok()
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let qn = e.local_name();
                let n = strip_ns(qn.as_ref());
                match n {
                    b"response" => {
                        if let Some(c) = cur.take() {
                            entries.push(c);
                        }
                        cur_field = None;
                    }
                    b"href" | b"getetag" | b"getcontentlength" => cur_field = None,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    entries
}

/// 从 PROPFIND 响应体 XML 提取 getetag 值
///
/// 兼容带命名空间前缀（<D:getetag>）与不带（<getetag>）两种写法，
/// 并解码 XML 实体（坚果云返回 &quot;...&quot; 包裹的 ETag）。
fn extract_etag_from_xml(body: &str) -> Option<String> {
    let idx = body.find("getetag")?;
    let after = &body[idx + "getetag".len()..];
    // 自闭合标签 <getetag/>：无值
    let trimmed_start = after.trim_start();
    if trimmed_start.starts_with("/>") {
        return None;
    }
    let start = after.find('>')? + 1;
    let rest = &after[start..];
    let end = rest.find("</")?;
    let mut value = rest[..end].trim().to_string();
    if value.is_empty() {
        return None;
    }
    value = value
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    Some(value)
}

/// WebDAV 客户端
pub struct WebdavClient {
    client: Client,
    base: Url,
    username: String,
    password: String,
}

impl WebdavClient {
    /// 构造客户端；url 形如 https://dav.jianguoyun.com/dav/
    ///
    /// 自动补齐尾随斜杠：Url::join 在 base 无尾斜杠时会替换最后一段路径，
    /// 导致所有请求 404（坚果云等服务的常见坑）。
    pub fn new(
        base: &str,
        username: String,
        password: String,
        verify_tls: bool,
    ) -> Result<Self, WebdavError> {
        let trimmed = base.trim();
        let normalized = if trimmed.ends_with('/') {
            trimmed.to_string()
        } else {
            format!("{trimmed}/")
        };
        let base =
            Url::parse(&normalized).map_err(|_| WebdavError::InvalidUrl(base.to_string()))?;
        let client = Client::builder()
            .danger_accept_invalid_certs(!verify_tls)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            client,
            base,
            username,
            password,
        })
    }

    fn url_for(&self, path: &str) -> Result<Url, WebdavError> {
        self.base
            .join(path.trim_start_matches('/'))
            .map_err(|_| WebdavError::InvalidUrl(path.into()))
    }

    fn auth(&self) -> Option<reqwest::header::HeaderValue> {
        use reqwest::header::HeaderMap;
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            {
                let cred = format!("{}:{}", self.username, self.password);
                use std::io::Write as _;
                let mut b64 = String::new();
                // 手写 base64（标准字母表），避免引入额外依赖
                let bytes = cred.as_bytes();
                const TABLE: &[u8] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                for chunk in bytes.chunks(3) {
                    let b = [
                        chunk[0],
                        *chunk.get(1).unwrap_or(&0),
                        *chunk.get(2).unwrap_or(&0),
                    ];
                    let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                    b64.push(TABLE[(n >> 18) as usize & 63] as char);
                    b64.push(TABLE[(n >> 12) as usize & 63] as char);
                    b64.push(if chunk.len() > 1 {
                        TABLE[(n >> 6) as usize & 63] as char
                    } else {
                        '='
                    });
                    b64.push(if chunk.len() > 2 {
                        TABLE[n as usize & 63] as char
                    } else {
                        '='
                    });
                }
                let _ = std::io::sink().write_all(b"");
                reqwest::header::HeaderValue::from_str(&format!("Basic {b64}"))
            }
            .ok()?,
        );
        headers.get(reqwest::header::AUTHORIZATION).cloned()
    }

    /// PROPFIND 探测文件是否存在并取元信息（207 存在 / 404 不存在）
    ///
    /// 注意：WebDAV 规范中 PROPFIND 的 ETag 在响应体 XML（<D:getetag>），
    /// 不在响应头 —— 必须解析响应体。
    pub async fn propfind(&self, path: &str) -> Result<Option<RemoteFile>, WebdavError> {
        let url = self.url_for(path)?;
        let req = self
            .client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                url.clone(),
            )
            .header("Depth", "0")
            .header(reqwest::header::CONTENT_TYPE, "application/xml");
        let req = match self.auth() {
            Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
            None => req,
        };
        let resp = req
            .body(r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:getetag/><D:getlastmodified/><D:getcontentlength/></D:prop></D:propfind>"#)
            .send()
            .await?;
        match resp.status() {
            StatusCode::MULTI_STATUS => {
                // 先取响应头 ETag，再读响应体（text() 会消耗 resp），
                // 响应头没有时解析响应体 XML 的 <D:getetag>
                let header_etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                let body = resp.text().await.unwrap_or_default();
                let etag = header_etag.or_else(|| extract_etag_from_xml(&body));
                Ok(Some(RemoteFile {
                    etag,
                    last_modified: None,
                    size: None,
                }))
            }
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(WebdavError::Auth),
            s if s.is_server_error() => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
            s => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
        }
    }

    /// 列出远端目录（Depth:1 单层），返回目录项列表；目录不存在返回空列表
    pub async fn list_dir(&self, path: &str) -> Result<Vec<RemoteEntry>, WebdavError> {
        let url = self.url_for(path)?;
        let req = self
            .client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                url.clone(),
            )
            .header("Depth", "1")
            .header(reqwest::header::CONTENT_TYPE, "application/xml");
        let req = match self.auth() {
            Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
            None => req,
        };
        let resp = req
            .body(r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getetag/><D:getcontentlength/></D:prop></D:propfind>"#)
            .send()
            .await?;
        match resp.status() {
            StatusCode::MULTI_STATUS => {
                let body = resp.text().await.unwrap_or_default();
                let entries = parse_multistatus(&body);
                // 过滤目录自身（href 以请求路径结尾的项），并提取子项名称
                let base = path.trim_end_matches('/').to_string();
                Ok(entries
                    .into_iter()
                    .filter(|e| {
                        let t = e.name.trim_end_matches('/');
                        !t.is_empty()
                            && !t.eq_ignore_ascii_case(&base)
                            && !t.ends_with(&format!("/{base}"))
                    })
                    .map(|mut e| {
                        let t = e.name.trim_end_matches('/');
                        e.name = t.rsplit('/').next().unwrap_or("").to_string();
                        e
                    })
                    .filter(|e| !e.name.is_empty())
                    .collect())
            }
            StatusCode::NOT_FOUND => Ok(Vec::new()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(WebdavError::Auth),
            s if s.is_server_error() => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
            s => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
        }
    }

    /// 创建远端集合目录
    pub async fn mkcol(&self, path: &str) -> Result<(), WebdavError> {
        let url = self.url_for(path)?;
        let method = reqwest::Method::from_bytes(b"MKCOL").unwrap();
        let req = self.client.request(method, url.clone());
        let req = match self.auth() {
            Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
            None => req,
        };
        let resp = req.send().await?;
        match resp.status() {
            // 201 创建成功；405 已存在；200/204 部分服务器返回
            // 409 Conflict：多数服务器（含坚果云）在目录已存在时返回 409，
            // 视为"目录已存在"，幂等成功
            StatusCode::CREATED | StatusCode::OK | StatusCode::NO_CONTENT => Ok(()),
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::CONFLICT => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(WebdavError::Auth),
            s => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
        }
    }

    /// GET 下载文件，返回（内容, ETag）
    pub async fn get(&self, path: &str) -> Result<(Vec<u8>, Option<String>), WebdavError> {
        let url = self.url_for(path)?;
        let req = self.client.get(url.clone());
        let req = match self.auth() {
            Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
            None => req,
        };
        let resp = req.send().await?;
        match resp.status() {
            StatusCode::OK => {
                let etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                Ok((resp.bytes().await?.to_vec(), etag))
            }
            StatusCode::NOT_FOUND => Err(WebdavError::NotFound(path.into())),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(WebdavError::Auth),
            s => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
        }
    }

    /// PUT 上传；指定 if_match 时附加 If-Match 条件头
    pub async fn put(
        &self,
        path: &str,
        data: &[u8],
        if_match: Option<&str>,
    ) -> Result<String, WebdavError> {
        let url = self.url_for(path)?;
        let mut req = self.client.put(url.clone());
        if let Some(etag) = if_match {
            req = req.header(reqwest::header::IF_MATCH, etag);
        }
        let req = match self.auth() {
            Some(h) => req.header(reqwest::header::AUTHORIZATION, h),
            None => req,
        };
        let resp = req.body(data.to_vec()).send().await?;
        match resp.status() {
            StatusCode::CREATED | StatusCode::OK | StatusCode::NO_CONTENT => {
                let etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                Ok(etag.unwrap_or_default())
            }
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => {
                // 412 或 409：前置条件冲突（If-Match 不匹配 / 远端已被修改），
                // 语义等价，走统一的"远端已变更"处理路径
                Err(WebdavError::PreconditionFailed)
            }
            StatusCode::LOCKED => Err(WebdavError::Locked),
            StatusCode::TOO_MANY_REQUESTS => Err(WebdavError::TooManyRequests),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(WebdavError::Auth),
            s => Err(WebdavError::Server {
                status: s.as_u16(),
                url: url.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url自动补尾斜杠() {
        // 无尾斜杠的 URL（坚果云常见写法）必须被规范化，否则 join 替换末段
        let c = WebdavClient::new(
            "https://dav.jianguoyun.com/dav",
            "u".into(),
            "p".into(),
            true,
        )
        .unwrap();
        let url = c.url_for("cli-companion/services.json").unwrap();
        assert_eq!(
            url.as_str(),
            "https://dav.jianguoyun.com/dav/cli-companion/services.json"
        );
    }

    #[test]
    fn 已有尾斜杠不重复添加() {
        let c = WebdavClient::new(
            "https://dav.jianguoyun.com/dav/",
            "u".into(),
            "p".into(),
            true,
        )
        .unwrap();
        let url = c.url_for("dir/file.json").unwrap();
        assert_eq!(url.as_str(), "https://dav.jianguoyun.com/dav/dir/file.json");
    }

    #[test]
    fn 从xml提取getetag_带命名空间与实体() {
        let body = r#"<D:multistatus><D:response><D:href>/dav/f.json</D:href><D:propstat><D:prop><D:getetag>&quot;abc-123&quot;</D:getetag><D:getlastmodified>Mon, 01 Jan 2026 00:00:00 GMT</D:getlastmodified></D:prop></D:propstat></D:response></D:multistatus>"#;
        assert_eq!(extract_etag_from_xml(body), Some("\"abc-123\"".to_string()));
    }

    #[test]
    fn 从xml提取getetag_无命名空间() {
        let body = r#"<multistatus><response><propstat><prop><getetag>plain-etag</getetag></prop></propstat></response></multistatus>"#;
        assert_eq!(extract_etag_from_xml(body), Some("plain-etag".to_string()));
    }

    #[test]
    fn 自闭合getetag返回none() {
        let body = r#"<prop><D:getetag/><D:getlastmodified>x</D:getlastmodified></prop>"#;
        assert_eq!(extract_etag_from_xml(body), None);
    }
}
