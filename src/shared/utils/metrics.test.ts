// 资源指标展示辅助测试：速率格式化 + 占用分档边界
import { describe, expect, it } from "vitest";
import {
  combinedRate,
  formatRate,
  levelCls,
  percentLevel,
  rateLevel,
} from "./metrics";

describe("formatRate", () => {
  it("字节/秒人性化换算", () => {
    expect(formatRate(0)).toBe("0 B/s");
    expect(formatRate(1536)).toBe("1.5 KB/s");
    expect(formatRate(2 * 1024 * 1024)).toBe("2.0 MB/s");
  });
  it("无效值返回 —", () => {
    expect(formatRate(null)).toBe("—");
    expect(formatRate(undefined)).toBe("—");
    expect(formatRate(-5)).toBe("—");
    expect(formatRate(Number.NaN)).toBe("—");
  });
});

describe("percentLevel", () => {
  it("默认阈值 60/85 的边界", () => {
    expect(percentLevel(0)).toBe("ok");
    expect(percentLevel(59.9)).toBe("ok");
    expect(percentLevel(60)).toBe("warn");
    expect(percentLevel(84.9)).toBe("warn");
    expect(percentLevel(85)).toBe("err");
    expect(percentLevel(100)).toBe("err");
  });
  it("自定义阈值（内存 70/90）", () => {
    expect(percentLevel(69, 70, 90)).toBe("ok");
    expect(percentLevel(70, 70, 90)).toBe("warn");
    expect(percentLevel(89.9, 70, 90)).toBe("warn");
    expect(percentLevel(90, 70, 90)).toBe("err");
  });
  it("非法输入归入正常档", () => {
    expect(percentLevel(Number.NaN)).toBe("ok");
  });
});

describe("rateLevel", () => {
  it("默认阈值 1MB/20MB（磁盘）", () => {
    expect(rateLevel(0)).toBe("ok");
    expect(rateLevel(1024 * 1024 - 1)).toBe("ok");
    expect(rateLevel(1024 * 1024)).toBe("warn");
    expect(rateLevel(20 * 1024 * 1024 - 1)).toBe("warn");
    expect(rateLevel(20 * 1024 * 1024)).toBe("err");
  });
  it("自定义阈值（网络 512KB/5MB）", () => {
    expect(rateLevel(512 * 1024 - 1, 512 * 1024, 5 * 1024 * 1024)).toBe("ok");
    expect(rateLevel(512 * 1024, 512 * 1024, 5 * 1024 * 1024)).toBe("warn");
    expect(rateLevel(5 * 1024 * 1024, 512 * 1024, 5 * 1024 * 1024)).toBe("err");
  });
  it("负值/NaN 归入正常档", () => {
    expect(rateLevel(-1)).toBe("ok");
    expect(rateLevel(Number.NaN)).toBe("ok");
  });
});

describe("combinedRate", () => {
  it("合并读写速率", () => {
    expect(combinedRate(100, 200)).toBe(300);
    expect(combinedRate(null, 200)).toBe(200);
    expect(combinedRate(100, null)).toBe(100);
    expect(combinedRate(NaN, 1)).toBe(1);
  });
  it("双方均无效返回 null", () => {
    expect(combinedRate(null, null)).toBeNull();
    expect(combinedRate(undefined, Number.NaN)).toBeNull();
  });
});

describe("levelCls", () => {
  it("三档映射到语义色", () => {
    expect(levelCls.ok).toBe("text-ok");
    expect(levelCls.warn).toBe("text-warn");
    expect(levelCls.err).toBe("text-err");
  });
});
