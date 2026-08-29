//! 服务资源指标采样（CPU / 内存）
//!
//! 采样动作在 actor 的运行循环内执行（持有子进程与 Job 句柄），
//! 本模块提供采样周期常量与纯计算函数，便于单元测试。

use std::time::Duration;

/// 指标采样周期：2 秒（CPU% 计算窗口）
pub const SAMPLE_INTERVAL_MS: u64 = 2_000;

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
}
