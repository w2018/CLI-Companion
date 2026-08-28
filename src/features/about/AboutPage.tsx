// 关于页：动态版本号、功能介绍、作者、开源地址、应用图标
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Github, User, Sparkles, ExternalLink } from "lucide-react";

const REPO_URL = "https://github.com/w2018/CLI-Companion";

const FEATURES = [
  "集中管理多个 CLI 服务：启动、停止、重启、监控，参数可视化编辑",
  "GUI 关闭后服务常驻后台，重开自动恢复状态；崩溃自动重启 + 熔断保护",
  "Windows Job Object 进程树管理，停止服务不留孤儿进程",
  "每服务独立日志：实时查看、自动轮转归档",
  "WebDAV 配置同步：多设备协作，冲突显式化处理，凭据 DPAPI 加密",
  "Win32 服务模式：开机自启、无人值守运行",
];

export function AboutPage() {
  const [version, setVersion] = useState<string>("…");

  // 动态获取应用版本号（来自 tauri.conf.json，单一事实源）
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion("未知"));
  }, []);

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
          <p className="mt-2 inline-flex rounded-full bg-accent/10 px-3 py-0.5 text-xs font-medium text-accent">
            版本 {version}
          </p>
        </div>
      </section>

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
