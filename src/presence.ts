/**
 * 可见性 + 意图 + 节奏的唯一深模块（前端 C3）。
 * 收敛三处旧主人：overlay-manager（闲置/分组/悬停）、window-state（沉浸/置顶）、slideshow（v4 节拍器）。
 * 方向向内：main → Presence → host 回调 → presenter/viewer/Tauri。
 * 本模块不导入 ui/license/updater/viewer/window-state；窗口经 host.window，是否专业版经按次 {pro}。
 * 内容节点零拥有：浮层容器只查询一次，文本/图标写入归 UI/presenter。
 * 节拍唯一主人：start/stop/tick/advance/reset/恢复/退出清理全归此模块，
 * 调用面只做开始/停止/播放条接线与内容适配（onAdvance 内容体只做薄转发）。
 */

export type PresenceMode = "browse" | "slideshow";

export type InputIntent =
  | "prev" | "next" | "folder-first" | "folder-prev" | "folder-next" | "folder-last"
  | "mode-fit" | "mode-actual" | "zoom-in" | "zoom-out"
  | "rotate-cw" | "rotate-ccw" | "flip-h" | "flip-v"
  | "immersive-toggle" | "immersive-exit" | "pin-toggle"
  | "slideshow-toggle" | "slideshow-exit" | "wake";

export type NavigateKind = "prev" | "next" | "folder-first" | "folder-prev" | "folder-next" | "folder-last";

export interface PresenceBars {
  infoBar: HTMLElement;
  toolbar: HTMLElement;
  frameBar: HTMLElement;
  slideshowBar: HTMLElement;
}

export interface PresenceHost {
  /** 像素：viewer 调用 + setFitMode 包装（缩放按钮同步 rides 统一订阅） */
  applyView(intent: InputIntent, anchor?: { x: number; y: number }): void;
  /** 导航执行（presenter.navigate；文件夹预检归调用面，节拍重置 rides onShown） */
  navigate(kind: NavigateKind): void;
  /** 提示文案（toast/progress 由呈现层拥有，此处只发射） */
  notify(msg: string): void;
  /** 幻灯片模式翻转（调用面只同步计数文本） */
  onSlideshowMode(active: boolean): void;
  /** 幻灯片退出清理（调用面只重置进度计数；模式/缓存恢复顺序由本模块保证） */
  onSlideshowExit(): void;
  /** 当前缓存等级（进入时记录，退出恢复用；调用面只读值） */
  getCacheLevel(): number;
  /** 缓存提级/恢复执行（决策与记忆归本模块：进入传 true，退出传 false + 进入前等级） */
  onCacheElevation(active: boolean, elevatedFrom?: number | null): void;
  /** 沉浸翻转（调用面扇出到 Viewer/body/按钮） */
  onImmersiveChange(active: boolean): void;
  /** 置顶翻转（调用面同步按钮） */
  onPinChange(active: boolean): void;
  /** 弹窗关闭（打开时优先关弹窗，返回 true 即停止） */
  onDialogClose?(): boolean;
  window: {
    setFullscreen(b: boolean): Promise<void>;
    setAlwaysOnTop(b: boolean): Promise<void>;
  };
}

export interface PresenceOptions {
  /** 闲置隐藏时间（毫秒），默认 2000 */
  idleMs?: number;
  /** 唤醒高频节流时间（毫秒），默认 150 */
  throttleMs?: number;
  /** 是否专业版（仅 slideshow-toggle/start 时的提级决策用；实现永不导入 license） */
  isPro?: () => boolean;
}

/** 按住方向键的 repeat 限流（沿用旧值）：连发只放行每 120ms 一次 */
const NAV_REPEAT_MS = 120;
/** 幻灯片间隔持久化键（localStorage；沿用旧键名） */
const SS_INTERVAL_KEY = "ss-interval";
/** 幻灯片默认间隔（ms；与旧行为一致） */
const DEFAULT_INTERVAL_MS = 5000;

/** 读取持久化的幻灯片间隔（ms），无/非法回默认 5s（自 main.ts 迁移，校验逐字保留） */
export function loadSlideshowInterval(): number {
  try {
    const v = Number(localStorage.getItem(SS_INTERVAL_KEY));
    return Number.isFinite(v) && v >= 1000 ? v : DEFAULT_INTERVAL_MS;
  } catch {
    return DEFAULT_INTERVAL_MS;
  }
}

