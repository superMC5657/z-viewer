/**
 * 渲染与变换（纯前端，零 IPC）——《需求报告与技术方案.md》8.3/8.4
 *
 * 变换模型：图片以其自然中心为原点旋转/缩放/翻转，再平移到屏幕上的图片中心 (cx, cy)。
 *   transform: translate(cx,cy) rotate(r) scale(s*fx, s*fy) translate(-w/2, -h/2)
 * 总显示倍率 s = baseScale * userScale：
 *   - fit 模式：baseScale = 适应窗口倍率；userScale 为滚轮缩放倍率
 *   - actual 模式：baseScale = 1
 *
 * 渲染通道：
 *   - 静态图 → <img>（asset 协议或 RAW 解码的 Blob URL）
 *   - 动画图 → <canvas> 逐帧绘制（ImageBitmap 预解码 + setTimeout 链按帧延迟播放）
 *
 * 切图 crossfade：加载新图前把旧画面（含变换）重绘到快照层 <canvas> 冻结，
 * 新图解码完成后快照淡出、新图淡入（150ms 交叉淡化，无黑屏跳变）。
 */

const MIN_SCALE = 0.04;
const MAX_SCALE = 20;
const ROTATE_MS = 220; // 略大于 200ms 过渡，结束后移除 animating class

export type FitMode = "fit" | "actual";

export class Viewer {
  private imgA: HTMLImageElement;
  private imgB: HTMLImageElement;
  private activeImg: HTMLImageElement;
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private stage: HTMLElement;

  private loaded = false;
  private naturalW = 0;
  private naturalH = 0;
  private active: "img" | "canvas" = "img";

  // 变换状态
  private fitMode: FitMode = "fit";
  private baseScale = 1;
  private userScale = 1;
  private rotation = 0; // 0 / 90 / 180 / 270（归一化，用于宽高交换/布局）
  /** 累计旋转角（可超 ±360，单调步进）：驱动 CSS transition 走最短 90° 路径。
   *  旋转归一化角度会导致 0→270 的过渡绕 270° 长路径且方向相反（CSS rotate 插值不取最短路径） */
  private rotTotal = 0;
  private flipH = false;
  private flipV = false;
  private cx = 0; // 图片中心（屏幕坐标）
  private cy = 0;

  // 拖拽平移（rAF 合并，保证 60fps 高频输入只写一帧 style）
  private drag: { sx: number; sy: number; cx0: number; cy0: number } | null = null;
  private rafHandle: number | null = null;
  /** 动画播放 rAF 句柄 */
  private rafAnimHandle: number | null = null;
  private lastAnimTick = 0;
  /** 旋转/翻转过渡计时器（与播放器独立，互不干扰） */
  private transformTimer: number | undefined;
  /** 沉浸模式：fit 时是否避让标题栏 */
  private immersive = false;
  /** 加载代次：异步解码期间被新加载抢占时丢弃旧结果 */
  private loadSeq = 0;
  /** 进行中的静态加载（img 通道） */
  private pending: { resolve: () => void; reject: (e: Error) => void; seq: number } | null = null;
  /** 静态加载 2.5s 超时兜底定时器（完成/失败/作废时清理，不留空转定时器） */
  private staticLoadTimer: number | undefined;

  // 动画状态
  private animFrames: ImageBitmap[] = [];
  private animDelays: number[] = [];
  private animIndex = 0;
  private animPlaying = false;

  /** 帧变化回调（index 从 1 开始） */
  onFrameChange: ((index: number, total: number) => void) | null = null;
  /** 变换状态变化回调（缩放/模式/旋转/加载后触发，用于同步按钮状态） */
  onStateChange: (() => void) | null = null;

  constructor(stage: HTMLElement, imgA: HTMLImageElement, imgB: HTMLImageElement, canvas: HTMLCanvasElement) {
    this.stage = stage;
    this.imgA = imgA;
    this.imgB = imgB;
    this.activeImg = imgA;
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
  }

  get hasImage(): boolean {
    return this.loaded;
  }

  get mode(): FitMode {
    return this.fitMode;
  }

  /** 是否处于「纯适应窗口」状态（fit 模式且缩放接近 1，容差防浮点抖动） */
  get isFit(): boolean {
    return this.fitMode === "fit" && Math.abs(this.userScale - 1) < 0.005;
  }

  get currentScale(): number {
    return this.baseScale * this.userScale;
  }

  get isAnimation(): boolean {
    return this.active === "canvas" && this.animFrames.length > 0;
  }

