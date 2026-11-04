/**
 * showUpgrade 变换保持回归测试（升级契约：viewer.ts「升级前后屏幕视图逐像素不变」）。
 *
 * 用户症状复现：预览（3840 帽）放大越过 1:1 → 深放大原图经 showUpgrade 到达 →
 * 视图突然跳回适应窗口（症状一）；再次缩放又跳回之前的放大倍率（症状二）。
 *
 * node 环境无 DOM 库，用最小桩驱动 Viewer：img 桩 decode() 立即 resolve，
 * envelope 走 JPEG Blob 路径（img 通道）与 x-rgba 路径（canvas 通道）各锁一处升级分支。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import type { LoadEnvelope } from "./types";
import { Viewer } from "./viewer";

// ---------- 最小 DOM 桩 ----------

function fakeClassList() {
  const set = new Set<string>();
  return {
    add: (...tokens: string[]) => tokens.forEach((t) => set.add(t)),
    remove: (...tokens: string[]) => tokens.forEach((t) => set.delete(t)),
    contains: (t: string) => set.has(t),
  };
}

function makeImg(w: number, h: number) {
  const el: Record<string, unknown> = {
    classList: fakeClassList(),
    style: {},
    complete: true,
    naturalWidth: w,
    naturalHeight: h,
    decode: () => Promise.resolve(),
    getAttribute: (name: string) => (name === "src" ? el["_src"] : null),
    removeAttribute: (name: string) => {
      if (name === "src") el["_src"] = null;
    },
  };
  Object.defineProperty(el, "src", {
    get: () => el["_src"],
    set: (v: string) => {
      el["_src"] = v;
    },
  });
  return el as unknown as HTMLImageElement;
}

function makeCanvas() {
  return {
    classList: fakeClassList(),
    style: {},
    width: 0,
    height: 0,
    getContext: () => ({ drawImage() {} }),
  } as unknown as HTMLCanvasElement;
}

type Transform = { cx: number; cy: number; sx: number; sy: number; w: number; h: number };

function parseTransform(t: string): Transform {
  const m = t.match(
    /translate\(([-\d.eE+]+)px, ([-\d.eE+]+)px\) rotate\(([-\d.eE+]+)deg\) scale\(([-\d.eE+]+), ([-\d.eE+]+)\) translate\(([-\d.eE+]+)px, ([-\d.eE+]+)px\)/,
  );
  expect(m, `transform 形态异常: ${t}`).toBeTruthy();
  return { cx: +m![1], cy: +m![2], sx: +m![4], sy: +m![5], w: +m![6], h: +m![7] };
}

// ---------- 夹具 ----------

const STAGE_W = 1200;
const STAGE_H = 800;

/** 原图 7680×4320（>3840 显示帽，触发深放大升级）；预览为其一半 3840×2160 */
const FULL_W = 7680;
const FULL_H = 4320;

let imgFull: HTMLImageElement;
let imgPreview: HTMLImageElement;
let canvas: HTMLCanvasElement;
let stage: HTMLElement;
let viewer: Viewer;

const jpegEnvelope = (): LoadEnvelope => ({
  mode: "raw",
  mime: "image/jpeg",
  isPreview: false,
  capped: false,
  width: FULL_W,
  height: FULL_H,
  frameSizes: [],
  frameDelays: [],
});

/** 可见元素的 transform（img 双缓冲取带 visible 的那个；canvas 路径读 canvas） */
function readVisibleTransform(): string {
  const a = imgFull as unknown as { classList: { contains(t: string): boolean }; style: { transform?: string } };
  const b = imgPreview as unknown as { classList: { contains(t: string): boolean }; style: { transform?: string } };
  if (a.classList.contains("visible")) return a.style.transform ?? "";
  if (b.classList.contains("visible")) return b.style.transform ?? "";
  return (canvas as unknown as { style: { transform?: string } }).style.transform ?? "";
}

beforeEach(() => {
  vi.stubGlobal("window", {
    setTimeout: globalThis.setTimeout.bind(globalThis),
    clearTimeout: globalThis.clearTimeout.bind(globalThis),
  });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(cb, 0) as unknown as number);
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
  (URL as unknown as Record<string, unknown>).createObjectURL = vi.fn(() => "blob:test-full");
  (URL as unknown as Record<string, unknown>).revokeObjectURL = vi.fn();

  imgFull = makeImg(FULL_W, FULL_H);
  imgPreview = makeImg(FULL_W / 2, FULL_H / 2);
  canvas = makeCanvas();
  stage = {
    classList: fakeClassList(),
    clientWidth: STAGE_W,
    clientHeight: STAGE_H,
  } as unknown as HTMLElement;
  viewer = new Viewer(stage, imgFull, imgPreview, canvas);
});