/** Promise 竞速 + 兜底定时器清理：主 Promise 先完成时 clearTimeout
 *  （否则兜底定时器每拍空转到点才释放，幻灯片播放中持续累积；自 main.ts 逐字迁移） */
export function raceWithTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
  let timer: number | undefined;
  const timeout = new Promise<null>((res) => {
    timer = window.setTimeout(() => res(null), ms);
  });
  return Promise.race([p, timeout]).finally(() => window.clearTimeout(timer));
}

/** 键盘事件 → 输入意图；非快捷键返回 null（调用面忽略，不 preventDefault）。
 *  纯映射（自 input.ts 逐字并入，归属 attachKeyboard；连续手势仍走调用面 direct viewer）。
 *  方向键翻页 / PgUp PgDn Home End 文件夹 / 空格-幻灯片 / R H V 1 0 / ↑↓ 缩放 / F 沉浸 / T 置顶 / Esc 退出 */
function keyToIntent(e: Pick<KeyboardEvent, "key" | "shiftKey">): InputIntent | null {
  switch (e.key) {
    case "ArrowLeft":
      return "prev";
    case "ArrowRight":
      return "next";
    case "PageUp":
      return "folder-prev";
    case "PageDown":
      return "folder-next";
    case "Home":
      return "folder-first";
    case "End":
      return "folder-last";
    case "ArrowUp":
      return "zoom-in";
    case "ArrowDown":
      return "zoom-out";
    case "1":
      return "mode-actual";
    case "0":
      return "mode-fit";
    case "r":
    case "R":
      return e.shiftKey ? "rotate-ccw" : "rotate-cw";
    case "h":
    case "H":
      return "flip-h";
    case "v":
    case "V":
      return "flip-v";
    case "F":
    case "f":
    case "F11":
      // 沉浸模式切换（草图 3.5）；F11 兼容需求文档
      return "immersive-toggle";
    case "Escape":
      // 弹窗优先（Presence 经 onDialogClose 判定）、否则退出沉浸
      return "immersive-exit";
    case "T":
    case "t":
      return "pin-toggle";
    case " ":
      // 幻灯片播放/暂停（草图 3.6）
      return "slideshow-toggle";
    default:
      return null;
  }
}

export class Presence {
  /** 浮层容器（构造时查询一次；唯一持有 overlay-bar 查找的位置） */
  readonly bars: PresenceBars;
  /** 节拍回调：返回 false 表示撞边界应停止（由调用面提供） */
  onAdvance: (() => Promise<boolean>) | null = null;
  /** 播放状态变化（true=开始 false=停止），调用面只同步按钮视觉 */
  onRunningChange: ((running: boolean) => void) | null = null;

  private mode: PresenceMode = "browse";
  private running = false;
  private immersive = false;
  private pinned = false;
  private immBusy = false;
  private pinBusy = false;
  private frameAvailable = false;
  private dialogOpen = false;

  private readonly idleMs: number;
  private readonly throttleMs: number;
  private idleTimer: number | undefined;
  private lastWakeAt = 0;
  private hoveringOverlay = false;

  // v4 节拍器（自 slideshow.ts 逐字迁移）：拍点固定推进 + 欠拍补跳
  private timerId: number | null = null;
  private intervalMs = 5000;
  private advancing = false;
  private pendingAdvance = false;
  private nextTickAt = 0;
  /** 进入幻灯片时记录的缓存等级（暂停→继续不覆盖，退出时恢复；免费/未提级时为 null） */
  private elevatedFrom: number | null = null;

  private detachKeyboard: (() => void) | null = null;
  private readonly disposers: Array<() => void> = [];

  constructor(
    private readonly host: PresenceHost,
    opts?: PresenceOptions,
  ) {
    this.idleMs = opts?.idleMs ?? 2000;
    this.throttleMs = opts?.throttleMs ?? 150;
    if (opts?.isPro) this.isPro = opts.isPro;
    this.bars = {
      infoBar: document.getElementById("info-bar")!,
      toolbar: document.getElementById("toolbar")!,
      frameBar: document.getElementById("frame-bar")!,
      slideshowBar: document.getElementById("slideshow-bar")!,
    };
    this.bindHover(this.bars.infoBar);
    this.bindHover(this.bars.toolbar);
    this.bindHover(this.bars.frameBar);
    this.bindHover(this.bars.slideshowBar);
  }

  private isPro: () => boolean = () => false;

