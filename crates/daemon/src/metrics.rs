//! 服务资源指标采样（CPU / 内存）
//!
//! 采样动作在 actor 的运行循环内执行（持有子进程与 Job 句柄），
//! 本模块提供采样周期常量与纯计算函数，便于单元测试。

use std::time::Duration;

/// 指标采样周期：2 秒（CPU% 计算窗口）
pub const SAMPLE_INTERVAL_MS: u64 = 2_000;

/// 内存告警的最小重复间隔（同一服务两条告警之间）
pub const MEM_ALERT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(600);

/// 内存告警是否应当触发
///
/// 条件：配置了阈值 && 有采样值 && 超过阈值 && 距上次告警 ≥ 冷却期。
pub fn mem_alert_triggered(
    alert_mb: Option<u32>,
    mem_bytes: Option<u64>,
    last_alert: Option<std::time::Instant>,
) -> bool {
    let Some(limit_mb) = alert_mb else {
        return false;
    };
    let Some(bytes) = mem_bytes else {
        return false;
    };
    let limit_bytes = (limit_mb as u64) * 1024 * 1024;
    // 仅"严格超过"阈值才告警（等于阈值视为正常）
    if bytes <= limit_bytes {
        return false;
    }
    match last_alert {
        Some(t) => t.elapsed() >= MEM_ALERT_COOLDOWN,
        None => true,
    }
}

/// CPU 占用率（0-100，按逻辑核数归一化）
///
/// - 采样间隔过短（<0.5s）返回 None，调用方沿用上一次结果；
/// - now < prev 视为进程重建（PID 复用 / 句柄重新打开），丢弃本窗口；
/// - now == prev 视为真实空闲，返回 0；
/// - 多线程进程可占用多核，归一化后仍 ≤100。
pub fn compute_cpu_percent(
    prev_100ns: u64,
    now_100ns: u64,
    elapsed: Duration,
    cores: u64,
) -> Option<f32> {
    if elapsed.as_secs_f64() < 0.5 {
        return None;
    }
    if now_100ns < prev_100ns {
        return None;
    }
    let delta_secs = (now_100ns.saturating_sub(prev_100ns)) as f64 / 10_000_000.0;
    let pct = delta_secs / elapsed.as_secs_f64() / cores.max(1) as f64 * 100.0;
    Some((pct as f32).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 半核占用得50() {
        // 1 秒内消耗 0.5 核的 CPU 时间（500 万个 100ns）
        let pct = compute_cpu_percent(0, 5_000_000, Duration::from_secs(1), 8).unwrap();
        assert!((pct - 6.25).abs() < 0.01); // 0.5 核 / 8 核 = 6.25%
    }

    #[test]
    fn 单核进程满载接近100() {
        // 2 秒窗口、2 逻辑核：满载 = 2 核 × 2 秒 = 4×10^7 个 100ns → 100%
        let pct = compute_cpu_percent(0, 40_000_000, Duration::from_secs(2), 2).unwrap();
        assert!((pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn 间隔过短不可信() {
        assert!(compute_cpu_percent(0, 5_000_000, Duration::from_millis(100), 8).is_none());
    }

    #[test]
    fn 空闲进程返回0() {
        // 完全无 CPU 消耗（now == prev）：0%
        assert_eq!(
            compute_cpu_percent(500, 500, Duration::from_secs(2), 8),
            Some(0.0)
        );
        // 微小消耗：接近 0 但非 0
        let pct = compute_cpu_percent(0, 1_000, Duration::from_secs(2), 8).unwrap();
        assert!(pct > 0.0 && pct < 0.01);
    }

    #[test]
    fn 时间倒退丢弃窗口() {
        assert!(compute_cpu_percent(1_000, 999, Duration::from_secs(2), 8).is_none());
    }

    #[test]
    fn 内存告警判定() {
        use std::time::{Duration, Instant};
        // 未配置阈值 / 无采样：不告警
        assert!(!mem_alert_triggered(None, Some(9_999_999_999), None));
        assert!(!mem_alert_triggered(Some(512), None, None));
        // 未超阈值：不告警
        assert!(!mem_alert_triggered(Some(512), Some(512 * 1024 * 1024), None));
        // 超阈值首次：告警
        let over = 513 * 1024 * 1024;
        assert!(mem_alert_triggered(Some(512), Some(over), None));
        // 冷却期内：不重复告警
        assert!(!mem_alert_triggered(Some(512), Some(over), Some(Instant::now())));
        // 冷却期已过：再次告警
        let past = Instant::now() - Duration::from_secs(601);
        assert!(mem_alert_triggered(Some(512), Some(over), Some(past)));
    }
}