  get isPlaying(): boolean {
    return this.animPlaying;
  }

  /** 当前显示尺寸（静态图 = 图像自然尺寸；动画 = 第一帧尺寸） */
  get naturalWidth(): number {
    return this.naturalW;
  }

  get naturalHeight(): number {
    return this.naturalH;
  }

  /** 加载静态图（asset 协议 URL 或 RAW 解码的 Blob URL） */
  loadStatic(url: string): Promise<void> {
    this.stopAnimation();
    this.settlePending(); // 作废进行中的加载（静默 resolve，由新加载覆盖显示）
    const seq = ++this.loadSeq;
    this.active = "img";
    this.canvas.classList.remove("visible");

    // 双缓冲轮换：取非当前显示的那个 img 作为 incoming
    const incoming = this.activeImg === this.imgA ? this.imgB : this.imgA;
    const outgoing = this.activeImg;

    return new Promise((resolve, reject) => {
      this.pending = { resolve, reject, seq };
      window.clearTimeout(this.staticLoadTimer);
      this.staticLoadTimer = window.setTimeout(() => {
        if (this.pending?.seq === seq) {
          this.pending.resolve();
        }
      }, 2500);

      const onDecoded = () => {
        if (this.pending?.seq !== seq) return;
        window.clearTimeout(this.staticLoadTimer);
        this.resetTransform();
        this.naturalW = incoming.naturalWidth;
        this.naturalH = incoming.naturalHeight;
        this.loaded = true;
        this.activeImg = incoming;
        this.fit(); // 内部针对 incoming 设置 transform

        // 双缓冲 crossfade：纯 GPU 合成器透明度过渡，零 Canvas 绘图与主线程阻塞
        incoming.classList.add("visible");
        outgoing.classList.remove("visible");

        this.pending?.resolve();
        this.pending = null;
        this.onStateChange?.();
        resolve();
      };

      const onError = () => {
        if (this.pending?.seq !== seq) return;
        window.clearTimeout(this.staticLoadTimer);
        this.pending = null;
        this.resetTransform();
        outgoing.classList.remove("visible");
        reject(new Error("图片加载失败"));
      };

      incoming.classList.remove("animating");
      if (incoming.getAttribute("src") !== url) {
        incoming.removeAttribute("src");
        incoming.src = url;
      }
      // 直接触发 decode()，在图片完成解码并上载到 GPU 显存后 resolve
      incoming
        .decode()
        .then(() => onDecoded())
        .catch(() => {
          if (incoming.complete && incoming.naturalWidth > 0) {
            onDecoded();
          } else {
            incoming.onload = () => onDecoded();
            incoming.onerror = () => onError();
          }
        });
    });
  }

  /** 加载单张位图（原始 RGBA 直通，免 JPEG 二次编解码开销） */
  async loadBitmap(bitmap: ImageBitmap, keepTransform = false): Promise<void> {
    this.stopAnimation();
    this.settlePending();
    const seq = ++this.loadSeq;
    this.active = "canvas";
    this.imgA.classList.remove("visible");
    this.imgB.classList.remove("visible");

    if (seq !== this.loadSeq) {
      bitmap.close();
      return;
    }

    const oldW = this.naturalW;
    const oldH = this.naturalH;
    if (!keepTransform) {
      this.resetTransform();
    }
    this.naturalW = bitmap.width;
    this.naturalH = bitmap.height;
    this.canvas.width = this.naturalW;
    this.canvas.height = this.naturalH;
    this.ctx.drawImage(bitmap, 0, 0);
    bitmap.close();

    this.loaded = true;
    if (keepTransform && oldW > 0 && oldH > 0) {
      if (this.fitMode === "fit" && Math.abs(this.userScale - 1) < 0.005) {
        this.fit();
      } else {
        this.apply();
      }
    } else {
      this.fit();
    }
    this.canvas.classList.add("visible");
    this.onStateChange?.();
  }

  /** 加载预解码的动画位图序列（Web 原生 ImageDecoder 通道，首选零开销路径） */
  async loadBitmapsAnimation(bitmaps: ImageBitmap[], delays: number[]): Promise<void> {
    this.stopAnimation();
    this.settlePending();
    const seq = ++this.loadSeq;
    this.active = "canvas";
    this.imgA.classList.remove("visible");
    this.imgB.classList.remove("visible");
    this.resetTransform();
    this.onFrameChange?.(0, bitmaps.length);

    if (bitmaps.length === 0) return;
    if (seq !== this.loadSeq) {
      for (const b of bitmaps) b.close();
      return;
    }

    this.animFrames = bitmaps;
    this.animDelays = delays;
    this.animIndex = 0;
    this.naturalW = bitmaps[0].width;
    this.naturalH = bitmaps[0].height;
    this.canvas.width = this.naturalW;
    this.canvas.height = this.naturalH;
    this.loaded = true;
    this.drawFrame();
    this.fit();
    this.canvas.classList.add("visible");
    this.play();
    this.onStateChange?.();
  }

