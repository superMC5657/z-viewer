/**
 * 图片呈现 owner（前端深模块）：代次守卫 + 通道分发 + 时序 + 清理 + UI 同步的唯一入口。
 * main.ts 只保留接线（window/input/slideshow/cache/license），呈现策略全部收敛于此。
 * 解码与双缓冲委托 Viewer.show（C2 缝）；Tauri invoke 停在命令边缘，永不出接口。
 * 跨切面协作只经注入闭包（onShown/onProRequired），不依赖 Slideshow/WindowState/LicenseService。
 * 新鲜度真相源（本模块唯一）：gen 为单一代次钟，rawTimer/ctxTimer/animCheckCache
 * LRU/预解码池状态全部归此管理；Viewer 侧 loadSeq/pending/2.5s 兜底仅为执行层回防
 * （同代重入串行化），不做第二真相源。show/navigate 对外保持薄 face。
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { ContentState, FitMode, Viewer } from "./viewer";
import type { UI } from "./ui";
import { isPotentialAnim, needsIpc, parseLoadEnvelope } from "./types";
import type { BoundaryKind, BrowseState, LoadEnvelope, NavResult } from "./types";

export interface ImagePresenterOptions {
  /** 成功显示一张图片后（导航/直显通用，调用面接 presence.notifyManualNav） */
  onShown?: () => void;
  /** 导航撞上付费边界时（调用面接解锁对话框） */
  onProRequired?: () => void;
}

type FrameOp = "first" | "prev" | "toggle" | "next" | "last";

/** 边界 Toast 文案（图片级 / 文件夹级；键与 types.ts BoundaryKind 对齐） */
const BOUNDARY_TEXT: Record<BoundaryKind, string> = {
  "first-image": "已经是第一张了",
  "last-image": "已经是最后一张了",
  "first-folder": "已经是第一个文件夹了",
  "last-folder": "已经是最后一个文件夹了",
  "pro-required": "跨文件夹浏览是专业版功能",
};

/** check_animation 结果缓存：同路径重复显示免重复 IPC + 文件扫描；
 *  会话内至多保留 1000 条（满时删最旧一条近似 LRU，防浏览大文件夹时无界增长） */
const ANIM_CHECK_CACHE_MAX = 1000;
/** RAW 全量解码防抖：停留超过此时长（或放大到 1:1）才请求全量，避免翻页轰炸 */
const RAW_UPGRADE_DELAY_MS = 350;
/** get_context 防抖：快速翻页时合并为一次预取 */
const CONTEXT_DEBOUNCE_MS = 100;
/** 前端预解码池上限（warmContext 唯一入口驱动；隐藏 <img> 不挂载 DOM，仅占解码缓存） */
const PREFETCH_POOL_MAX = 10;

export class ImagePresenter {
  // ---------- 新鲜度真相源（单一代次钟 + 所属计时器/缓存/池状态，同一 locality） ----------
  /** 统一呈现代次（替代 showSeq；每次 show 自增，await 后复核，过期静默丢弃） */
  private gen = 0;
  /** 导航单飞：进行中只记最新 pending（替代 navInFlight/navPending 二元组） */
  private navInFlight = false;
  private navPending: (() => Promise<NavResult>) | null = null;
  /** 最近一次浏览状态（解锁重开当前图片用；调用面经 lastPath 只读） */
  private lastState: BrowseState | null = null;
  private currentDims: { w: number; h: number } | null = null;
  /** Viewer 推送的内容快照（onContent 写入；帧条显隐/播放图标/尺寸读数由此派生，不轮询 Viewer） */
  private lastContent: ContentState | null = null;
  /** 当前图片是否为原生 <img> 播放中的动画（帧条交互时按需拆帧） */
  private animCandidate = false;
  private readonly animCheckCache = new Map<string, boolean>();
  private rawTimer: number | undefined;
  private ctxTimer: number | undefined;
  /** 前端预解码池状态（warmContext 唯一入口驱动；原 PrefetchPool 折叠入此，deletion test 通过） */
  private poolImgs: HTMLImageElement[] = [];
  private poolLoading = new Set<string>();
  /** 深放大真像素状态（随 show 重置）：nativeLoaded=已升级或无需升级；
   *  currentCapped=当前显示位图来自降采样（后端 envelope 标记） */
  private nativeLoaded = false;
  private nativePending = false;
  private nativeFailed = false;
  private currentCapped = false;

