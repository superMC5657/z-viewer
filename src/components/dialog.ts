/**
 * 轻量 Modal 对话框基类
 * 统一处理：
 * - 遮罩层显隐状态与 aria-hidden
 * - Light-Dismiss 点击遮罩背景关闭
 * - Esc 键盘拦截与关闭
 * - 展开 / 收起过渡
 * - 焦点设置与关闭后焦点恢复 (Focus Restoration)
 */

export interface DialogOptions {
  /** 遮罩层 DOM 元素或 ID（例如 "unlock-dialog"） */
  mask: HTMLElement | string;
  /** 卡片内容 DOM 元素或 ID（可选） */
  card?: HTMLElement | string;
  /** 点击遮罩层空白区域时自动关闭（Light-Dismiss），默认 true */
  lightDismiss?: boolean;
  /** 按 Esc 键时自动关闭，默认 true */
  escapeDismiss?: boolean;
  /** 默认初始聚焦元素（或返回聚焦元素的函数） */
  initialFocus?: HTMLElement | (() => HTMLElement | null) | null;
  /** 弹窗关闭后的回调 */
  onClose?: () => void;
  /** 弹窗打开后的回调 */
  onOpen?: () => void;
}

export class Dialog {
  readonly mask: HTMLElement;
  readonly card: HTMLElement | null = null;
  private lightDismiss: boolean;
  private escapeDismiss: boolean;
  private initialFocus?: HTMLElement | (() => HTMLElement | null) | null;
  private closeCallbacks = new Set<() => void>();
  private openCallbacks = new Set<() => void>();

  private previousActiveElement: HTMLElement | null = null;
  private boundKeyDown: (e: KeyboardEvent) => void;

  /** 当前活动的 Modal 弹窗（用于顶层单例调度） */
  private static activeModal: Dialog | null = null;

  /** 获取当前处于打开状态的 Modal 弹窗 */
  static get currentModal(): Dialog | null {
    return Dialog.activeModal;
  }

  /** 是否有任意 Modal 处于打开状态 */
  static isAnyModalOpen(): boolean {
    return Boolean(Dialog.activeModal?.isOpen);
  }

  constructor(options: DialogOptions) {
    if (typeof options.mask === "string") {
      const el = document.getElementById(options.mask.replace(/^#/, ""));
      if (!el) throw new Error(`Dialog mask element not found: ${options.mask}`);
      this.mask = el;
    } else {
      this.mask = options.mask;
    }

    if (options.card) {
      if (typeof options.card === "string") {
        this.card = document.getElementById(options.card.replace(/^#/, ""));
      } else {
        this.card = options.card;
      }
    } else {
      this.card = (this.mask.querySelector(".unlock-card, [role='dialog'], .card") ||
        this.mask.firstElementChild) as HTMLElement | null;
    }

    this.lightDismiss = options.lightDismiss ?? true;
    this.escapeDismiss = options.escapeDismiss ?? true;
    this.initialFocus = options.initialFocus;

    if (options.onClose) this.closeCallbacks.add(options.onClose);
    if (options.onOpen) this.openCallbacks.add(options.onOpen);

    this.boundKeyDown = this.handleKeyDown.bind(this);
    this.bindEvents();
  }

  /** 弹窗当前是否处于打开状态 */
  get isOpen(): boolean {
    return !this.mask.classList.contains("hidden");
  }

  /** 注册关闭监听回调 */
  onClose(callback: () => void): () => void {
    this.closeCallbacks.add(callback);
    return () => this.closeCallbacks.delete(callback);
  }

  /** 注册打开监听回调 */
  onOpen(callback: () => void): () => void {
    this.openCallbacks.add(callback);
    return () => this.openCallbacks.delete(callback);
  }

  private bindEvents(): void {
    // Light-Dismiss: 点击背景遮罩关闭
    if (this.lightDismiss) {
      this.mask.addEventListener("click", (e) => {
        if (e.target === this.mask) {
          this.hideModal();
        }
      });
    }
  }

  private handleKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape" && this.escapeDismiss && this.isOpen) {
      e.preventDefault();
      e.stopPropagation();
      this.hideModal();
    }
  }

  /**
   * 打开模态弹窗
   * @param focusTarget 可选显式聚焦元素，未指定则使用 initialFocus 或自动查找
   */
  showModal(focusTarget?: HTMLElement | (() => HTMLElement | null) | null): void {
    if (this.isOpen) return;

    // 1. 记录当前焦点，以便关闭后恢复
    this.previousActiveElement = (document.activeElement as HTMLElement) ?? null;

    // 2. 显示遮罩与状态标记
    this.mask.classList.remove("hidden");
    this.mask.setAttribute("aria-hidden", "false");

    Dialog.activeModal = this;
    window.addEventListener("keydown", this.boundKeyDown, true);

    // 3. 初始焦点调度（等待 DOM 布局就绪）
    const target = focusTarget !== undefined ? focusTarget : this.initialFocus;
    requestAnimationFrame(() => {
      const el = typeof target === "function" ? target() : target;
      if (el && typeof el.focus === "function") {
        el.focus();
      } else {
        const firstFocusable = this.mask.querySelector<HTMLElement>(
          'input:not([type="hidden"]):not([disabled]), button:not([disabled]), [tabindex]:not([tabindex="-1"])'
        );
        firstFocusable?.focus();
      }
    });

    for (const cb of this.openCallbacks) {
      cb();
    }
  }

  /**
   * 关闭模态弹窗并恢复焦点
   */
  hideModal(): void {
    if (!this.isOpen) return;

    // 1. 隐藏遮罩
    this.mask.classList.add("hidden");
    this.mask.setAttribute("aria-hidden", "true");

    if (Dialog.activeModal === this) {
      Dialog.activeModal = null;
    }
    window.removeEventListener("keydown", this.boundKeyDown, true);

    // 2. 焦点恢复
    if (this.previousActiveElement && typeof this.previousActiveElement.focus === "function") {
      try {
        if (document.contains(this.previousActiveElement)) {
          this.previousActiveElement.focus();
        }
      } catch {
        // 忽略非挂载元素的聚焦异常
      }
      this.previousActiveElement = null;
    }

    // 3. 触发关闭监听
    for (const cb of this.closeCallbacks) {
      cb();
    }
  }

  /** 销毁清理 */
  destroy(): void {
    this.hideModal();
    this.closeCallbacks.clear();
    this.openCallbacks.clear();
  }
}
