// 应用功能页（v2.6.0）：承载 daemon 内置工具型功能（FTP 服务端等），后续新功能统一放此页
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
// eslint-disable-next-line import/order
import { open } from "@tauri-apps/plugin-dialog";
import { rpc, rpcSchema } from "../../shared/rpc/client";
import {
  ConfigGetSchema,
  FtpStatusSchema,
  type FtpPermissions,
  type FtpSettings,
  type FtpUser,
} from "../../shared/rpc/schema";
import { describeError } from "../../shared/rpc/errors";
import { useUiStore } from "../../stores/uiStore";

const inputCls =
  "h-9 w-full rounded-lg border border-surface-3 bg-surface px-3 text-sm focus:border-accent focus:outline-none";
const btnPrimary =
  "min-h-9 rounded-lg bg-accent px-4 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50";
const btnSecondary =
  "min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3 disabled:opacity-50";
const btnSmall = "min-h-8 rounded-lg border border-surface-3 px-3 text-xs hover:bg-surface-3";
const checkCls = "size-4 accent-[rgb(var(--accent))]";

/** 权限项（与 Rust FtpPermissions 字段一一对应） */
const PERMISSION_ITEMS: { key: keyof FtpPermissions; label: string }[] = [
  { key: "list", label: "浏览/列目录" },
  { key: "download", label: "下载" },
  { key: "upload", label: "上传/写入" },
  { key: "delete", label: "删除" },
  { key: "rename", label: "重命名/移动" },
  { key: "mkdir", label: "创建目录" },
];

const DEFAULT_FTP: FtpSettings = {
  enabled: false,
  passive_port_start: 50000,
  passive_port_end: 50100,
  listeners: [],
  users: [],
};

const overlayCls =
  "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4";
const dialogCls =
  "w-full max-w-lg space-y-4 rounded-xl border border-surface-3 bg-surface-2 p-5 shadow-xl";

