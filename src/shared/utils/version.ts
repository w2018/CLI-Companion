// 语义化版本比较（v2.3.0 修复：旧逻辑只判"不同"，旧版本也提示有更新）

export interface ParsedVersion {
  major: number;
  minor: number;
  patch: number;
}

/** 解析 "v1.2.3" / "1.2.3" 风格版本号；无法解析返回 null */
export function parseVersion(v: string): ParsedVersion | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(v.trim());
  return m
    ? { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]) }
    : null;
}

/** 判断 latest 是否严格大于 current；任一版本无法解析返回 null（交由人工判断） */
export function isNewerVersion(latest: string, current: string): boolean | null {
  const a = parseVersion(latest);
  const b = parseVersion(current);
  if (!a || !b) return null;
  if (a.major !== b.major) return a.major > b.major;
  if (a.minor !== b.minor) return a.minor > b.minor;
  return a.patch > b.patch;
}
