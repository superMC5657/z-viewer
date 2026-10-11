/**
 * 轻量 Modal 对话框基类
 * 统一处理：
 * - 遮罩层显隐状态与 aria-hidden
 * - Light-Dismiss 点击遮罩背景关闭
 * - Esc 键盘拦截与关闭
 * - 展开 / 收起过渡
 * - 焦点设置与关闭后焦点恢复 (Focus Restoration)
 */

import type { LicenseInfo } from "../types";

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

/**
 * 授权对话框（unlock-dialog 唯一主人）：全部零件 DOM 与显隐归此拥有，
 * LicenseService 只发意图（render/showError/按钮文案），经 LicenseCallbacks 收结果。
 * 零件在构造时一次缓存（querySelector 限定遮罩内）；调用面零 getElementById。
 */
export class LicenseDialog extends Dialog {
  private readonly titleEl: HTMLElement;
  private readonly descEl: HTMLElement;
  private readonly infoBox: HTMLElement;
  private readonly formEl: HTMLElement;
  private readonly buyRow: HTMLElement;
  private readonly confirmBtn: HTMLButtonElement;
  private readonly emailText: HTMLElement;
  private readonly codeText: HTMLElement;
  private readonly codeField: HTMLInputElement;
  private readonly emailField: HTMLInputElement;
  private readonly errEl: HTMLElement;
  private readonly cancelBtn: HTMLElement;
  private readonly buyBtn: HTMLElement;

  constructor(maskId = "unlock-dialog") {
    super({ mask: maskId });
    const req = <T extends HTMLElement = HTMLElement>(sel: string): T => {
      const el = this.mask.querySelector<T>(sel);
      if (!el) throw new Error(`LicenseDialog part not found: ${sel}`);
      return el;
    };
    this.titleEl = req("#unlock-title");
    this.descEl = req("#unlock-desc");
    this.infoBox = req("#unlock-info");
    this.formEl = req("#unlock-form");
    this.buyRow = req("#unlock-buy-row");
    this.confirmBtn = req<HTMLButtonElement>("#unlock-confirm");
    this.emailText = req("#unlock-email-text");
    this.codeText = req("#unlock-code-value");
    this.codeField = req<HTMLInputElement>("#unlock-code");
    this.emailField = req<HTMLInputElement>("#unlock-email");
    this.errEl = req("#unlock-error");
    this.cancelBtn = req("#unlock-cancel");
    this.buyBtn = req("#unlock-buy");
  }

  /** 当前模式（renderLicense 写入：active=管理/注销态，inactive=激活态） */
  get mode(): string {
    return this.mask.dataset.mode ?? "";
  }

  get cancelButton(): HTMLElement {
    return this.cancelBtn;
  }

  get confirmButton(): HTMLButtonElement {
    return this.confirmBtn;
  }

  get buyButton(): HTMLElement {
    return this.buyBtn;
  }

  get codeInput(): HTMLInputElement {
    return this.codeField;
  }

  get emailInput(): HTMLInputElement {
    return this.emailField;
  }

  /** 按授权状态渲染内容并打开（active=管理态，inactive=激活态） */
  renderLicense(info: LicenseInfo): void {
    const hasLocal = info.status === "pro" && Boolean(info.code && info.email);
    this.mask.dataset.mode = hasLocal ? "active" : "inactive";
    this.codeField.value = "";
    this.emailField.value = "";
    this.clearLicenseError();
    this.confirmBtn.textContent = hasLocal ? "注销" : "激活";
    this.confirmBtn.classList.toggle("btn-text-danger", hasLocal);
    this.confirmBtn.classList.toggle("btn-text-accent", !hasLocal);

    if (hasLocal) {
      this.titleEl.textContent = "管理激活";
      this.descEl.textContent = "当前设备已激活专业版，可查看激活信息或注销。";
      this.emailText.textContent = info.email ?? "";
      this.codeText.textContent = info.code ?? "";
      this.infoBox.classList.remove("hidden");
      this.formEl.classList.add("hidden");
      this.buyRow.classList.add("hidden");
    } else {
      this.titleEl.textContent = "解锁专业版";
      this.descEl.innerHTML = "解锁后可使用：<b>跨文件夹无缝浏览</b> 与 <b>预取缓存</b>（翻页更流畅）";
      this.infoBox.classList.add("hidden");
      this.formEl.classList.remove("hidden");
      this.buyRow.classList.remove("hidden");
    }

    this.showModal(hasLocal ? this.confirmBtn : this.emailField);
  }

  /** 读取并 trim 输入（激活提交用；调用面不再直抓输入框） */
  readInputs(): { code: string; email: string } {
    return { code: this.codeField.value.trim(), email: this.emailField.value.trim() };
  }

  /** 错误条：隐藏（每次提交前清错） */
  clearLicenseError(): void {
    this.errEl.classList.add("hidden");
  }

  /** 错误条：显示文案 */
  showLicenseError(msg: string): void {
    this.errEl.textContent = msg;
    this.errEl.classList.remove("hidden");
  }

  /** 确认按钮回到 idle 文案（注销两步确认的复位；随当前 mode） */
  resetConfirmLabel(): void {
    this.confirmBtn.textContent = this.mode === "active" ? "注销" : "激活";
  }

  /** 确认按钮进入注销二次确认态 */
  armDeactivateConfirm(): void {
    this.confirmBtn.textContent = "确认注销";
  }
}
