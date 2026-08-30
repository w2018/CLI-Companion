// 行内彩色资源指标：CPU · 内存 · GPU · 显存 · 磁盘 · 网络（v2.4.0）
//
// - 数值颜色随占用分档变化（ok/warn/err 语义 token，明暗主题自动适配）
// - 字段缺省（daemon 未上报/不支持）时对应项不显示
// - 磁盘/网络展示读写合并速率，悬停 title 显示读/写、收/发明细
import { Fragment } from "react";
import type { ReactElement } from "react";
import type { ServiceMetric } from "../rpc/schema";
import { formatBytes } from "../utils/format";
import {
  combinedRate,
  formatRate,
  levelCls,
  percentLevel,
  rateLevel,
} from "../utils/metrics";

/** 速率分档阈值：磁盘 1MB/s 偏高、20MB/s 过载；网络 512KB/s 偏高、5MB/s 过载 */
const DISK_WARN = 1024 * 1024;
const DISK_ERR = 20 * 1024 * 1024;
const NET_WARN = 512 * 1024;
const NET_ERR = 5 * 1024 * 1024;

function Chip({
  label,
  value,
  cls,
  title,
}: {
  label: string;
  value: string;
  cls?: string;
  title?: string;
}) {
  return (
    <span className="inline-flex items-center gap-1" title={title}>
      <span className="text-muted">{label}</span>
      <span className={cls ?? "text-content"}>{value}</span>
    </span>
  );
}

/** 服务行内的一组资源指标；metric 缺省或全部字段缺省时渲染 null */
export function MetricChips({ metric }: { metric?: ServiceMetric }) {
  if (!metric) return null;
  const chips: { key: string; node: ReactElement }[] = [];

  if (metric.cpu_percent != null) {
    chips.push({
      key: "cpu",
      node: (
        <Chip
          label="CPU"
          value={`${metric.cpu_percent.toFixed(1)}%`}
          cls={levelCls[percentLevel(metric.cpu_percent)]}
        />
      ),
    });
  }
  if (metric.mem_bytes != null) {
    chips.push({
      key: "mem",
      node: (
        <Chip
          label="内存"
          value={formatBytes(metric.mem_bytes)}
          cls={
            metric.mem_percent != null
              ? levelCls[percentLevel(metric.mem_percent, 70, 90)]
              : undefined
          }
        />
      ),
    });
  }
  if (metric.gpu_percent != null) {
    chips.push({
      key: "gpu",
      node: (
        <Chip
          label="GPU"
          value={`${metric.gpu_percent.toFixed(0)}%`}
          cls={levelCls[percentLevel(metric.gpu_percent)]}
        />
      ),
    });
  }
  if (metric.gpu_mem_bytes != null) {
    chips.push({
      key: "vram",
      node: <Chip label="显存" value={formatBytes(metric.gpu_mem_bytes)} />,
    });
  }
  const disk = combinedRate(metric.disk_read_bytes_per_sec, metric.disk_write_bytes_per_sec);
  if (disk != null) {
    chips.push({
      key: "disk",
      node: (
        <Chip
          label="磁盘"
          value={formatRate(disk)}
          cls={levelCls[rateLevel(disk, DISK_WARN, DISK_ERR)]}
          title={`读 ${formatRate(metric.disk_read_bytes_per_sec)} / 写 ${formatRate(metric.disk_write_bytes_per_sec)}`}
        />
      ),
    });
  }
  const net = combinedRate(metric.net_rx_bytes_per_sec, metric.net_tx_bytes_per_sec);
  if (net != null) {
    chips.push({
      key: "net",
      node: (
        <Chip
          label="网络"
          value={formatRate(net)}
          cls={levelCls[rateLevel(net, NET_WARN, NET_ERR)]}
          title={`收 ${formatRate(metric.net_rx_bytes_per_sec)} / 发 ${formatRate(metric.net_tx_bytes_per_sec)}（TCP 口径）`}
        />
      ),
    });
  }

  if (chips.length === 0) return null;
  return (
    <span className="inline-flex items-center gap-x-1.5 whitespace-nowrap font-mono text-[10px]">
      {chips.map((c, i) => (
        <Fragment key={c.key}>
          {i > 0 && (
            <span aria-hidden className="text-muted/50">
              ·
            </span>
          )}
          {c.node}
        </Fragment>
      ))}
    </span>
  );
}
