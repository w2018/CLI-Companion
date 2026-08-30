// 内嵌终端（xterm.js + daemon ConPTY）
//
// 核心设计：**终端实例常驻内存，页面只是它的"临时显示窗口"**。
// Terminal 实例存放在模块级 sessions 表中，离开页面只把 DOM 摘下来，
// 重进时把同一个 DOM 挂回去——屏幕/光标/滚动/选区零丢失，几何从不变化。
//
// v2.3.1 关键修复：必须引入 @xterm/xterm/css/xterm.css。xterm 6 的滚动容器、
// 选区遮罩层、隐藏 textarea 全部依赖该样式表定位，此前从未引入导致：
//   ① helper textarea 按浏览器默认样式占约 2 行文档流高度，文字行整体下移，
//      底部输入行被容器裁掉（"终端高度超过框框"）；
//   ② 选区遮罩层本应相对 .xterm 定位，却退而相对页面外层盒子定位，与下移后
//      的文字错开约 2 行（"选第 1 行、第 3 行亮"）；
//   ③ 选中文字需被提升到不透明遮罩之上的规则同样缺失，选区文字被遮罩盖住。
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams, Link } from "react-router-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useServices } from "../../shared/hooks/useDaemon";
import { ConfirmDialog } from "../../shared/components/ConfirmDialog";

/** 内置主题：选区配色 = 前景/背景互换（相反色），保证选区文字任何情况下可见 */
const THEMES = {
  solarizedDark: {
    label: "Solarized Dark（默认）",
    background: "#002b36",
    foreground: "#93a1a1",
    cursor: "#93a1a1",
    cursorAccent: "#002b36",
    selectionBackground: "#93a1a1",
    selectionForeground: "#002b36",
    selectionInactiveBackground: "#0e4449",
  },
  dark: {
    label: "深色",
    background: "#101418",
    foreground: "#d6dae0",
    cursor: "#5fb3ff",
    cursorAccent: "#101418",
    selectionBackground: "#d6dae0",
    selectionForeground: "#101418",
    selectionInactiveBackground: "#4a545e",
  },
  solarizedLight: {
    label: "Solarized Light",
    background: "#fdf6e3",
    foreground: "#586e75",
    cursor: "#657b83",
    cursorAccent: "#fdf6e3",
    selectionBackground: "#586e75",
    selectionForeground: "#fdf6e3",
    selectionInactiveBackground: "#eee8d5",
  },
  light: {
    label: "浅色",
    background: "#ffffff",
    foreground: "#24292f",
    cursor: "#1f6feb",
    cursorAccent: "#ffffff",
    selectionBackground: "#24292f",
    selectionForeground: "#ffffff",
    selectionInactiveBackground: "#d0d7de",
  },
  hicon: {
    label: "高对比黑",
    background: "#000000",
    foreground: "#eaeaea",
    cursor: "#ffffff",
    cursorAccent: "#000000",
    selectionBackground: "#eaeaea",
    selectionForeground: "#000000",
    selectionInactiveBackground: "#555555",
  },
  green: {
    label: "经典绿屏",
    background: "#0b1000",
    foreground: "#33ff66",
    cursor: "#33ff66",
    cursorAccent: "#0b1000",
    selectionBackground: "#33ff66",
    selectionForeground: "#0b1000",
    selectionInactiveBackground: "#1e5c31",
  },
} as const;

type ThemeKey = keyof typeof THEMES;

const THEME_STORE_KEY = "cc-term-theme";
const WRAP_STORE_KEY = "cc-term-wrap";

function loadStored<T extends string>(key: string, fallback: T, valid: readonly T[]): T {
  try {
    const v = localStorage.getItem(key) as T | null;
    return v && valid.includes(v) ? v : fallback;
  } catch {
    return fallback;
  }
}

/** 常驻终端会话：生命周期独立于页面挂载 */
interface TerminalSession {
  term: Terminal;
  fit: FitAddon;
  ptyId: number | null;
  exited: boolean;
  /** 连接完成后的解绑函数（只在创建时接线一次） */
  unlisten: (() => void)[];
  connecting: Promise<void> | null;
}

/** 模块级会话表：key = 服务 ID。页面进出只增删 DOM，不销毁实例 */
const sessions = new Map<string, TerminalSession>();

