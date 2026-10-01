/**
 * 浮层统一管理器
 * 统一调度 info-bar / toolbar / frame-bar / slideshow-bar 等浮层：
 * - 注册制管理
 * - 统一 show / hide (管理 hidden class 与 aria-hidden)
 * - 统一 wake 与闲置淡出 (默认 2s 自动隐藏)
 * - 悬停 (bindHover) 期间抑制闲置隐藏
 */

export interface OverlayConfig {
  id: string;
  element: HTMLElement;
  /** 所属分组，例如 "normal" | "slideshow"，默认 "normal" */
  group?: string;
  /** 动态唤醒条件判定（例如 frame-bar 仅在 frameBarVisible 时唤醒） */
  condition?: () => boolean;
}

export interface OverlayManagerOptions {
  /** 闲置隐藏时间（毫秒），默认 2000 */
  idleMs?: number;
  /** 唤醒高频节流时间（毫秒），默认 150 */
  throttleMs?: number;
}

export class OverlayManager {
  private overlays = new Map<string, OverlayConfig>();
  private activeGroup: string = "normal";
  private idleTimer: number | undefined;
  private lastWakeAt: number = 0;
  private hoveringOverlay: boolean = false;

  readonly idleMs: number;
  readonly throttleMs: number;

  constructor(options?: OverlayManagerOptions) {
    this.idleMs = options?.idleMs ?? 2000;
    this.throttleMs = options?.throttleMs ?? 150;
  }

  /** 注册浮层并自动绑定 hover 监听 */
  register(config: OverlayConfig): void {
    const entry: OverlayConfig = {
      group: "normal",
      ...config,
    };
    this.overlays.set(config.id, entry);
    this.bindHover(config.element);
  }

  /** 获取浮层配置 */
  get(id: string): OverlayConfig | undefined {
    return this.overlays.get(id);
  }

  /** 获取浮层 DOM 元素 */
  getElement(id: string): HTMLElement | undefined {
    return this.overlays.get(id)?.element;
  }

  /** 设置当前激活分组（如 "normal" 或 "slideshow"） */
  setActiveGroup(group: string): void {
    this.activeGroup = group;
  }

  /** 获取当前激活分组 */
  getActiveGroup(): string {
    return this.activeGroup;
  }

  /** 绑定鼠标悬停监听：enter 取消闲置计时，leave 重启计时 */
  bindHover(element: HTMLElement): void {
    element.addEventListener("mouseenter", () => {
      if (this.isHidden(element)) return;
      this.hoveringOverlay = true;
      this.clearIdleTimer();
    });
    element.addEventListener("mouseleave", () => {
      this.hoveringOverlay = false;
      if (!this.isHidden(element)) {
        this.wake();
      }
    });
  }

  /** 显示指定浮层（移除 hidden 类，设置 aria-hidden="false"） */
  show(idOrEl: string | HTMLElement): void {
    const el = typeof idOrEl === "string" ? this.overlays.get(idOrEl)?.element : idOrEl;
    if (!el) return;
    el.classList.remove("hidden");
    el.setAttribute("aria-hidden", "false");
  }

  /** 隐藏指定浮层（添加 hidden 类，设置 aria-hidden="true"） */
  hide(idOrEl: string | HTMLElement): void {
    const el = typeof idOrEl === "string" ? this.overlays.get(idOrEl)?.element : idOrEl;
    if (!el) return;
    el.classList.add("hidden");
    el.setAttribute("aria-hidden", "true");
  }

  /** 浮层当前是否处于可见状态 */
  isVisible(idOrEl: string | HTMLElement): boolean {
    const el = typeof idOrEl === "string" ? this.overlays.get(idOrEl)?.element : idOrEl;
    if (!el) return false;
    return !this.isHidden(el);
  }

  private isHidden(el: HTMLElement): boolean {
    return el.classList.contains("hidden");
  }

  /** 判断指定分组中是否有任意浮层处于可见状态 */
  isGroupVisible(group: string = this.activeGroup): boolean {
    for (const item of this.overlays.values()) {
      if (item.group === group && this.isVisible(item.element)) {
        return true;
      }
    }
    return false;
  }

  /** 立即隐藏所有已注册浮层并清除闲置计时器 */
  hideAll(): void {
    for (const item of this.overlays.values()) {
      this.hide(item.element);
    }
    this.clearIdleTimer();
  }

  /** 唤醒当前分组浮层并排定闲置隐藏 */
  wake(): void {
    const now = performance.now();
    const isVisible = this.isGroupVisible(this.activeGroup);

    if (isVisible && now - this.lastWakeAt < this.throttleMs) {
      return;
    }
    this.lastWakeAt = now;

    if (!isVisible) {
      for (const item of this.overlays.values()) {
        if (item.group === this.activeGroup) {
          if (!item.condition || item.condition()) {
            this.show(item.element);
          } else {
            this.hide(item.element);
          }
        }
      }
    }

    this.scheduleIdleHide();
  }

  /** 清除闲置隐藏计时 */
  clearIdleTimer(): void {
    if (this.idleTimer !== undefined) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = undefined;
    }
  }

  /** 排定闲置隐藏：鼠标悬停在浮层上时不启动计时 */
  scheduleIdleHide(ms: number = this.idleMs): void {
    this.clearIdleTimer();
    if (this.hoveringOverlay) return;
    this.idleTimer = window.setTimeout(() => {
      for (const item of this.overlays.values()) {
        if (item.group === this.activeGroup) {
          this.hide(item.element);
        }
      }
    }, ms);
  }

  /** 销毁并清除计时器 */
  destroy(): void {
    this.clearIdleTimer();
    this.overlays.clear();
  }
}
