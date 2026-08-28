// 参数编辑器：有序数组、启用开关、上移/下移排序（排序影响 CLI 语义，禁用自动排序）
import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-react";
import type { Arg, ArgKind } from "../../shared/rpc/schema";

export function ArgsEditor({
  args,
  onChange,
}: {
  args: Arg[];
  onChange: (next: Arg[]) => void;
}) {
  const update = (index: number, patch: Partial<Arg>) => {
    onChange(args.map((a, i) => (i === index ? { ...a, ...patch } : a)));
  };
  const move = (index: number, target: number) => {
    if (target < 0 || target >= args.length) return;
    const next = [...args];
    [next[index], next[target]] = [next[target], next[index]];
    onChange(next);
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted">
          参数按顺序传给程序；取消勾选的参数不生效但保留配置
        </p>
        <button
          type="button"
          onClick={() =>
            onChange([
              ...args,
              {
                id: crypto.randomUUID(),
                key: "",
                value: null,
                enabled: true,
                kind: "option" as ArgKind,
                description: "",
              },
            ])
          }
          className="inline-flex min-h-8 items-center gap-1 rounded-md border border-surface-3 px-2 text-xs hover:bg-surface-3"
        >
          <Plus size={13} aria-hidden /> 添加参数
        </button>
      </div>

      {args.length === 0 && (
        <p className="rounded-lg border border-dashed border-surface-3 px-3 py-4 text-center text-xs text-muted">
          无参数
        </p>
      )}

      <ul className="space-y-2">
        {args.map((arg, index) => (
          <li
            key={arg.id}
            className="flex items-center gap-2 rounded-lg border border-surface-3 bg-surface px-2 py-1.5"
          >
            {/* 启用开关 */}
            <input
              type="checkbox"
              checked={arg.enabled}
              onChange={(e) => update(index, { enabled: e.target.checked })}
              aria-label={`参数 ${index + 1} 启用`}
              className="size-4 accent-[rgb(var(--accent))]"
            />

            {/* 类型 */}
            <select
              aria-label={`参数 ${index + 1} 类型`}
              value={arg.kind}
              onChange={(e) =>
                update(index, {
                  kind: e.target.value as ArgKind,
                  // flag 类型无值
                  value: e.target.value === "flag" ? null : arg.value,
                })
              }
              className="h-8 rounded-md border border-surface-3 bg-surface-2 px-1 text-xs"
            >
              <option value="option">选项</option>
              <option value="flag">开关</option>
              <option value="positional">位置参数</option>
            </select>

            <input
              aria-label={`参数 ${index + 1} 键`}
              placeholder={arg.kind === "positional" ? "（无键）" : "键（--xxx）"}
              disabled={arg.kind === "positional"}
              value={arg.key}
              onChange={(e) => update(index, { key: e.target.value })}
              className="h-8 w-32 rounded-md border border-surface-3 bg-surface-2 px-2 font-mono text-xs disabled:opacity-40"
            />
            <input
              aria-label={`参数 ${index + 1} 值`}
              placeholder={arg.kind === "flag" ? "（开关无值）" : "值（可为空）"}
              disabled={arg.kind === "flag"}
              value={arg.value ?? ""}
              onChange={(e) => update(index, { value: e.target.value })}
              className="h-8 min-w-0 flex-1 rounded-md border border-surface-3 bg-surface-2 px-2 font-mono text-xs disabled:opacity-40"
            />

            {/* 排序：上移/下移（顺序即 CLI 语义） */}
            <button
              type="button"
              aria-label={`参数 ${index + 1} 上移`}
              disabled={index === 0}
              onClick={() => move(index, index - 1)}
              className="inline-flex size-7 items-center justify-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
            >
              <ArrowUp size={13} aria-hidden />
            </button>
            <button
              type="button"
              aria-label={`参数 ${index + 1} 下移`}
              disabled={index === args.length - 1}
              onClick={() => move(index, index + 1)}
              className="inline-flex size-7 items-center justify-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
            >
              <ArrowDown size={13} aria-hidden />
            </button>
            <button
              type="button"
              aria-label={`删除参数 ${index + 1}`}
              onClick={() => {
                if (window.confirm(`删除参数 ${index + 1}？`)) {
                  onChange(args.filter((_, i) => i !== index));
                }
              }}
              className="inline-flex size-7 items-center justify-center rounded text-muted hover:bg-err/10 hover:text-err"
            >
              <Trash2 size={13} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
