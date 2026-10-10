/**
 * 图片呈现 owner（前端深模块）：代次守卫 + 通道分发 + 时序 + 清理 + UI 同步的唯一入口。
 * main.ts 只保留接线（window/input/slideshow/cache/license），呈现策略全部收敛于此。
 * 解码与双缓冲委托 Viewer.show（C2 缝）；Tauri invoke 停在命令边缘，永不出接口。
 * 跨切面协作只经注入闭包（onShown/onProRequired），不依赖 Slideshow/WindowState/LicenseService。
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { FitMode, Viewer } from "./viewer";
import type { UI } from "./ui";
import { needsIpc, parseLoadEnvelope } from "./types";
import type { BrowseState, LoadEnvelope, NavResult } from "./types";
import { PrefetchPool } from "./prefetch";

export interface ImagePresenterOptions {
  /** 成功显示一张图片后（导航/直显通用，调用面接 slideshow.resetTimer） */
  onShown?: () => void;
  /** 导航撞上付费边界时（调用面接解锁对话框） */
  onProRequired?: () => void;
}

type FrameOp = "first" | "prev" | "toggle" | "next" | "last";

/** 边界 Toast 文案（图片级 / 文件夹级） */
const BOUNDARY_TEXT: Record<string, string> = {
  "first-image": "已经是第一张了",
  "last-image": "已经是最后一张了",
  "first-folder": "已经是第一个文件夹了",
  "last-folder": "已经是最后一个文件夹了",
  "pro-required": "跨文件夹浏览是专业版功能",
};

/** 潜在动画格式（原生 <img> 播放；仅这些格式需要 check_animation 判定） */
const ANIM_EXTS = new Set(["gif", "png", "webp"]);

/** check_animation 结果缓存：同路径重复显示免重复 IPC + 文件扫描；
 *  会话内至多保留 1000 条（满时删最旧一条近似 LRU，防浏览大文件夹时无界增长） */
const ANIM_CHECK_CACHE_MAX = 1000;
/** RAW 全量解码防抖：停留超过此时长（或放大到 1:1）才请求全量，避免翻页轰炸 */
const RAW_UPGRADE_DELAY_MS = 350;
/** get_context 防抖：快速翻页时合并为一次预取 */
const CONTEXT_DEBOUNCE_MS = 100;

export class ImagePresenter {
  /** 统一呈现代次（替代 showSeq；每次 show 自增，await 后复核，过期静默丢弃） */
  private gen = 0;
  /** 导航单飞：进行中只记最新 pending（替代 navInFlight/navPending 二元组） */
  private navInFlight = false;
  private navPending: (() => Promise<NavResult>) | null = null;
  /** 最近一次浏览状态（解锁重开当前图片用；调用面经 lastPath 只读） */
  private lastState: BrowseState | null = null;
  private currentDims: { w: number; h: number } | null = null;
  /** 当前图片是否为原生 <img> 播放中的动画（帧条交互时按需拆帧） */
  private animCandidate = false;
  private readonly animCheckCache = new Map<string, boolean>();
  private rawTimer: number | undefined;
  private ctxTimer: number | undefined;
  private readonly prefetch = new PrefetchPool();

  constructor(
    private readonly viewer: Viewer,
    private readonly ui: UI,
    private readonly opts: ImagePresenterOptions = {},
  ) {
    // 帧条策略一次接线：按钮 DOM 留在 UI 缝内，op 语义归此模块
    this.ui.onFrameAction((op) => this.onFrameOp(op));
    this.viewer.onFrameChange = (index, total) => this.ui.updateFrameCount(index, total);
    // 变换联动内聚：按钮同步 + 放大超 100% 立即提全量（原 bindEvents 订阅迁移至此）
    this.viewer.onStateChange = () => {
      this.syncZoom();
      if (this.viewer.currentScale > 1) this.triggerImmediateUpgrade();
    };
    this.syncZoom();
    this.ui.setFramePlaying(false);
    // 后台枚举完成：刷新信息条计数（图片不重载），兄弟文件夹预取防抖合并
    void listen<BrowseState>("browse://scan-ready", (event) => {
      const st = event.payload;
      this.ui.updateInfo(st, this.currentDims);
      this.ui.setSlideshowProgress(st.global_index + 1, st.global_total);
      this.scheduleContext();
    });
  }

  /** 当前图片路径（license 解锁重开用；无图时 null） */
  get lastPath(): string | null {
    return this.lastState?.path ?? null;
  }