  get currentMode(): PresenceMode {
    return this.mode;
  }

  get isRunning(): boolean {
    return this.running;
  }

  // ---------- 可见性（自 overlay-manager 折入，语义逐字保留） ----------

  /** 唤醒当前分组浮层并排定闲置隐藏；弹窗打开期间不翻转分组 */
  wake(): void {
    if (this.dialogOpen) return;
    const now = performance.now();
    const visible = this.isGroupVisible(this.mode);

    if (visible && now - this.lastWakeAt < this.throttleMs) {
      return;
    }
    this.lastWakeAt = now;

    if (!visible) {
      this.showGroup(this.mode);
    }

    this.scheduleIdleHide();
  }

  /** 立即隐藏所有浮层并清除闲置计时器 */
  hideAll(): void {
    this.hideEl(this.bars.infoBar);
    this.hideEl(this.bars.toolbar);
    this.hideEl(this.bars.frameBar);
    this.hideEl(this.bars.slideshowBar);
    this.clearIdleTimer();
  }

  /** 帧可用性（presenter 的帧条显隐经此路由；分组表内唯一条件） */
  setFrameAvailable(available: boolean): void {
    this.frameAvailable = available;
    if (!available) {
      this.hideEl(this.bars.frameBar);
    } else if (this.mode === "browse" && !this.isHidden(this.bars.infoBar)) {
      // 沿用旧 dance：仅当信息条可见时同步显示（闲置隐藏期间等 wake 唤醒）
      this.showEl(this.bars.frameBar);
    }
  }

  /** 弹窗显隐打点：打开时清计时+抑制，关闭时重排闲置隐藏 */
  setDialogOpen(open: boolean): void {
    this.dialogOpen = open;
    if (open) this.clearIdleTimer();
    else this.wake();
  }

  /** 模式唯一开关：翻转分组 → 显示控制条 → wake → 通知调用面 */
  setPresence(mode: PresenceMode): void {
    if (mode === this.mode) return;
    this.mode = mode;
    this.clearIdleTimer();
    if (mode === "slideshow") {
      this.hideGroup("browse");
      this.showGroup("slideshow");
      this.wake();
      this.host.onSlideshowMode(true);
    } else {
      this.hideGroup("slideshow");
      this.wake();
      this.host.onSlideshowMode(false);
    }
  }

  // ---------- 意图统一决策点 ----------

  /** 键盘/工具栏/侧键/双击全部生产 InputIntent；导航与缩放键不唤醒（专注看图） */
  dispatch(intent: InputIntent, anchor?: { x: number; y: number }): void {
    switch (intent) {
      case "prev":
      case "next":
      case "folder-first":
      case "folder-prev":
      case "folder-next":
      case "folder-last":
        // 节拍重置 rides presenter onShown → notifyManualNav（show 完成后锚定）
        this.host.navigate(intent);
        return;
      case "mode-fit":
      case "mode-actual":
      case "zoom-in":
      case "zoom-out":
      case "rotate-cw":
      case "rotate-ccw":
      case "flip-h":
      case "flip-v":
        // setFitMode 包装自带提全量+同步；其余经统一订阅收敛（单决策点）
        this.host.applyView(intent, anchor);
        return;
      case "immersive-toggle":
        void this.toggleImmersive();
        return;
      case "immersive-exit":
        void this.exitImmersive();
        return;
      case "pin-toggle":
        void this.togglePin();
        return;
      case "slideshow-toggle":
        this.toggleSlideshow();
        return;
      case "slideshow-exit":
        this.stopSlideshow();
        return;
      case "wake":
        this.wake();
        return;
    }
  }