export function EmbeddedTerminal() {
  const { serviceId = "" } = useParams();
  const navigate = useNavigate();
  const services = useServices();
  const serviceName = services.data?.find((r) => r.service.id === serviceId)?.service.name ?? "";
  const frameRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const ptyIdRef = useRef<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [themeKey, setThemeKey] = useState<ThemeKey>(() =>
    loadStored<ThemeKey>(THEME_STORE_KEY, "solarizedDark", Object.keys(THEMES) as ThemeKey[]),
  );
  const [wrap, setWrap] = useState<boolean>(() => {
    try {
      return localStorage.getItem(WRAP_STORE_KEY) !== "0";
    } catch {
      return true;
    }
  });
  // 右键菜单：坐标相对终端框；canCopy 记录打开那一刻是否存在选区
  const [menu, setMenu] = useState<{ x: number; y: number; canCopy: boolean } | null>(null);
  // 多行粘贴确认：待写入的原始文本 + 行数
  const [pasteAsk, setPasteAsk] = useState<{ text: string; lines: number } | null>(null);

  // ===== 挂载/重挂载：把会话终端接到页面 DOM 上（不新建、不销毁） =====
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    let disposed = false;
    const cleanups: (() => void)[] = [];

    let sess = sessions.get(serviceId);
    if (sess) {
      // 重挂载：把既有终端 DOM 搬回来（屏幕状态完整保留）
      if (sess.term.element && sess.term.element.parentElement !== box) {
        box.appendChild(sess.term.element);
        sess.term.scrollToBottom();
      }
      ptyIdRef.current = sess.ptyId;
      if (sess.ptyId != null && !sess.exited) setReady(true);
    } else {
      // 首次进入：创建终端实例并连接 PTY（之后常驻）
      const theme = THEMES[themeKey];
      const term = new Terminal({
        fontSize: 12,
        cursorBlink: true,
        fontFamily: "Consolas, 'Courier New', monospace",
        scrollback: 5000,
        theme: { ...theme },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(box);
      sess = { term, fit, ptyId: null, exited: false, unlisten: [], connecting: null };
      sessions.set(serviceId, sess);
      const session: TerminalSession = sess; // 闭包内使用的稳定引用（TS 收窄丢失）

      session.connecting = (async () => {
        // 行列数用 xterm 首次 fit 后的实际值——PTY 与 UI 几何一致是回显正确的前提
        const id = await invoke<number>("pty_open", {
          serviceId,
          rows: term.rows,
          cols: term.cols,
        });
        if (disposed) return; // 整页卸载：会话仍保留
        session.ptyId = id;
        ptyIdRef.current = id;
        setReady(true);

        const offOut = await listen<string>(`pty-output:${id}`, (e) => {
          term.write(e.payload);
        });
        const offExit = await listen(`pty-exit:${id}`, () => {
          session.exited = true;
          term.writeln("\r\n\x1b[90m[会话已结束]\x1b[0m");
        });
        const dataSub = term.onData((d) => {
          void invoke("pty_write_cmd", { id, data: d }).catch(() => {});
          term.scrollToBottom(); // 输入时确保输入行可见
        });
        const resizeSub = term.onResize(({ rows, cols }) => {
          void invoke("pty_resize_cmd", { id, rows, cols }).catch(() => {});
        });
        session.unlisten.push(
          () => offOut(),
          () => offExit(),
          () => dataSub.dispose(),
          () => resizeSub.dispose(),
        );

        // 自动换行初始状态同步给刚创建的终端
        term.write(wrap ? "\x1b[?7h" : "\x1b[?7l");
      })().catch((e) => {
        setError(String(e));
        term.writeln(`\x1b[31m终端启动失败: ${String(e)}\x1b[0m`);
      });
    }

    // ===== 几何校准：rAF + 延迟 + ResizeObserver 三重触发 =====
    // 终端容器零内边距（padding 会让 FitAddon 算出大于可视区的行列）
    const doFit = () => {
      if (disposed) return;
      try {
        sess.fit.fit();
        // 兜底：行区实际渲染高度若仍超出容器（字体/DPI 取整偏差），下一帧回退一行，
        // 确保底部输入行永远可见。rAF 等一帧是为了读到 resize 后的真实 DOM 高度
        requestAnimationFrame(() => {
          if (disposed) return;
          const host = boxRef.current;
          const rowsEl = sess.term.element?.querySelector<HTMLElement>(".xterm-rows");
          if (
            host &&
            rowsEl &&
            sess.term.rows > 2 &&
            rowsEl.getBoundingClientRect().height > host.clientHeight + 0.5
          ) {
            sess.term.resize(sess.term.rows - 1, sess.term.cols);
          }
        });
        if (sess.ptyId != null) {
          void invoke("pty_resize_cmd", {
            id: sess.ptyId,
            rows: sess.term.rows,
            cols: sess.term.cols,
          }).catch(() => {});
        }
      } catch {
        // 容器尺寸为 0（布局未完成）时忽略，下一轮再校
      }
    };
    const raf = requestAnimationFrame(doFit);
    const timer = setTimeout(doFit, 200);
    const ro = new ResizeObserver(() => doFit());
    ro.observe(box);
    cleanups.push(() => {
      cancelAnimationFrame(raf);
      clearTimeout(timer);
      ro.disconnect();
    });

    return () => {
      disposed = true;
      cleanups.forEach((f) => f());
      // 会话保持：卸载只摘 DOM，不终止 PTY、不销毁终端实例
    };
    // themeKey/wrap 不参与此依赖：二者走独立 effect 热应用，避免重建会话
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serviceId]);

  // ===== 右键菜单：选中文本后右键 → 复制/粘贴（旧"左选即复制/右键直粘贴"已移除） =====
  const openMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const frame = frameRef.current;
    if (!frame) return;
    const rect = frame.getBoundingClientRect();
    setMenu({
      x: e.clientX - rect.left,
      y: e.clientY - rect.top,
      canCopy: !!sessions.get(serviceId)?.term.hasSelection(),
    });
  };

  useEffect(() => {
    if (!menu) return;
    const close = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest("[data-term-menu]")) setMenu(null);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    document.addEventListener("mousedown", close, true);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close, true);
      document.removeEventListener("keydown", esc);
    };
  }, [menu]);

  const writeToPty = (data: string) => {
    const sess = sessions.get(serviceId);
    if (sess?.ptyId != null) {
      void invoke("pty_write_cmd", { id: sess.ptyId, data }).catch(() => {});
      sess.term.scrollToBottom();
    }
  };

  const doCopy = () => {
    setMenu(null);
    const sel = sessions.get(serviceId)?.term.getSelection();
    if (sel) void invoke("copy_to_clipboard", { text: sel }).catch(() => {});
    sessions.get(serviceId)?.term.clearSelection();
  };

  const doPaste = () => {
    setMenu(null);
    void invoke<string>("read_clipboard")
      .then((text) => {
        if (!text) return;
        // 行数按去掉尾部换行后的实际行算；多行必须经确认，防误粘误执行
        const lines = text.replace(/[\r\n]+$/, "").split(/\r\n|\r|\n/).length;
        if (lines > 1) {
          setPasteAsk({ text, lines });
        } else {
          writeToPty(text.replace(/\r\n|\r|\n/g, "\r"));
        }
      })
      .catch(() => {});
  };

  // ===== 主题切换（热应用到当前会话终端）=====
  useEffect(() => {
    const theme = THEMES[themeKey];
    const sess = serviceId ? sessions.get(serviceId) : undefined;
    if (sess) {
      sess.term.options.theme = { ...theme };
    }
    if (boxRef.current) {
      boxRef.current.style.background = theme.background;
    }
    try {
      localStorage.setItem(THEME_STORE_KEY, themeKey);
    } catch {
      // 存储不可用时仅本次生效
    }
  }, [themeKey, serviceId]);

  // ===== 自动换行（DECAWM）：开启 \x1b[?7h，关闭 \x1b[?7l =====
  useEffect(() => {
    const sess = serviceId ? sessions.get(serviceId) : undefined;
    sess?.term.write(wrap ? "\x1b[?7h" : "\x1b[?7l");
    try {
      localStorage.setItem(WRAP_STORE_KEY, wrap ? "1" : "0");
    } catch {
      // 存储不可用时仅本次生效
    }
  }, [wrap, serviceId]);

  /** 主动关闭会话：终止 PTY、销毁终端实例并移出会话表，返回服务列表 */
  const closeSession = async () => {
    const sess = sessions.get(serviceId);
    if (sess) {
      if (sess.ptyId != null) {
        try {
          await invoke("pty_close_cmd", { id: sess.ptyId });
        } catch {
          // 会话可能已随子进程退出被清理
        }
      }
      sess.unlisten.forEach((f) => f());
      sess.term.dispose();
      sessions.delete(serviceId);
    }
    ptyIdRef.current = null;
    navigate("/services");
  };

  return (
    <div className="mx-auto flex h-[calc(100vh-96px)] max-w-6xl flex-col gap-3">
      <header className="flex flex-wrap items-center gap-3">
        <Link
          to="/services"
          aria-label="返回服务列表（会话保持）"
          className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
        >
          <ArrowLeft size={16} aria-hidden />
        </Link>
        <h1 className="text-lg font-semibold">
          内嵌终端{serviceName ? ` · ${serviceName}` : ""}
        </h1>
        <span className="text-xs text-muted">
          {ready ? "已连接 · 返回页面会话保持，重新进入继续" : "连接中…"}
        </span>

        <div className="ml-auto flex flex-wrap items-center gap-3 text-xs text-muted">
          <label className="flex items-center gap-1.5">
            主题
            <select
              value={themeKey}
              onChange={(e) => setThemeKey(e.target.value as ThemeKey)}
              className="h-8 rounded-lg border border-surface-3 bg-surface px-2 text-xs focus:border-accent focus:outline-none"
            >
              {(Object.keys(THEMES) as ThemeKey[]).map((k) => (
                <option key={k} value={k}>
                  {THEMES[k].label}
                </option>
              ))}
            </select>
          </label>
          <label
            className="flex cursor-pointer items-center gap-1.5"
            title="关闭后长行不折行（DEC 自动换行模式切换）"
          >
            <input
              type="checkbox"
              className="size-3.5 accent-[rgb(var(--accent))]"
              checked={wrap}
              onChange={(e) => setWrap(e.target.checked)}
            />
            自动换行
          </label>
          {ready && (
            <button
              onClick={() => void closeSession()}
              className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-err hover:bg-err/10"
            >
              <X size={13} aria-hidden /> 关闭会话
            </button>
          )}
        </div>
      </header>

      {error && (
        <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
          {error}
        </p>
      )}

      <div
        ref={frameRef}
        aria-label="内嵌终端内容"
        className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-surface-3"
        style={{ background: THEMES[themeKey].background }}
        onContextMenu={openMenu}
      >
        {/* xterm 挂载点：absolute inset-0 保证 FitAddon 拿到的几何精确（零内边距） */}
        <div ref={boxRef} className="absolute inset-0" />

        {menu && (
          <div
            data-term-menu
            role="menu"
            aria-label="终端操作菜单"
            className="absolute z-20 min-w-36 overflow-hidden rounded-lg border border-surface-3 bg-surface-2 py-1 shadow-lg"
            style={{
              left: Math.max(0, Math.min(menu.x, (frameRef.current?.clientWidth ?? 0) - 156)),
              top: Math.max(0, Math.min(menu.y, (frameRef.current?.clientHeight ?? 0) - 96)),
            }}
          >
            <button
              role="menuitem"
              disabled={!menu.canCopy}
              onClick={doCopy}
              className="block w-full px-3 py-2 text-left text-sm text-content hover:bg-surface-3 disabled:cursor-not-allowed disabled:text-muted disabled:hover:bg-transparent"
            >
              复制
            </button>
            <button
              role="menuitem"
              onClick={doPaste}
              className="block w-full px-3 py-2 text-left text-sm text-content hover:bg-surface-3"
            >
              粘贴
            </button>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={pasteAsk != null}
        title="粘贴多行内容"
        message={`即将向终端粘贴 ${pasteAsk?.lines ?? 0} 行内容，多行内容会逐行发送并可能被逐行执行，确认继续？`}
        actions={[
          { key: "cancel", label: "取消" },
          { key: "ok", label: "确认粘贴", danger: true },
        ]}
        onAction={(key) => {
          if (key === "ok" && pasteAsk) {
            writeToPty(pasteAsk.text.replace(/\r\n|\r|\n/g, "\r"));
          }
          setPasteAsk(null);
        }}
      />

      <p className="text-xs text-muted">
        选中文本后右键选择复制/粘贴，粘贴多行内容前会提示确认 · 会话以服务配置的环境变量与工作目录运行 ·
        返回页面会话保持，仅「关闭会话」终止并重置。
      </p>
    </div>
  );
}
