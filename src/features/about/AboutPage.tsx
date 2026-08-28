// 关于页：动态版本号、版本检测（GitHub Releases）、功能介绍、作者、开源地址、应用图标
import { useCallback, useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Github, User, Sparkles, ExternalLink, RefreshCw, Download, CheckCircle2, AlertTriangle } from "lucide-react";

const REPO_URL = "https://github.com/w2018/CLI-Companion";
const RELEASES_URL = `${REPO_URL}/releases/latest`;

const FEATURES = [
  "集中管理多个 CLI 服务：启动、停止、重启、监控，参数可视化编辑",
  "GUI 关闭后服务常驻后台，重开自动恢复状态；崩溃自动重启 + 熔断保护",
  "Windows Job Object 进程树管理，停止服务不留孤儿进程",
  "每服务独立日志：实时查看、自动轮转归档",
  "WebDAV 配置同步：多设备协作，冲突显式化处理，凭据 DPAPI 加密",
  "Win32 服务模式：开机自启、无人值守运行",
];

interface ReleaseInfo {
  tag: string;
  url: string;
  publishedAt: string | null;
}

type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; release: ReleaseInfo }
  | { kind: "error"; message: string };

/** 解析 "v1.2.3" 风格版本号为可比较的数值三元组 */
function parseVersion(v: string): [number, number, number] | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(v.trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/** 判断 latest 是否比 current 更新；任一版本号无法解析时返回 null（交由人工判断） */
function isNewer(latest: string, current: string): boolean | null {
  const a = parseVersion(latest);
  const b = parseVersion(current);
  if (!a || !b) return null;
  return a[0] !== b[0] || a[1] !== b[1] || a[2] !== b[2];
}

export function AboutPage() {
  const [version, setVersion] = useState<string>("…");
  const [update, setUpdate] = useState<UpdateState>({ kind: "idle" });

  // 动态获取应用版本号（来自 tauri.conf.json，单一事实源）
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion("未知"));
  }, []);

  const checkUpdate = useCallback(
    async (silent: boolean) => {
      if (!silent) setUpdate({ kind: "checking" });
      try {
        const res = await fetch(
          "https://api.github.com/repos/w2018/CLI-Companion/releases/latest",
          { headers: { Accept: "application/vnd.github+json" } },
        );
        if (!res.ok) throw new Error(`GitHub API 返回 ${res.status}`);
        const data = (await res.json()) as {
          tag_name?: string;
          html_url?: string;
          published_at?: string;
        };
        const tag = data.tag_name ?? "";
        const release: ReleaseInfo = {
          tag,
          url: data.html_url ?? RELEASES_URL,
          publishedAt: data.published_at ?? null,
        };
        const newer = isNewer(tag, version);
        // 版本号无法解析时不自动判定，展示新版本信息交给用户决定
        if (newer === false) {
          setUpdate({ kind: "latest" });
        } else {
          setUpdate({ kind: "available", release });
        }
      } catch (e) {
        // 静默检查（页面打开时）失败不打扰，仅保留手动重试入口
        if (!silent) {
          setUpdate({
            kind: "error",
            message: e instanceof Error ? e.message : String(e),
          });
        } else {
          setUpdate({ kind: "idle" });
        }
      }
    },
    [version],
  );

  // 打开页面时静默检查一次（失败不提示，避免网络受限时打扰）
  useEffect(() => {
    if (version !== "…" && version !== "未知") void checkUpdate(true);
  }, [version, checkUpdate]);

  return (
    <div className="mx-auto max-w-2xl space-y-6">
      {/* 应用标识 */}
      <section className="flex items-center gap-5 rounded-2xl border border-surface-3 bg-gradient-to-br from-surface-2 to-surface p-6">
        <img
          src="/app-icon.png"
          alt="CLI Companion 应用图标"
          className="size-20 rounded-2xl shadow-md"
        />
        <div>
          <h1 className="text-2xl font-bold">CLI Companion</h1>
          <p className="mt-1 text-sm text-muted">CLI 应用辅助 —— Windows 桌面 CLI 服务管家</p>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <span className="inline-flex rounded-full bg-accent/10 px-3 py-0.5 text-xs font-medium text-accent">
              版本 {version}
            </span>
            <button
              onClick={() => void checkUpdate(false)}
              disabled={update.kind === "checking" || version === "…" || version === "未知"}
              className="inline-flex min-h-7 items-center gap-1 rounded-full border border-surface-3 px-2.5 text-xs text-muted hover:bg-surface-3 hover:text-content disabled:opacity-50"
            >
              <RefreshCw size={11} className={update.kind === "checking" ? "animate-spin" : ""} aria-hidden />
              检查更新
            </button>
          </div>
        </div>
      </section>

      {/* 版本检测结果 */}
      {update.kind === "latest" && (
        <p
          role="status"
          className="flex items-center gap-2 rounded-xl bg-ok/10 px-4 py-3 text-sm text-ok"
        >
          <CheckCircle2 size={15} aria-hidden /> 当前已是最新版本
        </p>
      )}
      {update.kind === "available" && (
        <div
          role="status"
          className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-accent/30 bg-accent/5 px-4 py-3"
        >
          <Sparkles size={15} className="text-accent" aria-hidden />
          <span className="text-sm">
            发现新版本 <span className="font-semibold text-accent">{update.release.tag}</span>
            {update.release.publishedAt && (
              <span className="ml-2 text-xs text-muted">
                发布于 {new Date(update.release.publishedAt).toLocaleString("zh-CN", { hour12: false })}
              </span>
            )}
          </span>
          <button
            onClick={() => void openUrl(update.release.url).catch(() => undefined)}
            className="ml-auto inline-flex min-h-9 items-center gap-1.5 rounded-lg bg-accent px-4 text-sm font-medium text-white hover:opacity-90"
          >
            <Download size={14} aria-hidden /> 前往下载
          </button>
        </div>
      )}
      {update.kind === "error" && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl bg-warn/10 px-4 py-3 text-sm text-warn"
        >
          <AlertTriangle size={15} aria-hidden />
          <span>检查更新失败（{update.message}），可稍后重试或直接前往发布页查看</span>
          <button
            onClick={() => void openUrl(RELEASES_URL).catch(() => undefined)}
            className="ml-auto inline-flex min-h-8 items-center gap-1 rounded-lg border border-warn/40 px-3 text-xs hover:bg-warn/10"
          >
            发布页 <ExternalLink size={12} aria-hidden />
          </button>
        </div>
      )}

      {/* 主要功能 */}
      <section className="rounded-2xl border border-surface-3 bg-surface-2 p-6">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Sparkles size={15} className="text-accent" aria-hidden /> 主要功能
        </h2>
        <ul className="space-y-2">
          {FEATURES.map((f) => (
            <li key={f} className="flex items-start gap-2 text-sm">
              <span className="mt-1.5 size-1.5 shrink-0 rounded-full bg-accent/60" aria-hidden />
              <span className="text-muted">{f}</span>
            </li>
          ))}
        </ul>
      </section>

      {/* 作者与开源 */}
      <section className="space-y-3 rounded-2xl border border-surface-3 bg-surface-2 p-6">
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-2 text-sm">
            <User size={15} className="text-muted" aria-hidden /> 作者
          </span>
          <span className="text-sm font-medium">曾先生</span>
        </div>
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-2 text-sm">
            <Github size={15} className="text-muted" aria-hidden /> 开源地址
          </span>
          <button
            onClick={() => void openUrl(REPO_URL).catch(() => undefined)}
            className="inline-flex min-h-8 items-center gap-1.5 rounded-lg border border-surface-3 px-3 text-xs text-accent hover:bg-surface-3"
          >
            {REPO_URL.replace("https://", "")}
            <ExternalLink size={12} aria-hidden />
          </button>
        </div>
        <p className="border-t border-surface-3 pt-3 text-xs text-muted">
          本程序基于 Rust + Tauri 2 + React 构建，遵循 MIT 协议开源。
        </p>
      </section>
    </div>
  );
}