  /** 加载动画图：预解码全部帧后显示第一帧并自动播放（帧 PNG 字节按 frame_sizes 切分） */
  async loadAnimation(frameBlobs: Blob[], delays: number[]): Promise<void> {
    if (frameBlobs.length === 0) return;
    const settled = await Promise.allSettled(frameBlobs.map((b) => createImageBitmap(b)));
    const bitmaps: ImageBitmap[] = [];
    let failed = false;
    for (const s of settled) {
      if (s.status === "fulfilled") bitmaps.push(s.value);
      else failed = true;
    }
    if (failed) {
      for (const b of bitmaps) b.close();
      throw new Error("动画帧解码失败");
    }
    await this.loadBitmapsAnimation(bitmaps, delays);
  }

  /** 切换沉浸模式：fit 避让高度变化后重新布局（全屏动画后窗口尺寸才稳定，下一帧再校准一次） */
  setImmersive(immersive: boolean): void {
    this.immersive = immersive;
    this.onResize();
    requestAnimationFrame(() => this.onResize());
  }

  /** 重新计算（窗口尺寸变化时调用） */
  onResize(): void {
    if (!this.loaded) return;
    if (this.isFit) {
      this.userScale = 1;
      this.fit();
      // 窗口尺寸变化 → fit 倍率变化 → 同步缩放读数/按钮态
      this.onStateChange?.();
    } else {
      this.clampPan();
      this.apply();
    }
  }

  // ---------- 变换操作 ----------

  setMode(mode: FitMode, anchorX?: number, anchorY?: number): void {
    // 无 early return：fit 模式下手动缩放后（fitMode 仍为 "fit"），
    // 点击适应窗口必须重置 userScale 回到纯 fit 状态
    this.fitMode = mode;
    this.userScale = 1;
    if (mode === "actual") {
      const s0 = this.currentScale;
      this.baseScale = 1;
      if (anchorX !== undefined && anchorY !== undefined && s0 > 0) {
        // 以点击位置为中心锚点放大到 1:1
        const s1 = 1;
        const ratio = s1 / s0;
        this.cx = anchorX - (anchorX - this.cx) * ratio;
        this.cy = anchorY - (anchorY - this.cy) * ratio;
        this.clampPan();
      }
      // 必须 apply：否则只改状态不更新 transform，看起来像没触发
      this.apply();
    } else {
      this.fit(); // fit() 内部已 apply
    }
    this.onStateChange?.();
  }

  /** 双击切换适应窗口与实际大小（支持按点击位置作为锚点） */
  toggleFitActual(anchorX?: number, anchorY?: number): void {
    if (!this.loaded) return;
    if (this.isFit) {
      this.setMode("actual", anchorX, anchorY);
    } else {
      this.setMode("fit");
    }
  }

  rotate(delta: number): void {
    this.rotation = ((this.rotation + delta) % 360 + 360) % 360;
    this.rotTotal += delta; // 累计角度保持单调：过渡沿真实方向转 90°，而非绕 270°
    if (this.isFit) {
      this.userScale = 1;
      // 旋转 90/270 后交换宽高重新适应
      this.fit();
    } else {
      this.apply();
    }
    this.animateTransform();
    // fit 态旋转会重算倍率（宽高互换），缩放百分比随之变化
    this.onStateChange?.();
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
    const minScale = Math.min(MIN_SCALE, this.baseScale * 0.1);
    const maxScale = Math.max(MAX_SCALE, this.baseScale * 10);
    const s1 = clamp(s0 * factor, minScale, maxScale);
    const ratio = s1 / s0;
    this.cx = mx - (mx - this.cx) * ratio;
    this.cy = my - (my - this.cy) * ratio;
    this.userScale = s1 / this.baseScale;
    this.apply();
    this.onStateChange?.();
  }

  /** 键盘缩放：以图片中心为锚点 */
  zoomByCenter(factor: number): void {
    if (!this.loaded) return;
    this.zoomAt(this.stage.clientWidth / 2, this.stage.clientHeight / 2, factor);
  }

