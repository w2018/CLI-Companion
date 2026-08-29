// 服务新建/编辑表单：身份与运行 / 参数与环境
// - exe 路径 / 工作目录支持手动输入与文件对话框选择（需求1）
// - exe 设置后工作目录自动跟随为 exe 所在目录（可自定义覆盖）
// - 点击遮罩不关闭窗口，防止误触丢失已填内容（需求3）
// - 提交前 Zod 校验，错误定位到具体字段
import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { X, FolderOpen, Copy } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { rpc } from "../../shared/rpc/client";
import {
  ServiceDefinitionSchema,
  type Arg,
  type EnvVar,
  type ServiceDefinition,
} from "../../shared/rpc/schema";
import { describeError } from "../../shared/rpc/errors";
import { useUiStore } from "../../stores/uiStore";
import { ArgsEditor } from "./ArgsEditor";
import { parseCommandLine } from "../../shared/utils/parseCommandLine";

interface Props {
  initial: ServiceDefinition | null;
  /** v2.2.0 任务7：克隆源（提供时以"新建"方式提交副本） */
  cloneOf?: ServiceDefinition | null;
  onClose: () => void;
}

/** 空白服务定义（新建用） */
function blankService(): ServiceDefinition {
  const now = new Date().toISOString();
  return {
    id: crypto.randomUUID(),
    name: "",
    description: "",
    enabled: true,
    autostart: false,
    exe: "",
    args: [],
    argument_delimiter: " ",
    working_dir: null,
    env: [],
    run_as: { kind: "current_user" },
    console: { mode: "no_console", startup: "normal" },
    stop: { signal: "ctrl_c", graceful_timeout_ms: 15000, kill_timeout_ms: 10000 },
    health: { kind: "process", interval_ms: 5000, failure_threshold: 3, success_threshold: 1 },
    restart: {
      policy: "on_failure",
      max_attempts_10m: 10,
      backoff: { initial_ms: 2000, max_ms: 300000, multiplier: 2 },
    },
    labels: [],
    created_at: now,
    updated_at: now,
  };
}

/** 从 exe 路径提取所在目录 */
function dirOf(exePath: string): string | null {
  const i = Math.max(exePath.lastIndexOf("\\"), exePath.lastIndexOf("/"));
  return i > 0 ? exePath.slice(0, i) : null;
}

/** 克隆服务定义：新 ID、名称加"-副本"、刷新时间戳（v2.2.0 任务7） */
function clonedService(src: ServiceDefinition): ServiceDefinition {
  const now = new Date().toISOString();
  return { ...src, id: crypto.randomUUID(), name: `${src.name}-副本`, created_at: now, updated_at: now };
}