  /** 直接显示（初态、打开路径、幻灯片推进共用）。显示失败只 Toast，永不抛错。 */
  async show(state: BrowseState): Promise<void> {
    this.lastState = state;
    const gen = ++this.gen;
    this.ui.setEmpty(false);
    this.ui.updateTitleFile(state.file_name);
    const winTitle = `${state.file_name} - ZViewer`;
    document.title = winTitle;
    void getCurrentWindow().setTitle(winTitle).catch(() => undefined);
    this.ui.updateInfo(state, null);
    this.currentDims = null;
    this.animCandidate = false;
    window.clearTimeout(this.rawTimer);
    this.rawTimer = undefined;
    this.ui.setFrameBarVisible(false);
    this.ui.setFramePlaying(false);

    // 预取下一批上下文（asset 预热 + Rust 缓存）；防抖合并快速翻页
    this.scheduleContext();

    // asset 快速通道：浏览器原生解码直接 convertFileSrc，跳过 IPC
    if (!needsIpc(state.path)) {
      const ext = state.path.split(".").pop()?.toLowerCase() ?? "";
      // 解码与动画判定并发：先启动判定 IPC，再等显示，避免串行等待
      const animatedP = this.checkAnimated(state.path, ext);
      try {
        await this.viewer.show({ kind: "url", url: convertFileSrc(state.path) });
      } catch (err) {
        console.error(`[render] failed: ${String(err)}`);
        if (gen === this.gen) {
          this.ui.showToast(`无法显示图片：${String(err)}`);
        }
        return;
      }
      if (gen !== this.gen) return;
      this.currentDims = { w: this.viewer.naturalWidth, h: this.viewer.naturalHeight };
      this.ui.updateInfo(state, this.currentDims);
      // 动画文件：原生播放中，亮出帧条（计数未知，交互时按需拆帧）
      if (await animatedP) {
        if (gen !== this.gen) return; // await 期间被抢占
        this.animCandidate = true;
        this.ui.setFrameBarVisible(true);
        this.ui.setFrameCountUnknown();
        this.ui.setFramePlaying(true);
      }
      this.ui.setSlideshowProgress(state.global_index + 1, state.global_total);
      // 埋点：asset 通道显示成功（浏览器原生解码不经 load_image，必须单独计数）
      void invoke("record_view", { path: state.path }).catch(() => undefined);
      return;
    }

    // IPC 通道：RAW/TIFF 由 Rust 解码，信封经 Viewer 统一缝显示
    this.ui.beginDecoding();
    try {
      let header: LoadEnvelope;
      let payload: Uint8Array;
      try {
        const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: false });
        ({ header, payload } = parseLoadEnvelope(buf));
      } catch (err) {
        if (gen === this.gen) this.ui.showToast(`无法加载图片：${String(err)}`);
        return;
      }
      if (gen !== this.gen) return;

      try {
        await this.viewer.show({ kind: "envelope", header, payload });
      } catch (err) {
        if (gen === this.gen) {
          this.ui.showToast(`无法显示图片：${String(err)}`);
          this.ui.setFrameBarVisible(false);
        }
        return;
      }
      if (gen !== this.gen) return;

      if (header.isPreview) {
        this.scheduleUpgrade(state, gen);
      }

