/**
 * 渲染与变换（纯前端，零 IPC）——《需求报告与技术方案.md》8.4
 *
 * 变换模型：图片以其自然中心为原点旋转/缩放/翻转，再平移到屏幕上的图片中心 (cx, cy)。
 *   transform: translate(cx,cy) rotate(r) scale(s*fx, s*fy) translate(-w/2, -h/2)
 * 总显示倍率 s = baseScale * userScale：
 *   - fit 模式：baseScale = 适应窗口倍率；userScale 为滚轮缩放倍率
 *   - actual 模式：baseScale = 1
 */

import { convertFileSrc } from "@tauri-apps/api/core";

const MIN_SCALE = 0.04;
const MAX_SCALE = 20;
const ROTATE_MS = 220; // 略大于 200ms 过渡，结束后移除 animating class

export type FitMode = "fit" | "actual";

export class Viewer {
  private img: HTMLImageElement;
  private stage: HTMLElement;

  private loaded = false;
  private naturalW = 0;
  private naturalH = 0;

  // 变换状态
  private fitMode: FitMode = "fit";
  private baseScale = 1;
  private userScale = 1;
  private rotation = 0; // 0 / 90 / 180 / 270
  private flipH = false;
  private flipV = false;
  private cx = 0; // 图片中心（屏幕坐标）
  private cy = 0;

  // 拖拽平移
  private drag: { sx: number; sy: number; cx0: number; cy0: number } | null = null;
  private animTimer: number | undefined;
  private onLoadCb: (() => void) | null = null;

  constructor(stage: HTMLElement, img: HTMLImageElement) {
    this.stage = stage;
    this.img = img;
    this.img.addEventListener("load", () => this.handleLoad());
    this.img.addEventListener("error", () => this.handleError());
  }

  get hasImage(): boolean {
    return this.loaded;
  }

  get mode(): FitMode {
    return this.fitMode;
  }

  get currentScale(): number {
    return this.baseScale * this.userScale;
  }

  /** 加载新图片：重置变换并适应窗口 */
  load(path: string): Promise<void> {
    return new Promise((resolve) => {
      this.onLoadCb = resolve;
      this.resetTransform();
      this.img.classList.remove("visible");
      // 先清除 src 再赋值：同路径重复打开时也能触发 load 事件
      this.img.removeAttribute("src");
      this.img.src = convertFileSrc(path);
    });
  }

  /** 重新计算（窗口尺寸变化时调用） */
  onResize(): void {
    if (!this.loaded) return;
    if (this.fitMode === "fit" && this.userScale === 1) {
      this.fit();
    } else {
      this.apply();
      this.updatePanState();
    }
  }

  // ---------- 变换操作 ----------

  setMode(mode: FitMode): void {
    if (this.fitMode === mode) return;
    this.fitMode = mode;
    this.userScale = 1;
    if (mode === "actual") {
      this.baseScale = 1;
    } else {
      this.fit();
    }
  }

  rotate(delta: number): void {
    this.rotation = ((this.rotation + delta) % 360 + 360) % 360;
    if (this.fitMode === "fit" && this.userScale === 1) {
      // 旋转 90/270 后交换宽高重新适应
      this.fit();
    } else {
      this.apply();
    }
    this.animateTransform();
  }

  flip(axis: "h" | "v"): void {
    if (axis === "h") this.flipH = !this.flipH;
    else this.flipV = !this.flipV;
    this.apply();
    this.animateTransform();
  }

  /** 滚轮缩放：以 (mx, my) 为锚点 */
  zoomAt(mx: number, my: number, factor: number): void {
    if (!this.loaded) return;
    const s0 = this.currentScale;
    const s1 = clamp(s0 * factor, MIN_SCALE, MAX_SCALE);
    const ratio = s1 / s0;
    this.cx = mx - (mx - this.cx) * ratio;
    this.cy = my - (my - this.cy) * ratio;
    this.userScale = s1 / this.baseScale;
    this.apply();
    this.updatePanState();
  }

  /** 键盘缩放：以图片中心为锚点 */
  zoomByCenter(factor: number): void {
    if (!this.loaded) return;
    this.zoomAt(this.stage.clientWidth / 2, this.stage.clientHeight / 2, factor);
  }