  // ---------- 拖拽平移 ----------

  /** 开始拖动：任何状态下都允许（图片大则平移，图片小则结束弹回原位）。
   *  不设 isPannable 拦截 —— 保证鼠标拖动能力始终可用 */
  startPan(sx: number, sy: number): void {
    if (!this.loaded) return;
    this.drag = { sx, sy, cx0: this.cx, cy0: this.cy };
    this.stage.classList.add("dragging");
  }

  /** 拖动中：手跟随鼠标自由移动，不实时 clamp（避免图片小时被拉回抖动） */
  panTo(x: number, y: number): void {
    if (!this.drag) return;
    this.cx = this.drag.cx0 + (x - this.drag.sx);
    this.cy = this.drag.cy0 + (y - this.drag.sy);
    this.scheduleApply();
  }

  /** 触控板平移 / 相对位移平移 */
  panDelta(dx: number, dy: number): void {
    if (!this.loaded) return;
    this.cx += dx;
    this.cy += dy;
    this.clampPan();
    this.scheduleApply();
  }

  endPan(): void {
    this.drag = null;
    this.stage.classList.remove("dragging");
    // 取消挂起的 rAF 帧，同步应用最终位置，避免幂等重放
    if (this.rafHandle !== null) {
      cancelAnimationFrame(this.rafHandle);
      this.rafHandle = null;
    }
    if (this.loaded) {
      // 结束时统一收敛：图片超出视口 → 停在拖到的位置；
      // 图片在视口内 → clampPan 拉回居中范围（弹回原位）
      this.clampPan();
      this.apply();
    }
  }

  /** 是否可拖拽平移：图片显示尺寸超过视口 */
  isPannable(): boolean {
    if (!this.loaded) return false;
    const s = this.currentScale;
    const ew = (this.rotation % 180 === 0 ? this.naturalW : this.naturalH) * s;
    const eh = (this.rotation % 180 === 0 ? this.naturalH : this.naturalW) * s;
    // 可拖 = 图片任一边真正超出视口（需 > 2*margin 才有可动空间，与 clampPan 一致）
    // 若仅超出 4px 容差，clamp 会把图片钉死（边缘强制留 margin），拖了没效果
    return ew > this.stage.clientWidth + PAN_MARGIN * 2 || eh > this.stage.clientHeight + PAN_MARGIN * 2;
  }

  // ---------- 动画控制 ----------

  play(): void {
    if (!this.isAnimation || this.animPlaying) return;
    this.animPlaying = true;
    this.lastAnimTick = performance.now();
    this.scheduleNext();
  }

  pause(): void {
    this.animPlaying = false;
    if (this.rafAnimHandle !== null) {
      cancelAnimationFrame(this.rafAnimHandle);
      this.rafAnimHandle = null;
    }
  }

  togglePlay(): void {
    if (this.animPlaying) this.pause();
    else this.play();
  }

  /** 逐帧步进（暂停状态下） */
  stepFrame(delta: number): void {
    if (!this.isAnimation) return;
    this.pause();
    const n = this.animFrames.length;
    this.animIndex = ((this.animIndex + delta) % n + n) % n;
    this.drawFrame();
  }

  /** 跳到指定帧（0-based） */
  seekFrame(index: number): void {
    if (!this.isAnimation) return;
    this.pause();
    const n = this.animFrames.length;
    this.animIndex = clamp(index, 0, n - 1);
    this.drawFrame();
  }

  // ---------- 内部 ----------

  private scheduleNext(): void {
    if (!this.animPlaying) return;
    if (this.rafAnimHandle !== null) {
      cancelAnimationFrame(this.rafAnimHandle);
      this.rafAnimHandle = null;
    }
    const loop = (now: number) => {
      if (!this.animPlaying) return;
      const targetDelay = Math.max(this.animDelays[this.animIndex] ?? 100, 16);
      if (now - this.lastAnimTick >= targetDelay) {
        this.lastAnimTick = now;
        this.animIndex = (this.animIndex + 1) % this.animFrames.length;
        this.drawFrame();
      }
      this.rafAnimHandle = requestAnimationFrame(loop);
    };
    this.rafAnimHandle = requestAnimationFrame(loop);
  }

  private drawFrame(): void {
    const bmp = this.animFrames[this.animIndex];
    if (!bmp) return;
    // 各帧统一按首帧画布尺寸拉伸绘制（避免重置 canvas 尺寸破坏 transform）
    this.ctx.drawImage(bmp, 0, 0, this.canvas.width, this.canvas.height);
    this.onFrameChange?.(this.animIndex + 1, this.animFrames.length);
  }

