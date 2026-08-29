// Windows 风格命令行解析：粘贴整条命令 → exe + 有序参数
// 分词遵循 MSVCRT 规则（双引号分组、反斜杠转义引号、^不处理），
// 分类为 option / flag / positional，供服务表单参数编辑器微调。

export interface ParsedArg {
  /** option 的键（如 --port）；flag 的键；positional 为空串 */
  key: string;
  value: string | null;
  kind: "option" | "flag" | "positional";
}

export interface ParsedCommand {
  exe: string;
  args: ParsedArg[];
}

/** 解析整条命令行；空输入返回 null */
export function parseCommandLine(input: string): ParsedCommand | null {
  const tokens = tokenize(input);
  if (tokens.length === 0) return null;
  const [exe, ...rest] = tokens;
  const args: ParsedArg[] = [];
  let i = 0;
  while (i < rest.length) {
    const tok = rest[i];
    if (tok.startsWith("-")) {
      // --key=value 形式拆成 option
      const eq = tok.indexOf("=");
      if (eq > 1) {
        args.push({ key: tok.slice(0, eq), value: tok.slice(eq + 1), kind: "option" });
        i += 1;
        continue;
      }
      const next = rest[i + 1];
      if (next !== undefined && !next.startsWith("-")) {
        args.push({ key: tok, value: next, kind: "option" });
        i += 2;
      } else {
        args.push({ key: tok, value: null, kind: "flag" });
        i += 1;
      }
    } else {
      args.push({ key: "", value: tok, kind: "positional" });
      i += 1;
    }
  }
  return { exe, args };
}

/** 分词：双引号分组 + 反斜杠转义（MSVCRT 规则） */
export function tokenize(input: string): string[] {
  const tokens: string[] = [];
  let cur = "";
  let hasToken = false;
  let inQuotes = false;
  const n = input.length;
  let i = 0;
  while (i < n) {
    const c = input[i];
    if (c === "\\") {
      // 统计连续反斜杠；后跟引号时按 MSVCRT 规则折半
      let bs = 0;
      while (i < n && input[i] === "\\") {
        bs++;
        i++;
      }
      if (i < n && input[i] === '"') {
        cur += "\\".repeat(Math.floor(bs / 2));
        if (bs % 2 === 1) {
          cur += '"'; // 转义的引号是字面量
        } else {
          inQuotes = !inQuotes; // 偶数个：引号是分组定界符
          hasToken = true;
        }
      } else {
        cur += "\\".repeat(bs);
        hasToken = true;
      }
      continue;
    }
    if (c === '"') {
      inQuotes = !inQuotes;
      hasToken = true;
      i++;
      continue;
    }
    if ((c === " " || c === "\t") && !inQuotes) {
      if (hasToken) {
        tokens.push(cur);
        cur = "";
        hasToken = false;
      }
      i++;
      continue;
    }
    cur += c;
    hasToken = true;
    i++;
  }
  if (hasToken) tokens.push(cur);
  return tokens;
}
