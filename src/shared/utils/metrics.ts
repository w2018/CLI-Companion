// 资源指标展示辅助：速率格式化 + 按占用分档配色（v2.4.0）
import { formatBytes } from "./format";

/** 占用档位：正常 / 偏高 / 过载 */
export type LoadLevel = "ok" | "warn" | "err";

/** 档位 → 文本色类（ok/warn/err 语义 token，明暗主题自动适配） */
export const levelCls: Record<LoadLevel, string> = {
  ok: "text-ok",
  warn: "text-warn",
  err: "text-err",
};

/** 百分比型占用分档（CPU/GPU 默认 60/85，内存建议传 70/90） */
export function percentLevel(v: number, warnAt = 60, errAt = 85): LoadLevel {
  if (!Number.isFinite(v)) return "ok";
  if (v >= errAt) return "err";
  if (v >= warnAt) return "warn";
  return "ok";
}

/** 速率型占用分档（字节/秒）：磁盘建议 1MB/20MB，网络建议 512KB/5MB */
export function rateLevel(
  bytesPerSec: number,
  warnBytes = 1024 * 1024,
  errBytes = 20 * 1024 * 1024,
): LoadLevel {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return "ok";
  if (bytesPerSec >= errBytes) return "err";
  if (bytesPerSec >= warnBytes) return "warn";
  return "ok";
}

/** 速率格式化：字节/秒 → "1.2 MB/s"；无效值返回 "—" */
export function formatRate(bytesPerSec?: number | null): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec) || bytesPerSec < 0) return "—";
  return `${formatBytes(bytesPerSec)}/s`;
}

/** 读写合并速率（字节/秒）；双方均无效时返回 null */
export function combinedRate(a?: number | null, b?: number | null): number | null {
  const va = a != null && Number.isFinite(a) && a >= 0 ? a : null;
  const vb = b != null && Number.isFinite(b) && b >= 0 ? b : null;
  if (va == null && vb == null) return null;
  return (va ?? 0) + (vb ?? 0);
}
