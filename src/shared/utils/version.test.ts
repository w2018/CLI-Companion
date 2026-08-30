// 版本比较契约测试：只有 latest 严格大于 current 才提示更新
import { describe, expect, it } from "vitest";
import { isNewerVersion, parseVersion } from "./version";

describe("isNewerVersion", () => {
  it("更高版本返回 true", () => {
    expect(isNewerVersion("v2.4.0", "2.3.0")).toBe(true);
    expect(isNewerVersion("2.3.1", "v2.3.0")).toBe(true);
    expect(isNewerVersion("v3.0.0", "2.9.9")).toBe(true);
  });

  it("相同版本返回 false（不提示更新）", () => {
    expect(isNewerVersion("v2.3.0", "2.3.0")).toBe(false);
  });

  it("更低/不同分支的旧版本返回 false（旧逻辑的 bug 场景）", () => {
    expect(isNewerVersion("v2.2.0", "2.3.0")).toBe(false);
    expect(isNewerVersion("v2.1.0", "2.3.0")).toBe(false);
    expect(isNewerVersion("v1.9.9", "2.0.0")).toBe(false);
  });

  it("无法解析返回 null", () => {
    expect(isNewerVersion("abc", "2.3.0")).toBeNull();
    expect(isNewerVersion("v2.3", "2.3.0")).toBeNull();
  });
});

describe("parseVersion", () => {
  it("支持 v 前缀与纯数字", () => {
    expect(parseVersion("v2.3.0")).toEqual({ major: 2, minor: 3, patch: 0 });
    expect(parseVersion("2.10.1")).toEqual({ major: 2, minor: 10, patch: 1 });
    expect(parseVersion(" 1.2.3-beta ")).toEqual({ major: 1, minor: 2, patch: 3 });
  });
});