/** 复现路径公共前缀：预览加载（fit）→ 滚轮放大 4 倍（总倍率 1.25，越过 1:1） */
async function loadPreviewAndZoom(): Promise<Transform> {
  await viewer.show({ kind: "url", url: "blob:preview" });
  viewer.zoomAt(STAGE_W / 2, 400, 4);
  return parseTransform(readVisibleTransform());
}

describe("showUpgrade 变换保持（升级前后屏幕视图逐像素不变）", () => {
  it("症状一：升级后总倍率按新旧尺寸比换算且平移中心保持，不跳回适应窗口", async () => {
    const before = await loadPreviewAndZoom();
    expect(before.sx).toBeCloseTo(1.25); // 预览位图基准：fit 0.3125 × 4
    expect(before.cx).toBeCloseTo(600);
    expect(before.cy).toBeCloseTo(464); // 锚点 400：(400-416)*4 补偿

    await viewer.showUpgrade(jpegEnvelope(), new Uint8Array(8));

    // 升级后可见元素应切到全分辨率位图
    expect((imgFull as unknown as { classList: { contains(t: string): boolean } }).classList.contains("visible")).toBe(
      true,
    );
    const after = parseTransform(readVisibleTransform());
    // 总倍率 = 1.25 × (3840/7680) = 0.625（新位图基准，屏幕视觉倍率不变）
    expect(after.sx).toBeCloseTo(0.625);
    expect(after.sy).toBeCloseTo(0.625);
    // 平移中心保持（同倍率补偿下 cx/cy 天然不动），不跳回 fit 的居中 (600, 416)
    expect(after.cx).toBeCloseTo(before.cx);
    expect(after.cy).toBeCloseTo(before.cy);
    expect(after.w).toBeCloseTo(-FULL_W / 2); // transform 尾段为 -naturalW/2
    expect(after.h).toBeCloseTo(-FULL_H / 2);
  });

  it("症状二：升级后继续缩放从保持的视图出发，不跳回旧倍率/中心", async () => {
    await loadPreviewAndZoom();
    await viewer.showUpgrade(jpegEnvelope(), new Uint8Array(8));

    viewer.zoomAt(STAGE_W / 2, 400, 1.1);
    const t = parseTransform(readVisibleTransform());
    expect(t.sx).toBeCloseTo(0.625 * 1.1);
    // 从保持的 cy=464 出发：cy = 400 - (400-464)*1.1 = 470.4
    expect(t.cy).toBeCloseTo(470.4);
  });

  it("canvas 通道（x-rgba）升级同样保持总倍率与平移中心", async () => {
    vi.stubGlobal(
      "ImageData",
      class {
        data: Uint8ClampedArray;
        width: number;
        height: number;
        constructor(data: Uint8ClampedArray, width: number, height: number) {
          this.data = data;
          this.width = width;
          this.height = height;
        }
      },
    );
    vi.stubGlobal("createImageBitmap", async (src: { width: number; height: number }) => ({
      width: src.width,
      height: src.height,
      close() {},
    }));

    // 小尺寸降低测试内存：预览 1200×600 → 全量 2400×1200
    const pw = 1200;
    const ph = 600;
    (imgPreview as unknown as { naturalWidth: number }).naturalWidth = pw;
    (imgPreview as unknown as { naturalHeight: number }).naturalHeight = ph;
    (imgFull as unknown as { naturalWidth: number }).naturalWidth = pw * 2;
    (imgFull as unknown as { naturalHeight: number }).naturalHeight = ph * 2;

    const header: LoadEnvelope = {
      mode: "raw",
      mime: "image/x-rgba",
      isPreview: false,
      capped: false,
      width: pw * 2,
      height: ph * 2,
      frameSizes: [],
      frameDelays: [],
    };
    const payload = new Uint8Array(pw * 2 * ph * 2 * 4);

    await viewer.show({ kind: "envelope", header: { ...header, width: pw, height: ph }, payload: new Uint8Array(pw * ph * 4) });
    viewer.zoomAt(STAGE_W / 2, 400, 2);
    const before = parseTransform(readVisibleTransform());

    await viewer.showUpgrade(header, payload);

    expect((canvas as unknown as { classList: { contains(t: string): boolean } }).classList.contains("visible")).toBe(
      true,
    );
    const after = parseTransform(readVisibleTransform());
    expect(after.sx).toBeCloseTo(before.sx / 2);
    expect(after.cx).toBeCloseTo(before.cx);
    expect(after.cy).toBeCloseTo(before.cy);
  });
});