      this.currentDims = { w: this.viewer.naturalWidth, h: this.viewer.naturalHeight };
      this.ui.updateInfo(state, this.currentDims);
      // 动画分支已内聚于 Viewer：由呈现状态派生帧条可见性
      if (this.viewer.isAnimation) {
        this.ui.setFrameBarVisible(true);
      }
      this.ui.setFramePlaying(this.viewer.isPlaying);
      // 幻灯片进度计数（播放中实时更新）
      this.ui.setSlideshowProgress(state.global_index + 1, state.global_total);
      // 埋点：IPC 通道显示成功（RAW/动画；load_image 命令本身不再计数）
      void invoke("record_view", { path: state.path }).catch(() => undefined);
    } finally {
      this.ui.endDecoding();
    }
  }

  /** IPC 导航：单飞 + latest-wins 排队 + 边界处理。IPC 被拒则吞掉（与旧行为一致）。 */
  async navigate(task: () => Promise<NavResult>): Promise<void> {
    if (this.navInFlight) {
      this.navPending = task;
      return;
    }
    this.navInFlight = true;
    try {
      await this.navApply(task);
      while (this.navPending) {
        const next = this.navPending;
        this.navPending = null;
        await this.navApply(next);
      }
    } finally {
      this.navInFlight = false;
    }
  }

  /** 缩放模式：拥有立即 RAW 升级 + 按钮同步（工具栏/快捷键/双击共用） */
  setFitMode(mode: FitMode, anchorX?: number, anchorY?: number): void {
    if (mode === "actual") this.triggerImmediateUpgrade();
    this.viewer.setMode(mode, anchorX, anchorY);
    this.syncZoom();
  }

  /** 立即按当前上下文预热（缓存开启后调用） */
  refreshContext(): void {
    void this.warmContext();
  }

  /** 打开新上下文前清空预解码池 */
  clearContext(): void {
    this.prefetch.clear();
  }

  // ---------- 内部 ----------

  /** 单次导航：IPC → 边界 Toast / 显示 */
  private async navApply(task: () => Promise<NavResult>): Promise<void> {
    let result: NavResult;
    try {
      result = await task();
    } catch {
      return;
    }
    if (result.boundary) {
      // 撞边界：图片未变化，只弹 Toast，不重载图片（避免闪烁与变换重置）
      this.ui.showToast(BOUNDARY_TEXT[result.boundary] ?? "已经到边界了");
      // 专业版功能被锁定：交由调用面引导解锁
      if (result.boundary === "pro-required") this.opts.onProRequired?.();
      return;
    }
    if (result.state) {
      await this.show(result.state);
      // 手动翻页不中断幻灯片，但从当前图重置计时
      this.opts.onShown?.();
    }
  }

  /** 同步缩放按钮状态与信息条缩放读数：适应窗口仅纯 fit 时激活，手动缩放后置灰 */
  private syncZoom(): void {
    this.ui.setZoomButtons(this.viewer.isFit, this.viewer.mode === "actual");
    this.ui.setZoom(this.viewer.hasImage ? this.viewer.currentScale : null);
  }

  /** check_animation 按路径缓存：翻回同一张图时免重复 IPC + 后端文件扫描 */
  private checkAnimated(path: string, ext: string): Promise<boolean> {
    if (!ANIM_EXTS.has(ext)) return Promise.resolve(false);
    const cached = this.animCheckCache.get(path);
    if (cached !== undefined) return Promise.resolve(cached);
    return invoke<boolean>("check_animation", { path })
      .then((v) => {
        if (this.animCheckCache.size >= ANIM_CHECK_CACHE_MAX) {
          const oldest = this.animCheckCache.keys().next().value;
          if (oldest !== undefined) this.animCheckCache.delete(oldest);
        }
        this.animCheckCache.set(path, v);
        return v;
      })
      .catch(() => false);
  }

  /** 防抖调度 RAW 全量解码：快速翻页只展示内嵌预览，仅停留超 350ms 才请求全量 */
  private scheduleUpgrade(state: BrowseState, gen: number, delay = RAW_UPGRADE_DELAY_MS): void {
    window.clearTimeout(this.rawTimer);
    this.rawTimer = window.setTimeout(() => {
      if (gen === this.gen) {
        void this.upgradeToFull(state, gen);
      }
    }, delay);
  }

  private triggerImmediateUpgrade(): void {
    if (this.rawTimer !== undefined && this.lastState) {
      window.clearTimeout(this.rawTimer);
      this.rawTimer = undefined;
      void this.upgradeToFull(this.lastState, this.gen);
    }
  }

  /** RAW 内嵌预览已显示：后台拉取全量并以 keepTransform 替换；失败时预览由 Viewer 保留 */
  private async upgradeToFull(state: BrowseState, gen: number): Promise<void> {
    this.ui.beginDecoding();
    try {
      const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: true });
      if (gen !== this.gen) return;
      const { header, payload } = parseLoadEnvelope(buf);
      if (header.isPreview) return; // 防御：仍返回预览（异常路径），保持现有显示
      if (gen !== this.gen) return;
      try {
        await this.viewer.show({ kind: "envelope", header, payload, keepTransform: true });
      } catch {
        // 全量失败：预览仍在（keep 路径下 Viewer 不清空旧画面），静默保留
        return;
      }
      if (gen !== this.gen) return;
      // 信息条尺寸更新为全量尺寸
      this.currentDims = { w: this.viewer.naturalWidth, h: this.viewer.naturalHeight };
      this.ui.updateInfo(state, this.currentDims);
    } finally {
      this.ui.endDecoding();
    }
  }

  /** 获取当前上下文路径并预热（asset 预解码池；RAW 邻居由 Rust 侧自动预取） */
  private async warmContext(): Promise<void> {
    try {
      const paths = await invoke<string[]>("get_context");
      this.prefetch.warm(paths, needsIpc);
    } catch {
      /* 忽略失败 */
    }
  }

  /** refreshContext 防抖：快速翻页时合并为一次 get_context */
  private scheduleContext(delay = CONTEXT_DEBOUNCE_MS): void {
    window.clearTimeout(this.ctxTimer);
    this.ctxTimer = window.setTimeout(() => void this.warmContext(), delay);
  }

  /** 帧条策略：帧已就绪直接控制；原生播放中先按需拆帧，完成后执行同一动作 */
  private onFrameOp(op: FrameOp): void {
    const run = (): void => {
      switch (op) {
        case "first":
          this.viewer.seekFrame(0);
          break;
        case "prev":
          this.viewer.stepFrame(-1);
          break;
        case "toggle":
          this.viewer.togglePlay();
          break;
        case "next":
          this.viewer.stepFrame(1);
          break;
        case "last":
          this.viewer.seekFrame(Number.MAX_SAFE_INTEGER);
          break;
      }
    };
    if (this.viewer.isAnimation) {
      run();
      this.ui.setFramePlaying(this.viewer.isPlaying);
      return;
    }
    if (!this.animCandidate) return;
    void this.ensureAnimation()
      .then(() => {
        if (this.viewer.isAnimation) {
          run();
          this.ui.setFramePlaying(this.viewer.isPlaying);
        }
      })
      .catch((err) => this.ui.showToast(String(err)));
  }

  /** 按需拆帧：原生播放切换为 canvas 逐帧（ImageDecoder 优先，Rust 通道回退） */
  private async ensureAnimation(): Promise<void> {
    const state = this.lastState;
    if (!state) return;
    const gen = this.gen; // 代次快照：await 期间切图则丢弃，防旧动画覆盖新图显示
    this.ui.setFrameLoading(true);
    try {
      // 优先 Web 原生 ImageDecoder（硬件加速，免 Rust 端单帧重编码）
      if (typeof ImageDecoder !== "undefined") {
        try {
          const ext = state.path.split(".").pop()?.toLowerCase() ?? "";
          const mime = ext === "gif" ? "image/gif" : ext === "webp" ? "image/webp" : "image/png";
          if (await ImageDecoder.isTypeSupported(mime)) {
            const resp = await fetch(convertFileSrc(state.path));
            if (!resp.ok) throw new Error("无法读取动画文件");
            const blob = await resp.blob();
            if (gen !== this.gen) return;

            const decoder = new ImageDecoder({ data: blob.stream(), type: mime });
            await decoder.tracks.ready;
            const track = decoder.tracks.selectedTrack;
            const frameCount = track?.frameCount ?? 0;

            if (frameCount <= 1) {
              this.ui.setFrameBarVisible(false);
              this.animCandidate = false;
              this.ui.showToast("该文件不是多帧动画");
              return;
            }

            const bitmaps: ImageBitmap[] = [];
            const delays: number[] = [];
            for (let i = 0; i < frameCount; i++) {
              if (gen !== this.gen) {
                for (const b of bitmaps) b.close();
                return;
              }
              const res = await decoder.decode({ frameIndex: i });
              const bmp = await createImageBitmap(res.image);
              res.image.close();
              bitmaps.push(bmp);
              const d = res.image.duration ? Math.max(Math.round(res.image.duration / 1000), 10) : 100;
              delays.push(d);
            }

            if (gen !== this.gen) {
              for (const b of bitmaps) b.close();
              return;
            }

            this.ui.setFrameBarVisible(true);
            await this.viewer.show({ kind: "frames", bitmaps, delays });
            return;
          }
        } catch {
          /* 降级走 Rust 通道 */
        }
      }

      // 回退通道：Rust 解码服务（信封经统一缝显示，单帧与否由呈现状态派生）
      const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: false });
      if (gen !== this.gen) return;
      const { header, payload } = parseLoadEnvelope(buf);
      if (gen !== this.gen) return;
      this.ui.setFrameBarVisible(true);
      if (gen !== this.gen) return; // await 期间被抢占：不覆盖新图
      await this.viewer.show({ kind: "envelope", header, payload });
      if (gen !== this.gen) return;
      if (!this.viewer.isAnimation) {
        // 单帧文件：收起帧条并提示
        this.ui.setFrameBarVisible(false);
        this.animCandidate = false;
        this.ui.showToast("该文件不是多帧动画");
        return;
      }
    } finally {
      this.ui.setFrameLoading(false);
    }
  }
}
