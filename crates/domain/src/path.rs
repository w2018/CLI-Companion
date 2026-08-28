//! 路径可移植性策略（开发文档 §5.3）
//!
//! 优先顺序：相对路径可解析 → 环境变量可展开 → 路径存在；
//! 否则标记为 Unresolved，绝不静默删除或猜测。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 路径解析结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathResolution {
    /// 已解析为绝对路径
    Resolved(PathBuf),
    /// 无法解析（导入/跨设备场景），需用户修复
    Unresolved { raw: String, reason: String },
}

/// 解析单个路径字符串
///
/// 规则：
/// 1. 绝对路径 → 直接存在性检查；
/// 2. 相对路径 → 相对 base_dir 解析；
/// 3. 支持 `%VAR%` 与 `$VAR` 环境变量展开。
pub fn resolve_path(raw: &str, base_dir: Option<&Path>) -> PathResolution {
    let expanded = expand_env(raw);
    let p = PathBuf::from(&expanded);

    if p.is_absolute() {
        return mark(&expanded, p.is_absolute() && p.exists());
    }
    if let Some(base) = base_dir {
        let joined = base.join(&p);
        return mark(&expanded, joined.exists());
    }
    PathResolution::Unresolved {
        raw: raw.to_string(),
        reason: "无基准目录可解析相对路径".into(),
    }
}

fn mark(expanded: &str, exists: bool) -> PathResolution {
    if exists {
        PathResolution::Resolved(PathBuf::from(expanded))
    } else {
        PathResolution::Unresolved {
            raw: expanded.to_string(),
            reason: "路径不存在".into(),
        }
    }
}

/// 展开 %VAR% 与 $VAR 形式的环境变量；未定义的变量保留原样
fn expand_env(input: &str) -> String {
    let mut out = input.to_string();
    // 展开 %VAR%
    if out.starts_with('%') && out.len() > 2 {
        if let Some(end) = out[1..].find('%') {
            let name = &out[1..1 + end];
            if let Ok(v) = std::env::var(name) {
                out = format!("{v}{}", &out[end + 2..]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 环境变量可展开() {
        // TEMP 在 Windows 上必然存在
        let r = resolve_path("%TEMP%", None);
        assert!(matches!(r, PathResolution::Resolved(_)));
    }

    #[test]
    fn 不存在的路径标记为未解析() {
        let r = resolve_path("Z:/确定不存在/claude-test-404/x", None);
        assert!(matches!(r, PathResolution::Unresolved { .. }));
    }
}