export function ServiceForm({ initial, cloneOf = null, onClose }: Props) {
  const qc = useQueryClient();
  const pushToast = useUiStore((s) => s.pushToast);
  const isClone = cloneOf !== null;
  const [svc, setSvc] = useState<ServiceDefinition>(initial ?? (cloneOf ? clonedService(cloneOf) : blankService()));
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const set = <K extends keyof ServiceDefinition>(key: K, value: ServiceDefinition[K]) =>
    setSvc((s) => ({ ...s, [key]: value }));

  /** 渲染完整命令行（exe + 启用参数），供复制调试；与 daemon render_args 语义一致 */
  const commandLine = useMemo(() => {
    if (!svc.exe) return "";
    const quote = (s: string) => (s.includes(" ") ? `"${s.replaceAll('"', '\\"')}"` : s);
    const parts: string[] = [quote(svc.exe)];
    for (const a of svc.args) {
      if (!a.enabled) continue;
      if (a.kind === "flag") parts.push(quote(a.key));
      else if (a.kind === "option" && a.value != null) parts.push(quote(a.key), quote(a.value));
      else if (a.kind === "positional" && a.value != null) parts.push(quote(a.value));
    }
    return parts.join(" ");
  }, [svc.exe, svc.args]);

  const copyCommandLine = async () => {
    try {
      await navigator.clipboard.writeText(commandLine);
      pushToast("ok", "完整命令行已复制");
    } catch {
      pushToast("err", "复制失败，请手动选择文本复制");
    }
  };

  // v2.2.0 任务6：探活方式（kind 序列化：进程为字符串 "process"，命令为 {command:{program,args}}）
  const healthKind = svc.health.kind as
    | string
    | { command?: { program: string; args: string[] } }
    | null
    | undefined;
  const healthIsCommand = typeof healthKind === "object" && healthKind !== null && "command" in healthKind;
  const healthCmd = healthIsCommand
    ? (healthKind as { command: { program: string; args: string[] } }).command
    : null;

  /** v2.2.0 任务1：粘贴整条命令行 → 解析填充 exe 与参数编辑器 */
  const [pasteInput, setPasteInput] = useState("");
  const applyPastedCommand = () => {
    const parsed = parseCommandLine(pasteInput.trim());
    if (!parsed) {
      pushToast("err", "命令行为空或无法解析");
      return;
    }
    const fill = () => {
      setSvc((s) => ({
        ...s,
        exe: parsed.exe,
        args: parsed.args.map((a) => ({
          id: crypto.randomUUID(),
          key: a.key,
          value: a.value,
          enabled: true,
          kind: a.kind,
          description: "",
        })),
        // 工作目录跟随新 exe 所在目录（留空时）
        ...(dirOf(parsed.exe) && !s.working_dir ? { working_dir: dirOf(parsed.exe) } : {}),
      }));
      setPasteInput("");
      pushToast("ok", `已解析：exe + ${parsed.args.length} 个参数，可继续微调`);
    };
    if (svc.exe || svc.args.length > 0) {
      if (window.confirm("粘贴解析将覆盖当前 exe 与全部参数，确定吗？")) fill();
    } else {
      fill();
    }
  };

  /** 需求1：exe 变更后，工作目录为空（或等于旧 exe 目录）时自动跟随为新的 exe 目录 */
  const setExe = (path: string) => {
    setSvc((s) => {
      const oldDir = dirOf(s.exe);
      const newDir = dirOf(path);
      const follow =
        s.working_dir === null || s.working_dir === "" || s.working_dir === oldDir;
      return { ...s, exe: path, working_dir: follow && newDir ? newDir : s.working_dir };
    });
  };

  /** 浏览选择 exe 文件 */
  const browseExe = async () => {
    const picked = await open({
      multiple: false,
      directory: false,
      title: "选择可执行文件",
      filters: [{ name: "可执行文件", extensions: ["exe", "bat", "cmd", "com", "ps1"] }],
    });
    if (typeof picked === "string") setExe(picked);
  };

  /** 浏览选择工作目录 */
  const browseWorkingDir = async () => {
    const picked = await open({ directory: true, title: "选择工作目录" });
    if (typeof picked === "string") set("working_dir", picked);
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      // 提交前 Zod 校验
      const parsed = ServiceDefinitionSchema.safeParse(svc);
      if (!parsed.success) {
        const first = parsed.error.issues[0];
        setError(`字段「${first.path.join(".")}」校验失败：${first.message}`);
        return;
      }
      const method = initial && !isClone ? "service.update" : "service.create";
      await rpc(method, { service: svc });
      pushToast("ok", initial ? "服务已更新" : "服务已创建");
      void qc.invalidateQueries({ queryKey: ["services"] });
      onClose();
    } catch (e) {
      setError(describeError(e as never));
    } finally {
      setSaving(false);
    }
  };

  return (
    // 需求3：点击遮罩不关闭（数据保护），仅右上角 X / 取消按钮可关闭
    <div
      role="dialog"
      aria-modal="true"
      aria-label={initial && !isClone ? `编辑服务 ${initial.name}` : "新建服务"}
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-6"
    >
      <div className="flex max-h-full w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-surface-3 bg-surface-2 shadow-2xl">
        <header className="flex items-center justify-between border-b border-surface-3 px-5 py-3">
          <h2 className="text-base font-semibold">
            {initial && !isClone ? "编辑服务" : isClone ? "新建服务（副本）" : "新建服务"}
          </h2>
          <button
            aria-label="关闭"
            onClick={onClose}
            className="inline-flex size-8 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
          >
            <X size={16} aria-hidden />
          </button>
        </header>

        <div className="flex-1 space-y-5 overflow-y-auto p-5">
          {/* ===== 身份与运行 ===== */}
          <fieldset className="space-y-3">
            <legend className="text-xs font-semibold uppercase tracking-wide text-muted">
              身份与运行
            </legend>

            {/* v2.2.0 任务1：粘贴整条命令行一键填充 */}
            <Field label="从命令行粘贴（自动解析 exe 与参数）">
              <div className="flex gap-2">
                <input
                  className={`${inputCls} flex-1 font-mono text-xs`}
                  value={pasteInput}
                  onChange={(e) => setPasteInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      applyPastedCommand();
                    }
                  }}
                  placeholder='如：java -Xms512m -jar app.jar --port 8080'
                />
                <button
                  type="button"
                  onClick={applyPastedCommand}
                  disabled={!pasteInput.trim()}
                  className="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-accent/40 px-3 text-xs text-accent hover:bg-accent/10 disabled:opacity-40"
                >
                  解析填充
                </button>
              </div>
            </Field>

            {/* exe 路径：手动输入 或 文件选择 */}
            <Field label="exe 路径 *">
              <div className="flex gap-2">
                <input
                  className={`${inputCls} flex-1 font-mono text-xs`}
                  value={svc.exe}
                  onChange={(e) => setExe(e.target.value)}
                  placeholder="C:\Tools\agent.exe 或点击右侧按钮选择"
                />
                <button
                  type="button"
                  onClick={() => void browseExe()}
                  className="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-surface-3 px-3 text-xs hover:bg-surface-3"
                >
                  <FolderOpen size={13} aria-hidden /> 选择文件
                </button>
              </div>
            </Field>

            {/* 工作目录：手动输入 或 目录选择；exe 设置后自动跟随 */}
            <Field label="工作目录">
              <div className="flex gap-2">
                <input
                  className={`${inputCls} flex-1 font-mono text-xs`}
                  value={svc.working_dir ?? ""}
                  onChange={(e) => set("working_dir", e.target.value || null)}
                  placeholder="留空时跟随 exe 所在目录；可手动修改"
                />
                <button
                  type="button"
                  onClick={() => void browseWorkingDir()}
                  className="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-surface-3 px-3 text-xs hover:bg-surface-3"
                >
                  <FolderOpen size={13} aria-hidden /> 选择目录
                </button>
              </div>
            </Field>

            {/* 完整命令行预览：exe + 启用参数，一键复制到终端调试 */}
            {commandLine && (
              <Field label="完整命令行预览">
                <div className="flex gap-2">
                  <code className="min-h-9 flex-1 truncate rounded-lg border border-dashed border-surface-3 bg-surface px-3 py-2 font-mono text-xs text-muted">
                    {commandLine}
                  </code>
                  <button
                    type="button"
                    onClick={() => void copyCommandLine()}
                    className="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-surface-3 px-3 text-xs hover:bg-surface-3"
                  >
                    <Copy size={13} aria-hidden /> 复制
                  </button>
                </div>
              </Field>
            )}

            <div className="grid grid-cols-2 gap-3">
              <Field label="名称 *">
                <input
                  className={inputCls}
                  value={svc.name}
                  onChange={(e) => set("name", e.target.value)}
                  placeholder="如：本地代理"
                />
              </Field>
              <Field label="说明（仅界面展示）">
                <input
                  className={inputCls}
                  value={svc.description}
                  onChange={(e) => set("description", e.target.value)}
                />
              </Field>
            </div>
            <div className="grid grid-cols-3 gap-3">
              <Field label="控制台模式">
                <select
                  className={inputCls}
                  value={svc.console.mode}
                  onChange={(e) =>
                    set("console", { ...svc.console, mode: e.target.value as never })
                  }
                >
                  <option value="no_console">无窗口</option>
                  <option value="new_console_visible">新控制台（可见）</option>
                  <option value="new_console_hidden">新控制台（隐藏）</option>
                </select>
              </Field>
              <Field label="重启策略">
                <select
                  className={inputCls}
                  value={svc.restart.policy}
                  onChange={(e) =>
                    set("restart", { ...svc.restart, policy: e.target.value as never })
                  }
                >
                  <option value="on_failure">失败时自动重启</option>
                  <option value="always">总是自动重启</option>
                  <option value="never">不自动重启</option>
                </select>
              </Field>
              <Field label="开机自启">
                <label className="flex h-9 items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    className="size-4 accent-[rgb(var(--accent))]"
                    checked={svc.autostart}
                    onChange={(e) => set("autostart", e.target.checked)}
                  />
                  daemon 启动时自动运行
                </label>
              </Field>
            </div>

            {/* v2.2.0 任务5/6：探活与告警 */}
            <div className="grid grid-cols-3 gap-3">
              <Field label="探活方式">
                <select
                  className={inputCls}
                  value={healthIsCommand ? "command" : "process"}
                  onChange={(e) => {
                    if (e.target.value === "command") {
                      set("health", {
                        ...svc.health,
                        kind: { command: { program: "", args: [] } },
                      });
                    } else {
                      set("health", { ...svc.health, kind: "process" });
                    }
                  }}
                >
                  <option value="process">进程存活（默认）</option>
                  <option value="command">自定义命令（退出码 0 = 健康）</option>
                </select>
              </Field>
              <Field label="探活命令程序">
                <input
                  className={`${inputCls} font-mono text-xs`}
                  disabled={!healthIsCommand}
                  value={healthCmd?.program ?? ""}
                  onChange={(e) =>
                    set("health", {
                      ...svc.health,
                      kind: { command: { program: e.target.value, args: healthCmd?.args ?? [] } },
                    })
                  }
                  placeholder={healthIsCommand ? "如 mysqladmin" : "仅命令探活时使用"}
                />
              </Field>
              <Field label="内存告警阈值（MB，留空关闭）">
                <input
                  className={inputCls}
                  type="number"
                  min={0}
                  value={svc.mem_alert_mb ?? ""}
                  onChange={(e) =>
                    set("mem_alert_mb", e.target.value ? Number(e.target.value) : null)
                  }
                  placeholder="如 1024"
                />
              </Field>
            </div>
            {healthIsCommand && (
              <Field label="探活命令参数（空格分隔）">
                <input
                  className={`${inputCls} font-mono text-xs`}
                  value={(healthCmd?.args ?? []).join(" ")}
                  onChange={(e) =>
                    set("health", {
                      ...svc.health,
                      kind: {
                        command: {
                          program: healthCmd?.program ?? "",
                          args: e.target.value.split(" ").filter(Boolean),
                        },
                      },
                    })
                  }
                  placeholder="如 ping -h localhost"
                />
              </Field>
            )}
          </fieldset>

          {/* ===== 参数与环境 ===== */}
          <fieldset className="space-y-4">
            <legend className="text-xs font-semibold uppercase tracking-wide text-muted">
              启动参数与环境变量
            </legend>
            <ArgsEditor args={svc.args} onChange={(args: Arg[]) => set("args", args)} />
            <EnvEditor env={svc.env} onChange={(env: EnvVar[]) => set("env", env)} />
          </fieldset>
        </div>

        <footer className="space-y-2 border-t border-surface-3 px-5 py-3">
          {error && (
            <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button onClick={onClose} className="min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3">
              取消
            </button>
            <button
              onClick={save}
              disabled={saving}
              className="min-h-9 rounded-lg bg-accent px-5 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50"
            >
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}

/** 环境变量编辑（需求2：宽松布局，独立区块） */
function EnvEditor({ env, onChange }: { env: EnvVar[]; onChange: (next: EnvVar[]) => void }) {
  const update = (i: number, patch: Partial<EnvVar>) =>
    onChange(env.map((x, j) => (j === i ? { ...x, ...patch } : x)));

  return (
    <div className="space-y-2.5">
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted">环境变量（标记机密的不同步、不入日志）</p>
        <button
          type="button"
          onClick={() => onChange([...env, { name: "", value: "", secret: false }])}
          className="inline-flex min-h-8 items-center gap-1 rounded-md border border-surface-3 px-2.5 text-xs hover:bg-surface-3"
        >
          添加变量
        </button>
      </div>

      {env.length === 0 ? (
        <p className="rounded-lg border border-dashed border-surface-3 px-3 py-3.5 text-center text-xs text-muted">
          未设置环境变量
        </p>
      ) : (
        <ul className="space-y-2.5">
          {env.map((v, i) => (
            <li key={i} className="rounded-lg border border-surface-3 bg-surface p-3">
              {/* 第一行：变量名 + 删除 */}
              <div className="mb-2 flex items-center gap-2">
                <span className="w-14 shrink-0 text-xs text-muted">变量名</span>
                <input
                  aria-label={`变量 ${i + 1} 名`}
                  placeholder="NAME"
                  value={v.name}
                  onChange={(e) => update(i, { name: e.target.value })}
                  className="h-8 min-w-0 flex-1 rounded-md border border-surface-3 bg-surface-2 px-2 font-mono text-xs"
                />
                <button
                  type="button"
                  aria-label={`删除变量 ${i + 1}`}
                  onClick={() => onChange(env.filter((_, j) => j !== i))}
                  className="shrink-0 text-xs text-muted hover:text-err"
                >
                  删除
                </button>
              </div>
              {/* 第二行：变量值 + 机密开关 */}
              <div className="flex items-center gap-2">
                <span className="w-14 shrink-0 text-xs text-muted">值</span>
                <input
                  aria-label={`变量 ${i + 1} 值`}
                  placeholder={
                    v.secret && v.value === "__encrypted__"
                      ? "已加密保存（输入可更新，清空即删除）"
                      : "值"
                  }
                  type={v.secret ? "password" : "text"}
                  value={v.value}
                  onChange={(e) => update(i, { value: e.target.value })}
                  className="h-8 min-w-0 flex-1 rounded-md border border-surface-3 bg-surface-2 px-2 font-mono text-xs"
                />
                <label className="flex shrink-0 cursor-pointer items-center gap-1.5 text-xs text-muted">
                  <input
                    type="checkbox"
                    checked={v.secret}
                    onChange={(e) => update(i, { secret: e.target.checked })}
                    className="size-3.5 accent-[rgb(var(--accent))]"
                  />
                  机密
                </label>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const inputCls =
  "h-9 w-full rounded-lg border border-surface-3 bg-surface px-3 text-sm focus:border-accent focus:outline-none";

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs text-muted">{label}</span>
      {children}
    </label>
  );
}
