// 设置页：通用偏好 + 开机自启 + WebDAV 同步 + 配置导入导出 + daemon 控制（启动/停止）
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { rpc, rpcSchema } from "../../shared/rpc/client";
import { ConfigGetSchema, SyncStatusSchema, type AppConfig } from "../../shared/rpc/schema";
import { describeError } from "../../shared/rpc/errors";
import { useDaemonConnection } from "../../shared/hooks/useDaemon";
import { StopDaemonDialog } from "./StopDaemonDialog";
import { ConfirmDialog } from "../../shared/components/ConfirmDialog";
import { useUiStore } from "../../stores/uiStore";
import { formatDateTime } from "../../shared/utils/format";

export function SettingsPage() {
  const qc = useQueryClient();
  const pushToast = useUiStore((s) => s.pushToast);

  const cfg = useQuery({
    queryKey: ["config"],
    queryFn: () => rpcSchema(ConfigGetSchema, "config.get"),
  });
  const sync = useQuery({
    queryKey: ["sync"],
    queryFn: () => rpcSchema(SyncStatusSchema, "sync.status"),
    refetchInterval: 10_000,
  });

  const [app, setApp] = useState<AppConfig | null>(null);
  const [webdavPwd, setWebdavPwd] = useState("");
  const [busy, setBusy] = useState(false);
  // 需求5：停止 daemon 进度弹窗 + 二次确认（state 必须在条件 return 之前声明）
  const [stopDialogOpen, setStopDialogOpen] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  // 配置导入：解析成功后先二次确认再覆盖写入
  const [pendingImport, setPendingImport] = useState<{ services: unknown; app?: unknown } | null>(
    null,
  );

  // ===== 开机自启模式（off | daemon | both；默认 daemon：登录后仅启动 daemon，不开 GUI）=====
  const bootMode = useQuery({
    queryKey: ["bootAutostartMode"],
    queryFn: () => invoke<string>("get_boot_autostart_mode"),
  });
  const [bootModeBusy, setBootModeBusy] = useState(false);
  const changeBootMode = async (mode: string) => {
    setBootModeBusy(true);
    try {
      await invoke("set_boot_autostart_mode", { mode });
      pushToast(
        "ok",
        mode === "off"
          ? "已关闭开机自启"
          : mode === "both"
            ? "已设置：登录后启动 GUI 并拉起 daemon"
            : "已设置：登录后自动启动 daemon（不打开 GUI）",
      );
      void qc.invalidateQueries({ queryKey: ["bootAutostartMode"] });
    } catch (e) {
      pushToast("err", describeError(e as never));
    } finally {
      setBootModeBusy(false);
    }
  };

  // ===== v2.2.0 任务8：daemon 看门狗（当前用户计划任务，每 5 分钟检查拉起） =====
  const watchdog = useQuery({
    queryKey: ["watchdog"],
    queryFn: () => invoke<boolean>("get_watchdog_enabled"),
  });
  const [watchdogBusy, setWatchdogBusy] = useState(false);
  const changeWatchdog = async (enabled: boolean) => {
    setWatchdogBusy(true);
    try {
      await invoke("set_watchdog_enabled", { enabled });
      pushToast(
        "ok",
        enabled ? "看门狗已启用（每 5 分钟自动检查）" : "看门狗已关闭",
      );
      void qc.invalidateQueries({ queryKey: ["watchdog"] });
    } catch (e) {
      pushToast("err", describeError(e as never));
    } finally {
      setWatchdogBusy(false);
    }
  };

  // ===== daemon 启停 =====
  const { state: daemonState } = useDaemonConnection();
  const startDaemon = async () => {
    setBusy(true);
    try {
      const ok = await invoke<boolean>("ensure_daemon");
      pushToast(ok ? "ok" : "err", ok ? "daemon 已启动" : "daemon 启动失败（exe 缺失或启动出错）");
      void qc.invalidateQueries();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  // config 加载后同步到本地编辑态
  useEffect(() => {
    if (cfg.data) setApp(cfg.data.app);
  }, [cfg.data]);

  // 错误态：明确提示 + 启动 daemon + 重试，绝不卡在"加载中"
  if (cfg.isError) {
    return (
      <div className="mx-auto max-w-3xl pt-10">
        <div className="rounded-xl border border-err/40 bg-err/5 p-6 text-center">
          <p className="text-sm font-medium text-err">无法读取设置</p>
          <p className="mt-1 text-xs text-muted">
            {cfg.error instanceof Error ? cfg.error.message : "daemon 连接失败"}
          </p>
          <div className="mt-4 flex justify-center gap-2">
            {/* 修复需求4：daemon 关闭后可在此一键重新启动 */}
            <button
              onClick={() => void startDaemon()}
              disabled={busy}
              className="min-h-9 rounded-lg bg-ok px-4 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50"
            >
              启动守护进程
            </button>
            <button
              onClick={() => void cfg.refetch()}
              className="min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3"
            >
              重试
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (cfg.isPending || !app) {
    return <p className="py-10 text-center text-sm text-muted">加载中…</p>;
  }

  const saveApp = async (okMsg = "设置已保存") => {
    setBusy(true);
    try {
      // 密码输入框有内容时一并提交（密码经 DPAPI 加密，前端不回显）
      await rpc("config.update", {
        app,
        ...(webdavPwd ? { webdav_password: webdavPwd } : {}),
      });
      setWebdavPwd("");
      void qc.invalidateQueries({ queryKey: ["config"] });
      pushToast("ok", okMsg);
      void qc.invalidateQueries({ queryKey: ["config"] });
      void qc.invalidateQueries({ queryKey: ["sync"] });
    } catch (e) {
      pushToast("err", describeError(e as never));
    } finally {
      setBusy(false);
    }
  };

  const runSync = async (method: "sync.run_now" | "sync.test") => {    setBusy(true);
    try {
      if (method === "sync.run_now") {
        // 先保存设置（含密码）再手动同步
        await saveApp("设置已保存，开始同步");
      }
      const r = await rpc<{ action?: string; message?: string; ok?: boolean }>(method);
      pushToast("ok", r.message ?? r.action ?? "完成");
      void qc.invalidateQueries({ queryKey: ["sync"] });
    } catch (e) {
      pushToast("err", describeError(e as never));
    } finally {
      setBusy(false);
    }
  };

  // v2.2.0 任务2：自动备份历史与一键回滚
  const backups = useQuery({
    queryKey: ["backups"],
    queryFn: () =>
      rpc<{ backups: { name: string; ts: string; size: number }[] }>("backup.list"),
  });
  const restoreBackup = async (name: string) => {
    if (!window.confirm("确定回滚到该备份吗？回滚前会把当前配置再自动备份一份。")) return;
    try {
      await rpc("backup.restore", { name });
      pushToast("ok", "已回滚到所选备份");
      void qc.invalidateQueries();
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // 需求5：停止 daemon → 二次确认 → 打开逐条关闭进度弹窗
  const shutdownDaemon = () => {
    setConfirmStop(true);
  };

  // ===== 配置备份：导出当前配置 / 导入覆盖 =====
  const exportConfig = async () => {
    try {
      const path = await save({
        title: "导出配置",
        defaultPath: `cli-companion-config-${new Date().toISOString().slice(0, 10)}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return; // 用户取消
      const data = await rpc<Record<string, unknown>>("config.export");
      await invoke("write_text_file", { path, contents: JSON.stringify(data, null, 2) });
      pushToast("ok", `配置已导出：${path}`);
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  const importConfig = async () => {
    try {
      const picked = await open({
        title: "选择配置备份文件",
        multiple: false,
        directory: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof picked !== "string") return; // 用户取消
      const raw = await invoke<string>("read_text_file", { path: picked });
      const parsed = JSON.parse(raw) as { services?: unknown; app?: unknown };
      if (!parsed.services || typeof parsed.services !== "object") {
        pushToast("err", "文件格式无效：缺少 services 配置段");
        return;
      }
      setPendingImport({ services: parsed.services, app: parsed.app });
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  const doImport = async (data: { services: unknown; app?: unknown }) => {
    setBusy(true);
    try {
      const r = await rpc<{ imported_services: number }>("config.import", data);
      pushToast("ok", `导入完成：共 ${r.imported_services} 个服务`);
      void qc.invalidateQueries(); // 配置/服务/同步状态全部刷新
    } catch (e) {
      pushToast("err", describeError(e as never));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <header>
        <h1 className="text-xl font-semibold">设置</h1>
      </header>

      {/* ===== 通用 ===== */}
      <Section title="通用">
        <Row label="语言">
          <select
            className={inputCls}
            value={app.general.language}
            onChange={(e) =>
              setApp({ ...app, general: { ...app.general, language: e.target.value } })
            }
          >
            <option value="zh-CN">简体中文</option>
            <option value="en">English</option>
          </select>
        </Row>
        {/* 开机自启：三档模式，默认 daemon（登录 Windows 后自动启动 daemon 进程，不显示 GUI） */}
        <Row label="开机自动启动">
          <div>
            <select
              className={inputCls}
              value={bootMode.data ?? "daemon"}
              disabled={bootModeBusy || bootMode.isPending}
              onChange={(e) => void changeBootMode(e.target.value)}
            >
              <option value="off">关闭（不自启动）</option>
              <option value="daemon">仅启动 daemon（默认）</option>
              <option value="both">启动 daemon + GUI</option>
            </select>
            <p className="mt-1 text-xs text-muted">
              登录 Windows 后自动启动 daemon 进程（默认不显示 GUI 窗口）；切换立即生效
            </p>
          </div>
        </Row>
        {/* 需求②②：关闭行为 */}
        <Row label="关闭窗口时最小化到托盘">
          <input
            type="checkbox"
            className="size-4 accent-[rgb(var(--accent))]"
            checked={app.general.close_to_tray}
            onChange={(e) =>
              setApp({ ...app, general: { ...app.general, close_to_tray: e.target.checked } })
            }
          />
          <p className="mt-1 text-xs text-muted">
            开启：点关闭按钮隐藏到托盘；关闭：点关闭按钮时弹窗确认退出方式
          </p>
        </Row>
        {/* v2.1.0：服务异常系统通知（daemon 发送；Win32 服务模式收不到） */}
        <Row label="服务异常系统通知">
          <div>
            <input
              type="checkbox"
              className="size-4 accent-[rgb(var(--accent))]"
              checked={app.general.notify_on_failure ?? true}
              onChange={(e) =>
                setApp({
                  ...app,
                  general: { ...app.general, notify_on_failure: e.target.checked },
                })
              }
            />
            <p className="mt-1 text-xs text-muted">
              服务崩溃、自动重启失败或触发熔断时弹出 Windows 系统通知（关闭 GUI 也能收到；
              以 Windows 服务模式运行时不可用）
            </p>
          </div>
        </Row>
        {/* v2.2.0 任务8：daemon 看门狗 */}
        <Row label="daemon 看门狗">
          <div>
            <input
              type="checkbox"
              className="size-4 accent-[rgb(var(--accent))]"
              disabled={watchdogBusy || watchdog.isPending}
              checked={watchdog.data ?? false}
              onChange={(e) => void changeWatchdog(e.target.checked)}
            />
            <p className="mt-1 text-xs text-muted">
              每 5 分钟自动检查 daemon，未运行则静默拉起（当前用户 Windows
              计划任务，无需管理员；默认关闭）
            </p>
          </div>
        </Row>
        <div className="flex justify-end pt-2">
          <button onClick={() => saveApp()} disabled={busy} className={btnPrimary}>
            保存通用设置
          </button>
        </div>
      </Section>

      {/* ===== WebDAV 同步 ===== */}
      <Section title="WebDAV 配置同步">
        <Row label="启用同步">
          <input
            type="checkbox"
            className="size-4 accent-[rgb(var(--accent))]"
            checked={app.webdav.enabled}
            onChange={(e) =>
              setApp({ ...app, webdav: { ...app.webdav, enabled: e.target.checked } })
            }
          />
        </Row>
        <Row label="服务器 URL">
          <input
            className={`${inputCls} font-mono text-xs`}
            placeholder="https://dav.example.com/dav/"
            value={app.webdav.url}
            onChange={(e) => setApp({ ...app, webdav: { ...app.webdav, url: e.target.value } })}
          />
        </Row>
        <Row label="用户名">
          <input
            className={inputCls}
            value={app.webdav.username}
            onChange={(e) => setApp({ ...app, webdav: { ...app.webdav, username: e.target.value } })}
          />
        </Row>
        <Row label="密码">
          <input
            className={inputCls}
            type="password"
            placeholder={sync.data?.password_set ? "已保存（输入可更新）" : "输入密码"}
            value={webdavPwd}
            onChange={(e) => setWebdavPwd(e.target.value)}
          />
          <p className="mt-1 text-xs text-muted">
            密码经 DPAPI 加密存本机，不参与同步、不显示明文
          </p>
        </Row>
        <Row label="远端目录">
          <input
            className={`${inputCls} font-mono text-xs`}
            value={app.webdav.remote_dir}
            onChange={(e) => setApp({ ...app, webdav: { ...app.webdav, remote_dir: e.target.value } })}
          />
        </Row>
        <Row label="同步间隔（分钟）">
          <input
            className={`${inputCls} w-28`}
            type="number"
            min={1}
            max={1440}
            value={app.webdav.sync_interval_minutes}
            onChange={(e) =>
              setApp({
                ...app,
                webdav: { ...app.webdav, sync_interval_minutes: Number(e.target.value) || 15 },
              })
            }
          />
        </Row>
        <Row label="校验 TLS 证书">
          <input
            type="checkbox"
            className="size-4 accent-[rgb(var(--accent))]"
            checked={app.webdav.verify_tls}
            onChange={(e) =>
              setApp({ ...app, webdav: { ...app.webdav, verify_tls: e.target.checked } })
            }
          />
        </Row>
        {/* 同步分项：配置文件 / CLI 应用目录 */}
        <Row label="同步配置文件">
          <div>
            <input
              type="checkbox"
              className="size-4 accent-[rgb(var(--accent))]"
              checked={app.webdav.sync_config}
              onChange={(e) =>
                setApp({ ...app, webdav: { ...app.webdav, sync_config: e.target.checked } })
              }
            />
            <p className="mt-1 text-xs text-muted">同步 services.json 等服务配置</p>
          </div>
        </Row>
        <Row label="同步 CLI 应用">
          <div>
            <input
              type="checkbox"
              className="size-4 accent-[rgb(var(--accent))]"
              checked={app.webdav.sync_cli_apps}
              onChange={(e) =>
                setApp({ ...app, webdav: { ...app.webdav, sync_cli_apps: e.target.checked } })
              }
            />
            <p className="mt-1 text-xs text-muted">
              同步数据目录下 <code className="font-mono">cli\</code> 中的二进制应用（递归子目录与文件）
            </p>
          </div>
        </Row>

        <div className="flex flex-wrap justify-end gap-2 pt-2">
          <button onClick={() => runSync("sync.test")} disabled={busy} className={btnSecondary}>
            测试连接
          </button>
          <button onClick={() => runSync("sync.run_now")} disabled={busy} className={btnSecondary}>
            立即同步
          </button>
          <button onClick={() => saveApp()} disabled={busy} className={btnPrimary}>
            保存同步设置
          </button>
        </div>

        {/* 同步状态 */}
        {sync.data && (
          <dl className="mt-3 rounded-lg bg-surface px-3 py-2 text-xs text-muted">
            <div>上次运行：{formatDateTime(sync.data.state.last_run)}</div>
            {sync.data.state.last_action && <div>结果：{sync.data.state.last_action}</div>}
            {sync.data.state.last_error && (
              <div className="text-err">错误：{sync.data.state.last_error}</div>
            )}
          </dl>
        )}
      </Section>

      {/* ===== 配置备份 ===== */}
      <Section title="配置备份">
        <p className="text-sm text-muted">
          导出服务与应用配置为 JSON 文件（含环境变量值，请妥善保管）；导入会
          <span className="font-medium text-content">覆盖</span>
          当前全部服务配置。WebDAV 凭据不参与导入导出。
        </p>
        <div className="flex justify-end gap-2 pt-1">
          <button onClick={() => void exportConfig()} disabled={busy} className={btnSecondary}>
            导出配置…
          </button>
          <button onClick={() => void importConfig()} disabled={busy} className={btnSecondary}>
            导入配置…
          </button>
        </div>

        {/* v2.2.0 任务2：自动备份历史（每次保存前快照，保留最近 20 份） */}
        {backups.data && backups.data.backups.length > 0 && (
          <div className="space-y-1.5 rounded-lg border border-surface-3 p-3">
            <p className="text-xs text-muted">
              自动备份（每次保存前生成，保留最近 20 份；回滚前会把当前配置再备份一份）
            </p>
            <ul className="max-h-44 space-y-1 overflow-y-auto">
              {backups.data.backups.map((b) => (
                <li key={b.name} className="flex items-center justify-between gap-3 text-xs">
                  <span className="font-mono text-muted">
                    {formatDateTime(b.ts)} · {(b.size / 1024).toFixed(1)} KB
                  </span>
                  <button
                    onClick={() => void restoreBackup(b.name)}
                    disabled={busy}
                    className="shrink-0 rounded-md border border-surface-3 px-2 py-0.5 text-accent hover:bg-accent/10 disabled:opacity-40"
                  >
                    回滚
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </Section>

      {/* ===== daemon ===== */}
      <Section title="守护进程">
        <div className="flex items-center gap-2 text-sm">
          <span
            className={`size-2 rounded-full ${daemonState === "connected" ? "bg-ok" : "bg-err"}`}
            aria-hidden
          />
          当前状态：
          <span className={daemonState === "connected" ? "text-ok" : "text-err"}>
            {daemonState === "connected" ? "运行中" : "未运行"}
          </span>
        </div>
        <p className="text-sm text-muted">
          关闭 GUI 不影响 daemon 与受管服务。停止 daemon 会同时停止全部受管服务；
          停止后可在此重新启动。
        </p>
        {/* v2.2.0 任务10：本机只读状态页 */}
        <Row label="本机状态页">
          <div>
            <div className="flex items-center justify-end gap-3">
              <input
                type="checkbox"
                className="size-4 accent-[rgb(var(--accent))]"
                checked={app.status_page?.enabled ?? false}
                onChange={(e) =>
                  setApp({
                    ...app,
                    status_page: {
                      enabled: e.target.checked,
                      port: app.status_page?.port ?? 8765,
                    },
                  })
                }
              />
              <input
                className="h-9 w-28 rounded-lg border border-surface-3 bg-surface px-3 text-sm focus:border-accent focus:outline-none"
                type="number"
                min={1024}
                max={65535}
                value={app.status_page?.port ?? 8765}
                onChange={(e) =>
                  setApp({
                    ...app,
                    status_page: {
                      enabled: app.status_page?.enabled ?? false,
                      port: Number(e.target.value) || 8765,
                    },
                  })
                }
              />
            </div>
            <p className="mt-1 text-xs text-muted">
              仅本机 127.0.0.1 可访问的只读页面（服务名/状态/CPU/内存），不含环境变量与任何
              操作能力；修改后需重启 daemon 生效。访问 http://127.0.0.1:端口/
            </p>
          </div>
        </Row>
        <div className="flex justify-end gap-2 pt-1">
          {/* 修复：daemon 停止后可重新手动启动 */}
          <button
            onClick={() => void startDaemon()}
            disabled={busy || daemonState === "connected"}
            className="min-h-9 rounded-lg border border-ok/50 px-4 text-sm text-ok hover:bg-ok/10 disabled:opacity-40"
          >
            启动 daemon
          </button>
          <button
            onClick={shutdownDaemon}
            disabled={busy || daemonState !== "connected"}
            className="min-h-9 rounded-lg border border-err/50 px-4 text-sm text-err hover:bg-err/10 disabled:opacity-40"
          >
            停止 daemon（含全部服务）
          </button>
        </div>
      </Section>

      {/* 需求5：停止前二次确认 */}
      <ConfirmDialog
        open={confirmStop}
        title="确认停止守护进程"
        message={
          daemonState === "connected"
            ? "即将停止全部受管服务并关闭守护进程，停止过程会逐条显示进度。确定继续吗？"
            : "确定要停止守护进程吗？"
        }
        actions={[
          { key: "cancel", label: "取消" },
          { key: "confirm", label: "确认停止", danger: true },
        ]}
        onAction={(key) => {
          setConfirmStop(false);
          if (key === "confirm") setStopDialogOpen(true);
        }}
      />

      {/* 配置导入：覆盖前二次确认 */}
      <ConfirmDialog
        open={pendingImport !== null}
        title="确认导入配置"
        message="导入将覆盖当前全部服务配置（运行中服务不受影响）。确定继续吗？"
        actions={[
          { key: "cancel", label: "取消" },
          { key: "confirm", label: "覆盖导入", danger: true },
        ]}
        onAction={(key) => {
          const data = pendingImport;
          setPendingImport(null);
          if (key === "confirm" && data) void doImport(data);
        }}
      />

      {/* 需求5：逐条服务关闭进度弹窗 */}
      <StopDaemonDialog
        open={stopDialogOpen}
        onClose={() => setStopDialogOpen(false)}
        onFinished={() => {
          void qc.invalidateQueries();
        }}
      />
    </div>
  );
}

const inputCls =
  "h-9 w-full max-w-md rounded-lg border border-surface-3 bg-surface px-3 text-sm focus:border-accent focus:outline-none";
const btnPrimary = "min-h-9 rounded-lg bg-accent px-4 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50";
const btnSecondary = "min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3 disabled:opacity-50";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-3 rounded-xl border border-surface-3 bg-surface-2 p-5">
      <h2 className="text-sm font-semibold">{title}</h2>
      {children}
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span className="shrink-0 text-sm">{label}</span>
      <div className="flex-1 text-right">{children}</div>
    </div>
  );
}
