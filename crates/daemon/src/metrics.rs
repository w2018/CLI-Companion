//! 服务资源指标采样（CPU / 内存 / GPU / 磁盘 / 网络）
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

/// 进程树 CPU 占用率（0-100，按逻辑核数归一化）
///
/// - 采样间隔过短（<0.5s）返回 None，调用方沿用上一次结果；
/// - `delta_100ns` 为进程树内各进程 CPU 累计时间（100ns 单位）的差分之和，
///   由采集侧按 PID 逐个计算：新加入的子进程本窗口计 0，计数回落的 PID 跳过；
/// - 多线程进程可占用多核，归一化后仍 ≤100。
pub fn compute_tree_cpu_percent(delta_100ns: u64, elapsed: Duration, cores: u64) -> Option<f32> {
    if elapsed.as_secs_f64() < 0.5 {
        return None;
    }
    let delta_secs = delta_100ns as f64 / 10_000_000.0;
    let pct = delta_secs / elapsed.as_secs_f64() / cores.max(1) as f64 * 100.0;
    Some((pct as f32).clamp(0.0, 100.0))
}

/// 累计计数差分 → 每秒速率（字节/秒），磁盘 I/O 与网络流量共用
///
/// - 采样间隔过短（<0.5s）返回 None，调用方沿用上一次结果；
/// - 计数回落（now < prev：TCP 连接更替 / PID 复用）饱和为 0 而非丢弃——
///   连接有生命周期，累计值回落是常态，语义上即"本窗口 0 速率"。
pub fn compute_rate_per_sec(prev: u64, now: u64, elapsed: Duration) -> Option<u64> {
    if elapsed.as_secs_f64() < 0.5 {
        return None;
    }
    let delta = now.saturating_sub(prev) as f64;
    Some((delta / elapsed.as_secs_f64()).round() as u64)
}

/// 内存占系统物理内存百分比（0-100）；系统总量未知（0）时返回 None
pub fn compute_mem_percent(bytes: u64, total: u64) -> Option<f32> {
    if total == 0 {
        return None;
    }
    Some(((bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 半核占用得50() {
        // 1 秒窗口内消耗 0.5 核的 CPU 时间（500 万个 100ns），8 逻辑核
        let pct = compute_tree_cpu_percent(5_000_000, Duration::from_secs(1), 8).unwrap();
        assert!((pct - 6.25).abs() < 0.01); // 0.5 核 / 8 核 = 6.25%
    }

    #[test]
    fn 单核进程满载接近100() {
        // 2 秒窗口、2 逻辑核：满载 = 2 核 × 2 秒 = 4×10^7 个 100ns → 100%
        let pct = compute_tree_cpu_percent(40_000_000, Duration::from_secs(2), 2).unwrap();
        assert!((pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn 间隔过短不可信() {
        assert!(compute_tree_cpu_percent(5_000_000, Duration::from_millis(100), 8).is_none());
    }

    #[test]
    fn 空闲进程返回0() {
        // 完全无 CPU 消耗（差分为 0）：0%
        assert_eq!(
            compute_tree_cpu_percent(0, Duration::from_secs(2), 8),
            Some(0.0)
        );
        // 微小消耗：接近 0 但非 0
        let pct = compute_tree_cpu_percent(1_000, Duration::from_secs(2), 8).unwrap();
        assert!(pct > 0.0 && pct < 0.01);
    }

    #[test]
    fn 速率正常差分() {
        // 2 秒窗口内累计传输 1 MB → 512 KB/s（四舍五入）
        let r = compute_rate_per_sec(0, 1024 * 1024, Duration::from_secs(2)).unwrap();
        assert_eq!(r, 512 * 1024);
        // 微小速率不为 0
        let r = compute_rate_per_sec(0, 3, Duration::from_secs(2)).unwrap();
        assert_eq!(r, 2);
    }

    #[test]
    fn 速率计数回落饱和为0() {
        // TCP 连接更替导致累计值回落：本窗口 0 速率（而非沿用旧值或丢弃）
        assert_eq!(
            compute_rate_per_sec(2_000, 100, Duration::from_secs(2)),
            Some(0)
        );
        // 零增量：0 速率
        assert_eq!(
            compute_rate_per_sec(1_000, 1_000, Duration::from_secs(2)),
            Some(0)
        );
    }

    #[test]
    fn 速率间隔过短不可信() {
        assert!(compute_rate_per_sec(0, 5_000, Duration::from_millis(100)).is_none());
    }

    #[test]
    fn 内存百分比() {
        assert_eq!(compute_mem_percent(512, 1024), Some(50.0));
        assert_eq!(compute_mem_percent(0, 1024), Some(0.0));
        assert_eq!(compute_mem_percent(1024, 0), None); // 总量未知
        let pct = compute_mem_percent(2048, 1024).unwrap();
        assert_eq!(pct, 100.0); // 超界钳制
    }

    #[test]
    fn 内存告警判定() {
        use std::time::{Duration, Instant};
        // 未配置阈值 / 无采样：不告警
        assert!(!mem_alert_triggered(None, Some(9_999_999_999), None));
        assert!(!mem_alert_triggered(Some(512), None, None));
        // 未超阈值：不告警
        assert!(!mem_alert_triggered(
            Some(512),
            Some(512 * 1024 * 1024),
            None
        ));
        // 超阈值首次：告警
        let over = 513 * 1024 * 1024;
        assert!(mem_alert_triggered(Some(512), Some(over), None));
        // 冷却期内：不重复告警
        assert!(!mem_alert_triggered(
            Some(512),
            Some(over),
            Some(Instant::now())
        ));
        // 冷却期已过：再次告警
        let past = Instant::now() - Duration::from_secs(601);
        assert!(mem_alert_triggered(Some(512), Some(over), Some(past)));
    }
}