  private stopAnimation(): void {
    this.animPlaying = false;
    if (this.rafAnimHandle !== null) {
      cancelAnimationFrame(this.rafAnimHandle);
      this.rafAnimHandle = null;
    }
    // 显式释放位图（ImageBitmap 持有解码内存，等 GC 不可控；长 GIF 可达数百帧）
    for (const bmp of this.animFrames) bmp.close();
    this.animFrames = [];
    this.animDelays = [];
    this.animIndex = 0;
  }

  private resetTransform(): void {
    this.loaded = false;
    this.fitMode = "fit";
    this.baseScale = 1;
    this.userScale = 1;
    this.rotation = 0;
    this.rotTotal = 0;
    this.flipH = false;
    this.flipV = false;
    this.stage.classList.remove("pannable", "dragging", "animating");
    this.imgA.classList.remove("animating");
    this.imgB.classList.remove("animating");
    this.canvas.classList.remove("animating");
  }

  /** 作废进行中的静态加载：静默 resolve，让新加载覆盖显示 */
  private settlePending(): void {
    if (this.pending) {
      const p = this.pending;
      this.pending = null;
      window.clearTimeout(this.staticLoadTimer);
      p.resolve();
    }
  }

  private fit(): void {
    if (!this.loaded) return;
    const availW = this.stage.clientWidth;
    // 避让常驻标题栏（沉浸模式无标题栏，全屏可用）
    const availH = this.stage.clientHeight - (this.immersive ? 0 : TITLEBAR_H);
    const ew = this.rotation % 180 === 0 ? this.naturalW : this.naturalH;
    const eh = this.rotation % 180 === 0 ? this.naturalH : this.naturalW;
    if (availW <= 0 || availH <= 0 || ew <= 0 || eh <= 0) return;
    // 保持图片最大的展示能力：等比缩放至触达窗口边界（小图放大、大图缩小，不设 1× 封顶）
    this.baseScale = Math.min(availW / ew, availH / eh);
    this.cx = availW / 2;
    this.cy = (this.immersive ? 0 : TITLEBAR_H) + availH / 2;
    this.apply();
  }

  private apply(): void {
    if (!this.loaded) return;
    const el = this.active === "img" ? this.activeImg : this.canvas;
    const s = this.currentScale;
    const fx = this.flipH ? -1 : 1;
    const fy = this.flipV ? -1 : 1;
    el.style.transform =
      `translate(${this.cx}px, ${this.cy}px) ` +
      `rotate(${this.rotTotal}deg) ` +
      `scale(${s * fx}, ${s * fy}) ` +
      `translate(${-this.naturalW / 2}px, ${-this.naturalH / 2}px)`;
  }

  /** 平移范围约束：图片至少与视口保留 24px 重叠 */
  private clampPan(): void {
    const s = this.currentScale;
    const ew = (this.rotation % 180 === 0 ? this.naturalW : this.naturalH) * s;
    const eh = (this.rotation % 180 === 0 ? this.naturalH : this.naturalW) * s;
    const margin = PAN_MARGIN;
    const minX = margin - ew / 2;
    const maxX = this.stage.clientWidth - margin + ew / 2;
    const minY = margin - eh / 2;
    const maxY = this.stage.clientHeight - margin + eh / 2;
    this.cx = clamp(this.cx, Math.min(minX, maxX), Math.max(minX, maxX));
    this.cy = clamp(this.cy, Math.min(minY, maxY), Math.max(minY, maxY));
  }

  /** rAF 合并平移写入：高频 mousemove 只每帧应用一次 transform */
  private scheduleApply(): void {
    if (this.rafHandle !== null) return;
    this.rafHandle = requestAnimationFrame(() => {
      this.rafHandle = null;
      if (!this.loaded) return;
      this.clampPan();
      this.apply();
    });
  }

  /** 旋转/翻转时的 200ms ease-in-out 过渡 */
  private animateTransform(): void {
    const el = this.active === "img" ? this.activeImg : this.canvas;
    el.classList.add("animating");
    window.clearTimeout(this.transformTimer);
    this.transformTimer = window.setTimeout(() => {
      el.classList.remove("animating");
    }, ROTATE_MS);
  }
}

const TITLEBAR_H = 32;

/** 平移边距：图片边缘与视口保留的最小间距（isPannable/clampPan 共用） */
const PAN_MARGIN = 24;

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
