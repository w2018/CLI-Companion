// 内嵌终端（xterm.js + daemon ConPTY）—— v2.4.0 架构重构
//
// 核心设计：**终端实例常驻内存，页面只是它的"临时显示窗口"**。
// 旧方案每次进页面新建 Terminal 并回放字节流——回放的是历史几何下的 VT 差分，
// 注入全新 xterm 后屏幕必然错位（输入行跑出可视区），且依赖 PTY resize 链路。
// 新方案：Terminal 实例存放在模块级 sessions 表中，离开页面只把 DOM 摘下来，
// 重进时把同一个 DOM 挂回去——屏幕/光标/滚动/选区零丢失，几何从不变化。
// 会话保持、主题、复制粘贴、自动换行等既有想法全部保留。
import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams, Link } from "react-router-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useServices } from "../../shared/hooks/useDaemon";

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
        const selSub = term.onSelectionChange(() => {
          // 左键选中即复制（Rust 侧 arboard，绕开 WebView2 剪贴板限制）
          const sel = term.getSelection();
          if (sel) void invoke("copy_to_clipboard", { text: sel }).catch(() => {});
        });
        session.unlisten.push(
          () => offOut(),
          () => offExit(),
          () => dataSub.dispose(),
          () => resizeSub.dispose(),
          () => selSub.dispose(),
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

    // ===== 右键粘贴（按页面挂载绑定；左右键习惯：左选复制/右键粘贴） =====
    const pasteClipboard = () => {
      navigator.clipboard
        .readText()
        .then((t) => {
          if (t && sess.ptyId != null) {
            void invoke("pty_write_cmd", { id: sess.ptyId, data: t }).catch(() => {});
            sess.term.scrollToBottom();
          }
        })
        .catch(() => {});
    };
    const onCtx = (e: MouseEvent) => {
      e.preventDefault();
      pasteClipboard();
    };
    box.addEventListener("contextmenu", onCtx);
    cleanups.push(() => box.removeEventListener("contextmenu", onCtx));

    return () => {
      disposed = true;
      cleanups.forEach((f) => f());
      // 会话保持：卸载只摘 DOM，不终止 PTY、不销毁终端实例
    };
    // themeKey/wrap 不参与此依赖：二者走独立 effect 热应用，避免重建会话
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serviceId]);

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
        aria-label="内嵌终端内容"
        className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-surface-3"
        style={{ background: THEMES[themeKey].background }}
      >
        {/* xterm 挂载点：absolute inset-0 保证 FitAddon 拿到的几何精确（零内边距） */}
        <div ref={boxRef} className="absolute inset-0" />
      </div>
      <p className="text-xs text-muted">
        左键选中即复制，右键粘贴（Ctrl+Shift+C/V 同样可用）· 会话以服务配置的环境变量与工作目录运行 ·
        返回页面会话保持，仅「关闭会话」终止并重置。
      </p>
    </div>
  );
}
