// parseCommandLine 契约测试：Windows 分词与参数分类
import { describe, expect, it } from "vitest";
import { parseCommandLine, tokenize } from "./parseCommandLine";

describe("tokenize", () => {
  it("按空白分词并去引号", () => {
    expect(tokenize('java -jar "my app.jar" -v')).toEqual(["java", "-jar", "my app.jar", "-v"]);
  });

  it("反斜杠转义引号（MSVCRT 规则）", () => {
    // 奇数个反斜杠+引号 → 字面量引号
    expect(tokenize('say \\"hi\\"')).toEqual(["say", '"hi"']);
    // 偶数个反斜杠+引号 → 定界符
    expect(tokenize('"C:\\Program Files\\x.exe"')).toEqual(["C:\\Program Files\\x.exe"]);
  });

  it("引号内空白不分词", () => {
    expect(tokenize('a "b  c" d')).toEqual(["a", "b  c", "d"]);
  });

  it("空串与纯空白返回空", () => {
    expect(tokenize("")).toEqual([]);
    expect(tokenize("   \t ")).toEqual([]);
  });
});

describe("parseCommandLine", () => {
  it("完整命令行：exe + option + flag + positional", () => {
    const r = parseCommandLine('java -Xms512m -jar app.jar --port 8080 --verbose')!;
    expect(r.exe).toBe("java");
    expect(r.args).toEqual([
      { key: "-Xms512m", value: null, kind: "flag" },
      { key: "-jar", value: "app.jar", kind: "option" },
      { key: "--port", value: "8080", kind: "option" },
      { key: "--verbose", value: null, kind: "flag" },
    ]);
  });

  it("--key=value 拆为 option", () => {
    const r = parseCommandLine('nginx.exe --conf-path="C:\\my conf\\n.conf"')!;
    expect(r.args).toEqual([
      { key: "--conf-path", value: "C:\\my conf\\n.conf", kind: "option" },
    ]);
  });

  it("位置参数与带引号 exe", () => {
    const r = parseCommandLine('"C:\\Tools\\a b.exe" serve --debug')!;
    expect(r.exe).toBe("C:\\Tools\\a b.exe");
    expect(r.args).toEqual([
      { key: "", value: "serve", kind: "positional" },
      { key: "--debug", value: null, kind: "flag" },
    ]);
  });

  it("末尾 option 缺值按 flag 处理", () => {
    const r = parseCommandLine("app --port")!;
    expect(r.args).toEqual([{ key: "--port", value: null, kind: "flag" }]);
  });

  it("空输入返回 null", () => {
    expect(parseCommandLine("   ")).toBeNull();
  });
});