  // ---------- 拖拽平移 ----------

  startPan(sx: number, sy: number): void {
    if (!this.isPannable()) return;
    this.drag = { sx, sy, cx0: this.cx, cy0: this.cy };
    this.stage.classList.add("dragging");
  }

  panTo(x: number, y: number): void {
    if (!this.drag) return;
    this.cx = this.drag.cx0 + (x - this.drag.sx);
    this.cy = this.drag.cy0 + (y - this.drag.sy);
    this.clampPan();
    this.apply();
  }

  endPan(): void {
    this.drag = null;
    this.stage.classList.remove("dragging");
  }

  /** 是否可拖拽平移：图片显示尺寸超过视口 */
  isPannable(): boolean {
    if (!this.loaded) return false;
    const s = this.currentScale;
    const ew = (this.rotation % 180 === 0 ? this.naturalW : this.naturalH) * s;
    const eh = (this.rotation % 180 === 0 ? this.naturalH : this.naturalW) * s;
    return ew > this.stage.clientWidth + 4 || eh > this.stage.clientHeight + 4;
  }

  // ---------- 内部 ----------

  private resetTransform(): void {
    this.loaded = false;
    this.fitMode = "fit";
    this.baseScale = 1;
    this.userScale = 1;
    this.rotation = 0;
    this.flipH = false;
    this.flipV = false;
    this.stage.classList.remove("pannable", "dragging", "animating");
    this.img.classList.remove("animating");
  }

  private handleLoad(): void {
    this.naturalW = this.img.naturalWidth;
    this.naturalH = this.img.naturalHeight;
    this.loaded = true;
    this.fit();
    this.img.classList.add("visible");
    this.onLoadCb?.();
    this.onLoadCb = null;
  }

  private handleError(): void {
    // asset 协议读取失败：保留空状态即可
    this.onLoadCb?.();
    this.onLoadCb = null;
  }

  private fit(): void {
    if (!this.loaded) return;
    const availW = this.stage.clientWidth;
    const availH = this.stage.clientHeight - TITLEBAR_H; // 避让常驻标题栏
    const ew = this.rotation % 180 === 0 ? this.naturalW : this.naturalH;
    const eh = this.rotation % 180 === 0 ? this.naturalH : this.naturalW;
    this.baseScale = Math.min(availW / ew, availH / eh);
    this.cx = availW / 2;
    this.cy = TITLEBAR_H + availH / 2;
    this.apply();
    this.updatePanState();
  }

  private apply(): void {
    if (!this.loaded) return;
    const s = this.currentScale;
    const fx = this.flipH ? -1 : 1;
    const fy = this.flipV ? -1 : 1;
    this.img.style.transform =
      `translate(${this.cx}px, ${this.cy}px) ` +
      `rotate(${this.rotation}deg) ` +
      `scale(${s * fx}, ${s * fy}) ` +
      `translate(${-this.naturalW / 2}px, ${-this.naturalH / 2}px)`;
  }

  /** 平移范围约束：图片至少与视口保留 24px 重叠 */
  private clampPan(): void {
    const s = this.currentScale;
    const ew = (this.rotation % 180 === 0 ? this.naturalW : this.naturalH) * s;
    const eh = (this.rotation % 180 === 0 ? this.naturalH : this.naturalW) * s;
    const margin = 24;
    const minX = margin - ew / 2;
    const maxX = this.stage.clientWidth - margin + ew / 2;
    const minY = margin - eh / 2;
    const maxY = this.stage.clientHeight - margin + eh / 2;
    this.cx = clamp(this.cx, Math.min(minX, maxX), Math.max(minX, maxX));
    this.cy = clamp(this.cy, Math.min(minY, maxY), Math.max(minY, maxY));
  }

  private updatePanState(): void {
    this.stage.classList.toggle("pannable", this.isPannable());
  }

  /** 旋转/翻转时的 200ms ease-in-out 过渡 */
  private animateTransform(): void {
    this.img.classList.add("animating");
    window.clearTimeout(this.animTimer);
    this.animTimer = window.setTimeout(() => {
      this.img.classList.remove("animating");
    }, ROTATE_MS);
  }
}

const TITLEBAR_H = 32;

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