  constructor(
    private readonly viewer: Viewer,
    private readonly ui: UI,
    private readonly opts: ImagePresenterOptions = {},
  ) {
    // 帧条策略一次接线：按钮 DOM 留在 UI 缝内，op 语义归此模块
    this.ui.onFrameAction((op) => this.onFrameOp(op));
    this.viewer.onFrameChange = (index, total) => this.ui.updateFrameCount(index, total);
    // 内容状态推送一次接线：存快照 + 直驱帧条显隐/播放图标（调用方不再轮询 Viewer）
    this.viewer.onContent = (s) => {
      this.lastContent = s;
      this.ui.setFrameBarVisible(s.animated);
      this.ui.setFramePlaying(s.playing);
    };
    // 变换联动内聚：按钮同步 + 放大超 100% 立即提全量（RAW 预览）
    // + 深放大越过后按需拉取原始分辨率（真像素升级，票 07）
    this.viewer.onStateChange = () => {
      this.syncZoom();
      if (this.viewer.currentScale > 1) this.triggerImmediateUpgrade();
      this.maybeFetchNative();
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

  /** 内容快照重置（经方法边界：show 头的直接赋 null 会被 CFA 带入后续读取致 never） */
  private resetContent(): void {
    this.lastContent = null;
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
    this.resetContent();
    this.animCandidate = false;
    this.nativeLoaded = false;
    this.nativePending = false;
    this.nativeFailed = false;
    this.currentCapped = false;
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
      const snap: ContentState | null = this.lastContent;
      this.currentDims = snap ? { w: snap.width, h: snap.height } : null;
      this.ui.updateInfo(state, this.currentDims);
      // 动画文件：原生播放中，亮出帧条（计数未知，交互时按需拆帧）
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
        }
        return;
      }
      if (gen !== this.gen) return;
      this.currentCapped = header.capped;

      if (header.isPreview) {
        this.scheduleUpgrade(state, gen);
      }

      const snap: ContentState | null = this.lastContent;
      this.currentDims = snap ? { w: snap.width, h: snap.height } : null;
      this.ui.updateInfo(state, this.currentDims);
      // 帧条显隐与播放图标已由 onContent 推送直驱（不再轮询 isAnimation/isPlaying）
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

  /** 打开新上下文前清空预解码池（池状态与代次钟同 locality） */
  clearContext(): void {
    this.clearPool();
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
    if (!isPotentialAnim(ext)) return Promise.resolve(false);
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
        // 全量拉取失败只保留预览（Viewer 不清空旧画面），此处吞掉拒绝以保持静默
        void this.upgradeToFull(state, gen).catch(() => undefined);
      }
    }, delay);
  }

  private triggerImmediateUpgrade(): void {
    if (this.rawTimer !== undefined && this.lastState) {
      window.clearTimeout(this.rawTimer);
      this.rawTimer = undefined;
      void this.upgradeToFull(this.lastState, this.gen).catch(() => undefined);
    }
  }

  /** RAW 内嵌预览已显示：后台拉取全量并升级替换（变换保持归 Viewer）；失败时预览由 Viewer 保留 */
  private async upgradeToFull(state: BrowseState, gen: number): Promise<void> {
    this.ui.beginDecoding();
    try {
      const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: true });
      if (gen !== this.gen) return;
      const { header, payload } = parseLoadEnvelope(buf);
      this.currentCapped = header.capped;
      if (header.isPreview) return; // 防御：仍返回预览（异常路径），保持现有显示
      if (gen !== this.gen) return;
      try {
        await this.viewer.showUpgrade(header, payload);
      } catch {
        // 全量失败：预览仍在（升级路径下 Viewer 不清空旧画面），静默保留
        return;
      }
      if (gen !== this.gen) return;
      // 信息条尺寸更新为全量尺寸
      const snap: ContentState | null = this.lastContent;
      this.currentDims = snap ? { w: snap.width, h: snap.height } : null;
      this.ui.updateInfo(state, this.currentDims);
    } finally {
      this.ui.endDecoding();
    }
  }

  /** 深放大真像素（票 07）：显示倍率越过当前位图 1:1 且位图来自降采样时，
   *  按需拉取原始分辨率解码（≤8192 长边，不写 LRU）并经 showUpgrade 换图——
   *  屏幕视图逐像素不变、仅更锐。粘性升级（不回退），切图/失败即重置。 */
  private maybeFetchNative(): void {
    if (this.nativeLoaded || this.nativePending || this.nativeFailed) return;
    const state = this.lastState;
    const content = this.lastContent;
    if (!state || !content || content.animated) return;
    if (!needsIpc(state.path)) return; // asset 通道本就持有原文件，无升级概念
    if (!this.currentCapped) return; // 显示位图未降采样，无原图可换
    if (this.viewer.currentScale <= 1) return; // 未越过当前位图 1:1
    const gen = this.gen;
    this.nativePending = true;
    void (async () => {
      this.ui.beginDecoding();
      try {
        const buf = await invoke<ArrayBuffer>("load_image", {
          path: state.path,
          full: true,
          original: true,
        });
        if (gen !== this.gen) return;
        const { header, payload } = parseLoadEnvelope(buf);
        if (gen !== this.gen) return;
        if (header.isPreview) return; // 防御：异常路径仍返回预览
        if (!header.capped) {
          this.nativeLoaded = true; // 原图本就不超过显示帽，无需升级
          return;
        }
        try {
          await this.viewer.showUpgrade(header, payload);
        } catch {
          return; // 升级失败保留现有画面（Viewer 升级路径不清空旧图）
        }
        if (gen !== this.gen) return;
        this.nativeLoaded = true;
        // 信息条尺寸更新为原始分辨率
        const snap: ContentState | null = this.lastContent;
        this.currentDims = snap ? { w: snap.width, h: snap.height } : null;
        this.ui.updateInfo(state, this.currentDims);
      } catch {
        this.nativeFailed = true; // 静默失败，本图不再重试
      } finally {
        this.nativePending = false;
        this.ui.endDecoding();
      }
    })();
  }

  /** 获取当前上下文路径并预热（asset 预解码池；RAW 邻居由 Rust 侧自动预取） */
  private async warmContext(): Promise<void> {
    try {
      const paths = await invoke<string[]>("get_context");
      this.warmPool(paths);
    } catch {
      /* 忽略失败 */
    }
  }

  /** refreshContext 防抖：快速翻页时合并为一次 get_context */
  private scheduleContext(delay = CONTEXT_DEBOUNCE_MS): void {
    window.clearTimeout(this.ctxTimer);
    this.ctxTimer = window.setTimeout(() => void this.warmContext(), delay);
  }

  /** 预解码池预热（asset 通道提前隐藏 <img> 加载并 decode()，切图零解码瞬时显示；
   *  动画/RAW 走 Rust 缓存不在此预热；原 PrefetchPool.warm 逐字搬运，deletion test 通过） */
  private warmPool(paths: string[]): void {
    for (const p of paths) {
      if (this.poolLoading.has(p)) continue;
      if (needsIpc(p)) continue;
      const url = convertFileSrc(p);
      if (this.poolImgs.some((i) => i.dataset.src === url)) continue;
      const img = new Image();
      img.dataset.src = url;
      img.dataset.path = p;
      img.decoding = "async";
      this.poolLoading.add(p);
      img.src = url;
      img
        .decode()
        .then(() => this.poolLoading.delete(p))
        .catch(() => this.poolLoading.delete(p));
      this.poolImgs.push(img);
      this.trimPool();
    }
  }

  /** 裁剪池（保留最近 PREFETCH_POOL_MAX 个），被裁剪的加载中路径同步释放登记 */
  private trimPool(): void {
    if (this.poolImgs.length <= PREFETCH_POOL_MAX) return;
    const overflow = this.poolImgs.length - PREFETCH_POOL_MAX;
    for (let i = 0; i < overflow; i++) {
      const img = this.poolImgs.shift()!;
      const path = img.dataset.path;
      if (path) this.poolLoading.delete(path);
      // 只 removeAttribute：置 src="" 会把空串解析为当前文档 URL，触发一次对页面本身的冗余请求
      img.removeAttribute("src");
    }
  }

  /** 清空池（打开新上下文时调用，释放解码缓存；原 PrefetchPool.clear 逐字搬运） */
  private clearPool(): void {
    for (const img of this.poolImgs) {
      img.removeAttribute("src");
    }
    this.poolImgs = [];
    this.poolLoading.clear();
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
    if (this.lastContent?.animated) {
      run();
      this.ui.setFramePlaying(this.lastContent?.playing ?? false);
      return;
    }
    if (!this.animCandidate) return;
    void this.ensureAnimation()
      .then(() => {
        if (this.lastContent?.animated) {
          run();
          this.ui.setFramePlaying(this.lastContent?.playing ?? false);
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
        const bitmaps: ImageBitmap[] = [];
        let decoder: ImageDecoder | null = null;
        // 单一清理路径：丢弃已得位图 + 关解码器（所有中途退出共用；成功路径只关
        // 解码器，位图所有权随 show({kind:"frames"}) 移交 Viewer）
        const dispose = (): void => {
          for (const b of bitmaps) b.close();
          bitmaps.length = 0;
          decoder?.close();
          decoder = null;
        };
        try {
          const ext = state.path.split(".").pop()?.toLowerCase() ?? "";
          const mime = ext === "gif" ? "image/gif" : ext === "webp" ? "image/webp" : "image/png";
          if (await ImageDecoder.isTypeSupported(mime)) {
            const resp = await fetch(convertFileSrc(state.path));
            if (!resp.ok) throw new Error("无法读取动画文件");
            const blob = await resp.blob();
            if (gen !== this.gen) return;

            decoder = new ImageDecoder({ data: blob.stream(), type: mime });
            await decoder.tracks.ready;
            const track = decoder.tracks.selectedTrack;
            const frameCount = track?.frameCount ?? 0;

            if (frameCount <= 1) {
              dispose();
              this.ui.setFrameBarVisible(false);
              this.animCandidate = false;
              this.ui.showToast("该文件不是多帧动画");
              return;
            }

            const delays: number[] = [];
            for (let i = 0; i < frameCount; i++) {
              if (gen !== this.gen) {
                dispose();
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
              dispose();
              return;
            }

            decoder.close();
            decoder = null;
            this.ui.setFrameBarVisible(true);
            await this.viewer.show({ kind: "frames", bitmaps, delays });
            return;
          }
        } catch {
          // 异常（含中途解码失败）：释放已得位图与解码器，降级走 Rust 通道
          dispose();
        }
      }

      // 回退通道：Rust 解码服务（信封经统一缝显示，单帧与否由呈现状态派生）
      const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: false });
      if (gen !== this.gen) return;
      const { header, payload } = parseLoadEnvelope(buf);
      if (gen !== this.gen) return; // await 期间被抢占：不覆盖新图
      await this.viewer.show({ kind: "envelope", header, payload });
      if (gen !== this.gen) return;
      if (this.lastContent?.animated) {
        // 多帧动画：此时才亮出帧条（静态图不闪现帧条）
        this.ui.setFrameBarVisible(true);
        return;
      }
      // 单帧文件：收起帧条并提示
      this.ui.setFrameBarVisible(false);
      this.animCandidate = false;
      this.ui.showToast("该文件不是多帧动画");
      return;
    } finally {
      this.ui.setFrameLoading(false);
    }
  }
}
