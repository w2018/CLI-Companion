// 守护进程日志页：tail 轮询 + 等级筛选/着色 + 自动滚动（role="log"）+ 清理 daemon.log
// 数据源：daemon.logs RPC（读取 <数据目录>/logs/daemon.log，即 daemon 自身运行日志）
// 行格式（tracing 默认）：2026-08-28T10:27:20.816717Z  INFO 目标: 消息
import { useEffect, useMemo, useRef, useState } from "react";
import { Trash2 } from "lucide-react";
import { rpc } from "../../shared/rpc/client";
import { describeError } from "../../shared/rpc/errors";
import { useUiStore } from "../../stores/uiStore";

interface LogsResult {
  lines: string[];
  total: number;
}

type Level = "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE";

/** 严重度数值（越小越严重，用于"至少显示到某等级"的阈值过滤） */
const LEVEL_ORDER: Record<Level, number> = {
  ERROR: 0,
  WARN: 1,
  INFO: 2,
  DEBUG: 3,
  TRACE: 4,
};

const LEVEL_TEXT_CLS: Record<Level, string> = {
  ERROR: "text-err",
  WARN: "text-warn",
  INFO: "text-content",
  DEBUG: "text-muted",
  TRACE: "text-muted",
};

const LEVEL_BADGE_CLS: Record<Level, string> = {
  ERROR: "border-err/40 bg-err/10 text-err",
  WARN: "border-warn/40 bg-warn/10 text-warn",
  INFO: "border-accent/40 bg-accent/10 text-accent",
  DEBUG: "border-surface-3 bg-surface-3 text-muted",
  TRACE: "border-surface-3 bg-surface-3 text-muted",
};

interface ParsedLine {
  raw: string;
  time: string;
  level: Level | null;
  rest: string;
}

/** 解析 tracing 行：时间戳本地化 + 提取等级；不匹配的行原样保留 */
function parseLine(line: string): ParsedLine {
  const m = /^(\S+)\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)$/.exec(line);
  if (!m) return { raw: line, time: "", level: null, rest: line };
  const d = new Date(m[1]);
  const time = Number.isNaN(d.getTime())
    ? m[1]
    : d.toLocaleString("zh-CN", { hour12: false });
  return { raw: line, time, level: m[2] as Level, rest: m[3] };
}

export function DaemonLogPage() {
  const [logs, setLogs] = useState<LogsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  // v2.3.0：自动换行开关（勾选=长行折行显示；不勾=单行横向滚动）
  const [wrap, setWrap] = useState(true);
  // 等级阈值筛选：ALL 显示全部；选某等级显示"该等级及更严重"
  const [minLevel, setMinLevel] = useState<"ALL" | Level>("ALL");
  const boxRef = useRef<HTMLDivElement>(null);
  const pushToast = useUiStore((s) => s.pushToast);

  const clearLogs = async () => {
    if (!window.confirm("确定清空守护进程日志吗？此操作会删除 daemon.log 中的全部记录。")) {
      return;
    }
    try {
      await rpc("daemon.logs.clear");
      setLogs({ lines: [], total: 0 });
      pushToast("ok", "守护进程日志已清空");
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // 轮询日志（2s）
  useEffect(() => {
    let alive = true;
    const fetchLogs = async () => {
      try {
        const r = await rpc<LogsResult>("daemon.logs", { tail: 2000 });
        if (alive) {
          setLogs(r);
          setError(null);
        }
      } catch (e) {
        if (alive) setError(describeError(e as never));
      }
    };
    void fetchLogs();
    const timer = setInterval(fetchLogs, 2000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  // 解析 + 按等级阈值过滤
  const visible = useMemo(() => {
    const parsed = (logs?.lines ?? []).map(parseLine);
    if (minLevel === "ALL") return parsed;
    return parsed.filter(
      (l) => l.level !== null && LEVEL_ORDER[l.level] <= LEVEL_ORDER[minLevel],
    );
  }, [logs, minLevel]);

  // 自动滚动到底部
  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [visible, autoScroll]);

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col gap-3">
      <header className="flex flex-wrap items-center gap-3">
        <h1 className="text-lg font-semibold">守护进程日志</h1>
        <p className="text-xs text-muted">daemon 自身的运行日志（连接、服务编排、同步调度）</p>
        <div className="ml-auto flex flex-wrap items-center gap-3">
          {/* 等级筛选：显示所选等级及更严重的日志 */}
          <label className="flex items-center gap-2 text-sm text-muted">
            等级
            <select
              value={minLevel}
              onChange={(e) => setMinLevel(e.target.value as "ALL" | Level)}
              className="min-h-9 rounded-lg border border-surface-3 bg-surface px-2 text-sm text-content"
            >
              <option value="ALL">全部</option>
              <option value="ERROR">ERROR 及以上</option>
              <option value="WARN">WARN 及以上</option>
              <option value="INFO">INFO 及以上</option>
              <option value="DEBUG">DEBUG 及以上</option>
            </select>
          </label>
          <label className="flex items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
              className="size-4 accent-[rgb(var(--accent))]"
            />
            自动滚动
          </label>
          <label className="flex items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              checked={wrap}
              onChange={(e) => setWrap(e.target.checked)}
              className="size-4 accent-[rgb(var(--accent))]"
            />
            自动换行
          </label>
          <button
            onClick={() => void clearLogs()}
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-sm text-err hover:bg-err/10"
          >
            <Trash2 size={14} aria-hidden /> 清空日志
          </button>
        </div>
      </header>

      {error && (
        <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
          {error}
        </p>
      )}

      <div
        ref={boxRef}
        role="log"
        aria-label="守护进程日志内容"
        onWheel={() => setAutoScroll(false)} // 用户滚动时暂停跟随
        className={`flex-1 overflow-auto rounded-xl border border-surface-3 bg-surface-2 p-4 font-mono text-xs leading-5 ${wrap ? "" : "overflow-x-auto"}`}
      >
        {visible.length > 0 ? (
          visible.map((l, i) =>
            l.level === null ? (
              <div key={i} className={wrap ? "whitespace-pre-wrap text-muted" : "whitespace-pre text-muted"}>
                {l.rest}
              </div>
            ) : (
              <div key={i} className={`flex items-start gap-2 ${wrap ? "whitespace-pre-wrap" : "whitespace-pre"}`}>
                <span className="shrink-0 text-muted">{l.time}</span>
                <span
                  className={`shrink-0 rounded border px-1 text-[10px] leading-4 ${LEVEL_BADGE_CLS[l.level]}`}
                >
                  {l.level}
                </span>
                <span className={LEVEL_TEXT_CLS[l.level]}>{l.rest}</span>
              </div>
            ),
          )
        ) : (
          <span className="text-muted">{logs ? "暂无符合条件的日志" : "加载中…"}</span>
        )}
      </div>

      {logs && (
        <p className="text-xs text-muted">
          共 {logs.total} 行，当前显示 {visible.length} 行
          {" · "}
          <button
            className="text-accent hover:underline"
            onClick={() => {
              setLogs({ lines: [], total: 0 });
              pushToast("info", "已清空显示（不影响日志文件）");
            }}
          >
            清空显示
          </button>
        </p>
      )}
    </div>
  );
}