  /** 键盘接线：repeat 限流 + 表单守卫 + Esc 弹窗优先；返回卸载函数 */
  attachKeyboard(): () => void {
    let lastNavAt = -Infinity;
    const onKeyDown = (e: KeyboardEvent): void => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      const intent = keyToIntent(e);
      if (!intent) return;
      e.preventDefault();
      if (intent === "prev" || intent === "next") {
        const now = performance.now();
        if (e.repeat && now - lastNavAt < NAV_REPEAT_MS) return;
        lastNavAt = now;
      }
      // Esc 优先级一次判定：弹窗开着先关弹窗，否则退出沉浸
      if (intent === "immersive-exit" && this.host.onDialogClose?.()) return;
      this.dispatch(intent);
    };
    document.addEventListener("keydown", onKeyDown);
    const detach = () => document.removeEventListener("keydown", onKeyDown);
    this.detachKeyboard = detach;
    return detach;
  }

  // ---------- 节奏（v4 节拍器唯一主人：start/stop/tick/advance/reset/恢复/退出清理） ----------

  /** 开始播放（暂停后继续沿用；首次进入且专业版时记录等级并提级缓存） */
  startSlideshow(): void {
    if (this.running) return;
    const entering = this.mode !== "slideshow";
    this.running = true;
    this.pendingAdvance = false;
    this.nextTickAt = performance.now() + this.intervalMs;
    this.onRunningChange?.(true);
    this.setPresence("slideshow");
    if (entering && this.isPro()) {
      // 首次进入记录播放前等级（暂停→继续不覆盖，恢复仍用最初的等级）
      this.elevatedFrom = this.host.getCacheLevel();
      this.host.onCacheElevation(true);
    }
    this.schedule();
  }

  /** 停止播放并退出幻灯片模式（含缓存恢复；暂停态调用同样安全） */
  stopSlideshow(): void {
    this.pausePlayback();
    this.setPresence("browse");
    const saved = this.elevatedFrom;
    this.elevatedFrom = null;
    this.host.onCacheElevation(false, saved);
  }

  /** 退出幻灯片模式，返回图片浏览（返回按钮/播放完/打开新图共用；幂等） */
  exitSlideshow(): void {
    if (this.mode === "browse") return;
    console.info("[slideshow] exited");
    this.stopSlideshow();
    this.host.onSlideshowExit();
  }

  toggleSlideshow(): void {
    if (this.running) this.pausePlayback();
    else this.startSlideshow();
  }

  /** 设置间隔（毫秒，持久化）：播放中立即生效，从当前时刻起算新间隔 */
  setIntervalMs(ms: number): void {
    this.intervalMs = ms;
    try {
      localStorage.setItem(SS_INTERVAL_KEY, String(ms));
    } catch {
      /* 忽略持久化失败 */
    }
    this.pendingAdvance = false; // 重新锚定拍点：旧欠拍作废
    if (this.running) {
      this.nextTickAt = performance.now() + this.intervalMs;
      this.schedule();
    }
  }

  /** 手动翻页/跳转后重置计时（不中断播放）：从当前时刻起算下一间隔 */
  notifyManualNav(): void {
    if (!this.running) return;
    this.pendingAdvance = false; // 手动翻页后重新锚定拍点，旧欠拍作废
    this.nextTickAt = performance.now() + this.intervalMs;
    this.schedule();
  }

  destroy(): void {
    this.clearTimer();
    this.clearIdleTimer();
    if (this.detachKeyboard) {
      this.detachKeyboard();
      this.detachKeyboard = null;
    }
    for (const dispose of this.disposers) dispose();
    this.disposers.length = 0;
  }

  // ---------- 内部 ----------

  /** 暂停播放（保留模式与缓存，继续时恢复；未播放时静默） */
  private pausePlayback(): void {
    if (!this.running) return;
    this.running = false;
    this.pendingAdvance = false;
    this.clearTimer();
    this.onRunningChange?.(false);
  }

  /** 切换沉浸（IPC 成功后翻转，失败 Toast 回滚；连按防重入） */
  private async toggleImmersive(): Promise<void> {
    if (this.immBusy) return;
    this.immBusy = true;
    const next = !this.immersive;
    try {
      await this.host.window.setFullscreen(next);
      this.immersive = next;
      this.host.onImmersiveChange(next);
      if (next) this.hideAll();
      else this.wake();
      console.info(`[ui] immersive mode: ${next ? "on" : "off"}`);
    } catch (err) {
      this.host.notify(`沉浸模式切换失败：${String(err)}`);
    } finally {
      this.immBusy = false;
    }
  }

  /** 退出沉浸（未沉浸时静默） */
  private async exitImmersive(): Promise<void> {
    if (!this.immersive || this.immBusy) return;
    await this.toggleImmersive();
  }

  /** 切换置顶（IPC 成功后翻转，失败 Toast 回滚；连按防重入） */
  private async togglePin(): Promise<void> {
    if (this.pinBusy) return;
    this.pinBusy = true;
    const next = !this.pinned;
    try {
      await this.host.window.setAlwaysOnTop(next);
      this.pinned = next;
      this.host.onPinChange(next);
      console.info(`[ui] pin window: ${next ? "on" : "off"}`);
    } catch (err) {
      this.host.notify(`置顶切换失败：${String(err)}`);
    } finally {
      this.pinBusy = false;
    }
  }

  private showEl(el: HTMLElement): void {
    el.classList.remove("hidden");
    el.setAttribute("aria-hidden", "false");
  }

  private hideEl(el: HTMLElement): void {
    el.classList.add("hidden");
    el.setAttribute("aria-hidden", "true");
  }

  private isHidden(el: HTMLElement): boolean {
    return el.classList.contains("hidden");
  }

  private groupEls(mode: PresenceMode): HTMLElement[] {
    return mode === "slideshow"
      ? [this.bars.slideshowBar]
      : [this.bars.infoBar, this.bars.toolbar, this.bars.frameBar];
  }

  private isGroupVisible(mode: PresenceMode): boolean {
    return this.groupEls(mode).some((el) => !this.isHidden(el));
  }

  private showGroup(mode: PresenceMode): void {
    for (const el of this.groupEls(mode)) {
      if (el === this.bars.frameBar && !this.frameAvailable) {
        this.hideEl(el);
        continue;
      }
      this.showEl(el);
    }
  }

  private hideGroup(mode: PresenceMode): void {
    for (const el of this.groupEls(mode)) this.hideEl(el);
  }

  /** 悬停抑制：enter 取消闲置计时，leave 重启计时（语义沿用旧 bindHover） */
  private bindHover(element: HTMLElement): void {
    const onEnter = () => {
      if (this.isHidden(element)) return;
      this.hoveringOverlay = true;
      this.clearIdleTimer();
    };
    const onLeave = () => {
      this.hoveringOverlay = false;
      if (!this.isHidden(element)) {
        this.wake();
      }
    };
    element.addEventListener("mouseenter", onEnter);
    element.addEventListener("mouseleave", onLeave);
    this.disposers.push(() => {
      element.removeEventListener("mouseenter", onEnter);
      element.removeEventListener("mouseleave", onLeave);
    });
  }

  /** 清除闲置隐藏计时 */
  private clearIdleTimer(): void {
    if (this.idleTimer !== undefined) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = undefined;
    }
  }

  /** 排定闲置隐藏：悬停或弹窗打开时不启动计时；到点隐藏当前分组 */
  private scheduleIdleHide(ms: number = this.idleMs): void {
    this.clearIdleTimer();
    if (this.hoveringOverlay || this.dialogOpen) return;
    const mode = this.mode;
    this.idleTimer = window.setTimeout(() => {
      this.hideGroup(mode);
    }, ms);
  }

  private clearTimer(): void {
    if (this.timerId !== null) {
      window.clearTimeout(this.timerId);
      this.timerId = null;
    }
  }

  /** 按下一拍原定时刻排定；timer 独立运行，不被加载阻塞 */
  private schedule(): void {
    this.clearTimer();
    if (!this.running) return;
    const remaining = Math.max(0, this.nextTickAt - performance.now());
    this.timerId = window.setTimeout(() => {
      this.timerId = null;
      this.tick();
      if (this.running) this.schedule();
    }, remaining);
  }

  /** 每拍触发：拍点固定推进；上一跳未完成则记欠拍，完成后立即补跳 */
  private tick(): void {
    if (!this.running) return;
    this.nextTickAt += this.intervalMs;
    if (this.advancing) {
      this.pendingAdvance = true;
      return;
    }
    this.pendingAdvance = false;
    this.advancing = true;
    void this.advance().finally(() => {
      this.advancing = false;
      if (this.pendingAdvance) {
        this.pendingAdvance = false;
        if (this.running) {
          // 欠拍补跳：由完成回调驱动，非 0ms 定时器轮询
          this.nextTickAt = performance.now();
          this.schedule();
        }
      }
    });
  }

  private async advance(): Promise<void> {
    if (!this.running) return;
    if (!this.onAdvance) {
      this.pausePlayback();
      return;
    }
    let keepGoing = true;
    try {
      keepGoing = await this.onAdvance();
    } catch {
      // 单跳失败不杀死播放：继续下一跳（容错）
      keepGoing = true;
    }
    if (!keepGoing) {
      // 撞到全局最后一张：调用面已负责 Toast 与退出模式，此处只停拍
      this.pausePlayback();
    }
  }
}
