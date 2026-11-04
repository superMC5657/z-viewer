/**
 * 深放大真像素升级判定回归测试。
 * 三区间语义 + 两处 bug 的回归锁：
 * - BUG-A：3841–8192 区间 original 返回 capped=false（语义是「原图 ≤8192」，
 *   不是「无需升级」），必须升级而不是丢弃全分辨率位图；
 * - BUG-B：actual 模式恰好 1:1（currentScale=1）也要触发取数。
 */

import { describe, expect, it } from "vitest";

import { shouldUpgradeToNative } from "./native-upgrade";

const probe = (over: Partial<{ isPreview: boolean; capped: boolean; width: number | null }> = {}) => ({
  isPreview: false,
  capped: false,
  width: null,
  ...over,
});

describe("shouldUpgradeToNative（深放大真像素判定三区间）", () => {
  it("≤3840（currentCapped=false）：显示位图已是最高保真，任何倍率都不触发", () => {
    expect(shouldUpgradeToNative(null, false, 1, 3000)).toBe(false);
    expect(shouldUpgradeToNative(null, false, 4, 3000)).toBe(false);
  });

  it("fit 态未达 1:1（currentScale<1）：不触发（>3840 源 baseScale<1 天然不取数）", () => {
    expect(shouldUpgradeToNative(null, true, 0.5, 3840)).toBe(false);
  });

  it("actual 模式恰好 1:1（currentScale=1）：触发取数（BUG-B 回归）", () => {
    expect(shouldUpgradeToNative(null, true, 1, 3840)).toBe(true);
  });

  it("放大越过 1:1（currentScale>1）：触发取数", () => {
    expect(shouldUpgradeToNative(null, true, 1.5, 3840)).toBe(true);
  });

  it("3841–8192：original 返回 capped=false 的全分辨率真像素，必须升级（BUG-A 回归）", () => {
    // 24MP 主流全画幅 6000×4000：取数帽 8192 下 capped=false 语义是「原图 ≤8192」，
    // 返回的就是真像素位图——升级，而非按「无需升级」丢弃
    expect(shouldUpgradeToNative(probe({ capped: false, width: 6000 }), true, 1.5, 3840)).toBe(true);
    expect(shouldUpgradeToNative(probe({ capped: false, width: 8192 }), true, 2, 3840)).toBe(true);
  });

  it(">8192：original 返回 capped=true 的 8192 帽降采样，升级到取数帽", () => {
    expect(shouldUpgradeToNative(probe({ capped: true, width: 8192 }), true, 2, 3840)).toBe(true);
  });

  it("防御：异常路径仍返回预览时不升级", () => {
    expect(shouldUpgradeToNative(probe({ isPreview: true, capped: true, width: 8192 }), true, 2, 3840)).toBe(false);
  });

  it("防御：返回位图宽度无增益时不升级（视为已到位）", () => {
    expect(shouldUpgradeToNative(probe({ capped: true, width: 3840 }), true, 2, 3840)).toBe(false);
    expect(shouldUpgradeToNative(probe({ capped: false, width: 1920 }), true, 2, 3840)).toBe(false);
  });
});