export function AppsPage() {
  const qc = useQueryClient();
  const pushToast = useUiStore((s) => s.pushToast);

  const cfg = useQuery({
    queryKey: ["config"],
    queryFn: () => rpcSchema(ConfigGetSchema, "config.get"),
  });
  const ftpStatus = useQuery({
    queryKey: ["ftp-status"],
    queryFn: () => rpcSchema(FtpStatusSchema, "ftp.status"),
    refetchInterval: 5000,
  });

  // 站点设置的本地草稿（监听器 + 被动端口区间），独立保存
  const [draft, setDraft] = useState<FtpSettings | null>(null);
  useEffect(() => {
    setDraft(cfg.data?.app.ftp ?? null);
  }, [cfg.data]);

  // hooks 全部声明后才能提前返回（React #310 教训）
  if (cfg.isError) {
    return (
      <div className="mx-auto max-w-3xl pt-10">
        <div className="rounded-xl border border-err/40 bg-err/5 p-6 text-center">
          <p className="text-sm font-medium text-err">无法读取应用功能配置</p>
          <button className={`${btnSecondary} mt-3`} onClick={() => void cfg.refetch()}>
            重试
          </button>
        </div>
      </div>
    );
  }
  if (cfg.isPending || !cfg.data) {
    return <p className="pt-10 text-center text-sm text-muted">加载中…</p>;
  }

  const app = cfg.data.app;
  const ftp = app.ftp ?? DEFAULT_FTP;

  /** 保存 FTP 配置段（extra 可携带 ftp_user_password 等单独参数） */
  const saveFtp = async (next: FtpSettings, extra?: Record<string, unknown>, okMsg?: string) => {
    try {
      await rpc("config.update", { app: { ...app, ftp: next }, ...extra });
      void qc.invalidateQueries({ queryKey: ["config"] });
      void qc.invalidateQueries({ queryKey: ["ftp-status"] });
      if (okMsg) pushToast("ok", okMsg);
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // ===== 状态卡操作：启用开关立即生效 =====
  const toggleEnabled = async (v: boolean) => {
    await saveFtp({ ...ftp, enabled: v }, undefined, v ? "FTP 服务已启用" : "FTP 服务已停用");
  };

  // ===== 站点草稿保存 =====
  const saveDraft = async () => {
    if (!draft) return;
    await saveFtp({ ...ftp, enabled: draft.enabled, passive_port_start: draft.passive_port_start, passive_port_end: draft.passive_port_end, listeners: draft.listeners }, undefined, "FTP 站点设置已保存");
  };

  // ===== 用户操作（立即保存；密码经 DPAPI 加密单独提交） =====
  const saveUsers = async (users: FtpUser[], pwd?: { username: string; password: string }, okMsg?: string) => {
    await saveFtp(
      { ...ftp, users },
      pwd ? { ftp_user_password: pwd } : undefined,
      okMsg ?? "FTP 用户已保存（对新登录生效）",
    );
  };

  const listenerRoots = ftp.listeners.map((l) => ({ port: l.port, root: l.root }));

  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <header>
        <h1 className="text-xl font-semibold">应用功能</h1>
        <p className="mt-1 text-xs text-muted">daemon 内置工具型功能；FTP、TCP 调试等后续功能统一在此页管理</p>
      </header>

      {/* ===== FTP 服务状态 ===== */}
      <section className="space-y-3 rounded-xl border border-surface-3 bg-surface-2 p-5">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">FTP 服务</h2>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className={checkCls}
              checked={ftp.enabled}
              onChange={(e) => void toggleEnabled(e.target.checked)}
            />
            启用
          </label>
        </div>
        {ftpStatus.isError ? (
          <p className="text-xs text-err">状态获取失败（daemon 不可达或版本过旧）</p>
        ) : ftpStatus.data ? (
          <div className="space-y-1 text-sm">
            <p>
              <span className="text-muted">状态：</span>
              {ftpStatus.data.running ? (
                <span className="font-medium text-ok">运行中</span>
              ) : ftpStatus.data.enabled ? (
                <span className="font-medium text-warn">启动中/异常</span>
              ) : (
                <span className="text-muted">已停用</span>
              )}
              {ftpStatus.data.running && (ftpStatus.data.ports?.length ?? 0) > 0 && (
                <span className="ml-2 font-mono text-xs">
                  {ftpStatus.data.local_ip ? `ftp://${ftpStatus.data.local_ip}` : "ftp://本机IP"}
                  {ftpStatus.data.ports!.map((p) => `:${p}`).join("、")}
                </span>
              )}
              {ftpStatus.data.running && (
                <span className="ml-2 text-xs text-muted">在线会话 {ftpStatus.data.sessions}</span>
              )}
            </p>
            <p className="text-xs text-muted">
              监听站点 {ftpStatus.data.listeners} 个 · 用户 {ftpStatus.data.users} 个 · 被动端口
              {" "}{ftpStatus.data.passive_port_start}-{ftpStatus.data.passive_port_end}
              {ftpStatus.data.passive_port_start === 0 && "（临时）"}
            </p>
            {ftpStatus.data.last_error && (
              <p className="text-xs text-err">最近错误：{ftpStatus.data.last_error}</p>
            )}
            <p className="text-xs text-muted">
              局域网访问需在 Windows 防火墙放行对应端口；修改用户/权限后对新登录即时生效
            </p>
          </div>
        ) : (
          <p className="text-sm text-muted">状态加载中…</p>
        )}
      </section>

      {/* ===== FTP 站点（多监听器：每端口一个根目录）===== */}
      <section className="space-y-3 rounded-xl border border-surface-3 bg-surface-2 p-5">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">FTP 站点（监听端口 → 根目录）</h2>
          <button
            className={btnSmall}
            onClick={() => {
              const base = draft ?? ftp;
              setDraft({
                ...base,
                listeners: [
                  ...base.listeners,
                  { name: `站点${base.listeners.length + 1}`, port: 21, root: "", enabled: true },
                ],
              });
            }}
          >
            添加站点
          </button>
        </div>
        {!draft || draft.listeners.length === 0 ? (
          <p className="text-xs text-muted">尚未配置站点。每个站点 = 一个控制端口 + 一个根目录，用户登录后被限制在对应根目录内。</p>
        ) : (
          <div className="space-y-2">
            {draft.listeners.map((l, i) => (
              <div
                key={i}
                className="flex flex-wrap items-center gap-2 rounded-lg border border-surface-3 p-2"
              >
                <input
                  className={`${inputCls} w-32`}
                  placeholder="站点名称"
                  value={l.name}
                  onChange={(e) => {
                    const ls = [...draft.listeners];
                    ls[i] = { ...l, name: e.target.value };
                    setDraft({ ...draft, listeners: ls });
                  }}
                />
                <input
                  className={`${inputCls} w-24 font-mono`}
                  type="number"
                  min={1}
                  max={65535}
                  placeholder="端口"
                  value={l.port}
                  onChange={(e) => {
                    const ls = [...draft.listeners];
                    ls[i] = { ...l, port: Number(e.target.value) || 0 };
                    setDraft({ ...draft, listeners: ls });
                  }}
                />
                <input
                  className={`${inputCls} flex-1 font-mono text-xs`}
                  placeholder="根目录（如 D:\\ftp）"
                  value={l.root}
                  onChange={(e) => {
                    const ls = [...draft.listeners];
                    ls[i] = { ...l, root: e.target.value };
                    setDraft({ ...draft, listeners: ls });
                  }}
                />
                <button
                  className={btnSmall}
                  onClick={async () => {
                    const picked = await open({ directory: true, title: "选择站点根目录" });
                    if (typeof picked === "string") {
                      const ls = [...draft.listeners];
                      ls[i] = { ...l, root: picked };
                      setDraft({ ...draft, listeners: ls });
                    }
                  }}
                >
                  选择目录
                </button>
                <label className="flex items-center gap-1 text-xs">
                  <input
                    type="checkbox"
                    className={checkCls}
                    checked={l.enabled}
                    onChange={(e) => {
                      const ls = [...draft.listeners];
                      ls[i] = { ...l, enabled: e.target.checked };
                      setDraft({ ...draft, listeners: ls });
                    }}
                  />
                  启用
                </label>
                <button
                  className={btnSmall}
                  onClick={() =>
                    setDraft({ ...draft, listeners: draft.listeners.filter((_, j) => j !== i) })
                  }
                >
                  删除
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="flex items-end gap-3">
          <div>
            <p className="mb-1 text-xs text-muted">被动模式端口区间（数据连接用）</p>
            <div className="flex items-center gap-2">
              <input
                className={`${inputCls} w-28 font-mono`}
                type="number"
                min={0}
                max={65535}
                value={draft?.passive_port_start ?? 50000}
                onChange={(e) =>
                  draft &&
                  setDraft({ ...draft, passive_port_start: Number(e.target.value) || 0 })
                }
              />
              <span className="text-muted">–</span>
              <input
                className={`${inputCls} w-28 font-mono`}
                type="number"
                min={0}
                max={65535}
                value={draft?.passive_port_end ?? 50100}
                onChange={(e) =>
                  draft && setDraft({ ...draft, passive_port_end: Number(e.target.value) || 0 })
                }
              />
            </div>
          </div>
          <button className={btnPrimary} onClick={() => void saveDraft()}>
            保存站点设置
          </button>
        </div>
      </section>

      {/* ===== FTP 用户 ===== */}
      <section className="space-y-3 rounded-xl border border-surface-3 bg-surface-2 p-5">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">FTP 用户与权限</h2>
          <button
            className={btnSmall}
            onClick={() => {
              const username = window.prompt("新用户名（字母数字与 _.@-）：");
              if (!username) return;
              if (ftp.users.some((u) => u.username.toLowerCase() === username.toLowerCase())) {
                pushToast("err", "用户名已存在");
                return;
              }
              const password = window.prompt(`设置 ${username} 的登录密码：`);
              if (password == null || password === "") return;
              // 默认授权全部站点根（未授权任何目录的用户无法登录）
              void saveUsers(
                [
                  ...ftp.users,
                  {
                    username,
                    allowed_roots: listenerRoots.map((r) => r.root),
                    permissions: {
                      list: true,
                      download: true,
                      upload: false,
                      delete: false,
                      rename: false,
                      mkdir: false,
                    },
                    enabled: true,
                  },
                ],
                { username, password },
                `用户 ${username} 已添加`,
              );
            }}
          >
            添加用户
          </button>
        </div>
        {ftp.users.length === 0 ? (
          <p className="text-xs text-muted">暂无用户。用户须被授权至少一个站点根目录才能登录。</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-surface-3 text-left text-xs text-muted">
                <th className="py-2 pr-3 font-medium">用户名</th>
                <th className="py-2 pr-3 font-medium">授权目录</th>
                <th className="py-2 pr-3 font-medium">权限</th>
                <th className="py-2 pr-3 font-medium">状态</th>
                <th className="py-2 font-medium">操作</th>
              </tr>
            </thead>
            <tbody>
              {ftp.users.map((u) => (
                <tr key={u.username} className="border-b border-surface-3/50 last:border-0">
                  <td className="py-2 pr-3 font-mono text-xs">{u.username}</td>
                  <td className="max-w-40 truncate py-2 pr-3 font-mono text-xs text-muted" title={u.allowed_roots.join("\n")}>
                    {u.allowed_roots.length === 0
                      ? "未授权（无法登录）"
                      : u.allowed_roots.join("、")}
                  </td>
                  <td className="py-2 pr-3 text-xs">
                    <span className="text-muted">
                      {PERMISSION_ITEMS.filter((p) => u.permissions[p.key]).map((p) => p.label).join("·") || "无"}
                    </span>
                  </td>
                  <td className="py-2 pr-3 text-xs">
                    {u.enabled ? <span className="text-ok">启用</span> : <span className="text-muted">停用</span>}
                  </td>
                  <td className="flex gap-1 py-2">
                    <FtpUserEditButton
                      user={u}
                      listenerRoots={listenerRoots.map((r) => r.root)}
                      onSave={(next, pwd) =>
                        void saveUsers(
                          ftp.users.map((x) => (x.username === u.username ? next : x)),
                          pwd ? { username: u.username, password: pwd } : undefined,
                        )
                      }
                      onDelete={() => {
                        if (!window.confirm(`确定删除用户 ${u.username}？`)) return;
                        void saveUsers(
                          ftp.users.filter((x) => x.username !== u.username),
                          undefined,
                          `用户 ${u.username} 已删除`,
                        );
                      }}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {/* ===== 规划中的工具 ===== */}
      <section className="space-y-3 rounded-xl border border-surface-3 bg-surface-2 p-5">
        <h2 className="text-sm font-semibold">更多工具（规划中）</h2>
        <div className="flex items-center justify-between rounded-lg border border-dashed border-surface-3 p-4 opacity-60">
          <div>
            <p className="text-sm font-medium">TCP 调试助手</p>
            <p className="text-xs text-muted">TCP/UDP 连通性测试与报文收发调试，后续版本上线</p>
          </div>
          <span className="text-xs text-muted">规划中</span>
        </div>
      </section>
    </div>
  );
}

/** 用户编辑入口按钮 + 弹窗（单独组件避免污染主组件 hooks） */

function FtpUserEditButton({
  user,
  listenerRoots,
  onSave,
  onDelete,
}: {
  user: FtpUser;
  listenerRoots: string[];
  onSave: (next: FtpUser, password?: string) => void;
  onDelete: () => void;
}) {
  const [openDlg, setOpenDlg] = useState(false);
  return (
    <>
      <button className={btnSmall} onClick={() => setOpenDlg(true)}>
        编辑
      </button>
      <button className={btnSmall} onClick={onDelete}>
        删除
      </button>
      {openDlg && (
        <FtpUserDialog
          user={user}
          listenerRoots={listenerRoots}
          onClose={() => setOpenDlg(false)}
          onSubmit={(next, pwd) => {
            setOpenDlg(false);
            onSave(next, pwd);
          }}
        />
      )}
    </>
  );
}

/** 用户编辑弹窗：密码留空 = 不修改；授权目录 = 站点根勾选 + 自定义目录 */
function FtpUserDialog({
  user,
  listenerRoots,
  onClose,
  onSubmit,
}: {
  user: FtpUser;
  listenerRoots: string[];
  onClose: () => void;
  onSubmit: (next: FtpUser, password?: string) => void;
}) {
  const [username, setUsername] = useState(user.username);
  const [password, setPassword] = useState("");
  const [enabled, setEnabled] = useState(user.enabled);
  const [roots, setRoots] = useState<string[]>(user.allowed_roots);
  const [perms, setPerms] = useState<FtpPermissions>(user.permissions);
  const [err, setErr] = useState("");

  const toggleRoot = (root: string) =>
    setRoots((rs) => (rs.includes(root) ? rs.filter((r) => r !== root) : [...rs, root]));

  const submit = () => {
    if (!username.trim()) return setErr("用户名不能为空");
    if (!/^[A-Za-z0-9_.@-]{1,64}$/.test(username)) return setErr("用户名限 1-64 位字母数字与 _.@-");
    if (roots.length === 0) return setErr("至少授权一个目录，否则用户无法登录");
    onSubmit(
      { username: username.trim(), allowed_roots: roots, permissions: perms, enabled },
      password === "" ? undefined : password,
    );
  };

  return (
    <div className={overlayCls} onClick={onClose}>
      <div className={dialogCls} onClick={(e) => e.stopPropagation()}>
        <h3 className="text-sm font-semibold">编辑用户：{user.username}</h3>

        <div className="grid grid-cols-2 gap-3">
          <label className="space-y-1 text-xs text-muted">
            用户名
            <input className={inputCls} value={username} onChange={(e) => setUsername(e.target.value)} />
          </label>
          <label className="space-y-1 text-xs text-muted">
            重置密码（留空 = 不修改）
            <input
              className={inputCls}
              type="password"
              placeholder="••••••"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
        </div>

        <div className="space-y-1">
          <p className="text-xs text-muted">授权目录（勾选站点根；其余授权目录将挂载为虚拟子目录）</p>
          {listenerRoots.map((root) => (
            <label key={root} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                className={checkCls}
                checked={roots.includes(root)}
                onChange={() => toggleRoot(root)}
              />
              <span className="font-mono text-xs">站点根：{root}</span>
            </label>
          ))}
          {roots
            .filter((r) => !listenerRoots.includes(r))
            .map((root) => (
              <label key={root} className="flex items-center gap-2 text-sm">
                <input type="checkbox" className={checkCls} checked onChange={() => toggleRoot(root)} />
                <span className="font-mono text-xs">自定义：{root}</span>
              </label>
            ))}
          <button
            className={btnSmall}
            onClick={async () => {
              const picked = await open({ directory: true, title: "添加授权目录" });
              if (typeof picked === "string" && !roots.includes(picked)) setRoots([...roots, picked]);
            }}
          >
            添加自定义目录
          </button>
        </div>

        <div className="space-y-1">
          <p className="text-xs text-muted">操作权限</p>
          <div className="grid grid-cols-3 gap-2">
            {PERMISSION_ITEMS.map((p) => (
              <label key={p.key} className="flex items-center gap-1.5 text-sm">
                <input
                  type="checkbox"
                  className={checkCls}
                  checked={perms[p.key]}
                  onChange={(e) => setPerms({ ...perms, [p.key]: e.target.checked })}
                />
                {p.label}
              </label>
            ))}
          </div>
        </div>

        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" className={checkCls} checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          启用该用户
        </label>

        {err && <p className="text-xs text-err">{err}</p>}

        <div className="flex justify-end gap-2">
          <button className={btnSecondary} onClick={onClose}>
            取消
          </button>
          <button className={btnPrimary} onClick={submit}>
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
