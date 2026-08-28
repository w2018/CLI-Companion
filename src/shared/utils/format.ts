// 时间与时长格式化工具

/** ISO 时间 → 本地化显示（如 2026/8/28 14:30:05）；无效值返回 "—" */
export function formatDateTime(iso?: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString("zh-CN", { hour12: false });
}

/** 运行时长：从 started_at 到现在的人性化显示（如 2小时5分 / 3分42秒） */
export function formatDuration(startedAt?: string | null, now = Date.now()): string {
  if (!startedAt) return "—";
  const start = new Date(startedAt).getTime();
  if (Number.isNaN(start)) return "—";
  const secs = Math.max(0, Math.floor((now - start) / 1000));
  if (secs < 60) return `${secs}秒`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}分${secs % 60}秒`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}小时${mins % 60}分`;
  const days = Math.floor(hours / 24);
  return `${days}天${hours % 24}小时`;
}
