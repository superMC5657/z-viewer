/**
 * 专业版授权服务模块
 * 激活码验证、注销、在线购买跳转与解锁对话框交互（支持 Light Dismiss 点击背景关闭）
 */

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { LicenseInfo, StoreInfo } from "../types";
import type { UI } from "../ui";

export interface LicenseCallbacks {
  onUnlocked: () => void;
  onDowngraded: () => void;
  onReloadRequested: () => void;
}

export class LicenseService {
  private pro = false;
  private deactivateConfirmArmed = false;
  private deactivateConfirmTimer: number | undefined;

  constructor(
    private ui: UI,
    private callbacks: LicenseCallbacks
  ) {}

  get isPro(): boolean {
    return this.pro;
  }

  setPro(val: boolean): void {
    this.pro = val;
    this.ui.setLocked(!val);
  }

  /** 同步在线续验或启动拉取的授权状态 */
  applyStatus(info: LicenseInfo): void {
    const wasPro = this.pro;
    this.pro = info.status === "pro";
    this.ui.setLocked(!this.pro);

    if (!wasPro || this.pro) return;

    // 由 pro 降级时清理状态
    this.hideDialog();
    this.ui.showToast("专业版授权已失效，请重新激活");
    this.callbacks.onDowngraded();
    this.callbacks.onReloadRequested();
  }

  /** 打开激活/管理对话框 */
  async openDialog(): Promise<void> {
    this.resetDeactivateConfirm();
    try {
      const info = await invoke<LicenseInfo>("get_license_status");
      this.ui.showLicenseDialog(info);
    } catch (err) {
      this.ui.showToast(String(err));
    }
  }

  hideDialog(): void {
    this.resetDeactivateConfirm();
    this.ui.hideLicenseDialog();
  }

  /** 重置注销确认状态 */
  private resetDeactivateConfirm(): void {
    this.deactivateConfirmArmed = false;
    window.clearTimeout(this.deactivateConfirmTimer);
    const dialog = document.getElementById("unlock-dialog");
    const btn = document.getElementById("unlock-confirm") as HTMLButtonElement | null;
    if (btn) btn.textContent = dialog?.dataset.mode === "active" ? "注销" : "激活";
  }

  /** 激活专业版 */
  async activate(code: string, email: string): Promise<void> {
    const errEl = document.getElementById("unlock-error")!;
    errEl.classList.add("hidden");
    try {
      const info = await invoke<LicenseInfo>("activate_license", { code, email });
      if (info.status === "pro") {
        this.pro = true;
        this.hideDialog();
        this.ui.setLocked(false);
        this.ui.showToast("专业版解锁成功");
        this.callbacks.onUnlocked();
        this.callbacks.onReloadRequested();
      } else {
        errEl.textContent = "激活失败，请检查激活码";
        errEl.classList.remove("hidden");
      }
    } catch (err) {
      errEl.textContent = String(err);
      errEl.classList.remove("hidden");
    }
  }

  /** 取消激活（注销） */
  async deactivate(): Promise<void> {
    const errEl = document.getElementById("unlock-error")!;
    errEl.classList.add("hidden");
    try {
      const info = await invoke<LicenseInfo>("deactivate_license");
      this.pro = info.status === "pro";
      this.ui.setLocked(!this.pro);
      this.hideDialog();
      this.ui.showToast("已取消激活");
      this.callbacks.onReloadRequested();
    } catch (err) {
      this.resetDeactivateConfirm();
      errEl.textContent = String(err);
      errEl.classList.remove("hidden");
    }
  }

  /** 打开官网购买页 */
  async openStore(): Promise<void> {
    const errEl = document.getElementById("unlock-error")!;
    errEl.classList.add("hidden");
    try {
      const info = await invoke<StoreInfo>("get_store_info");
      if (!info.buyUrl) {
        errEl.textContent = "在线购买地址尚未配置，请联系开发者";
        errEl.classList.remove("hidden");
        return;
      }
      await openUrl(info.buyUrl);
    } catch (err) {
      errEl.textContent = String(err);
      errEl.classList.remove("hidden");
    }
  }

  /** 绑定解锁弹窗所有事件（含遮罩点击 Light-Dismiss） */
  bindDialogEvents(): void {
    const dialogMask = document.getElementById("unlock-dialog")!;
    const cancelBtn = document.getElementById("unlock-cancel")!;
    const confirmBtn = document.getElementById("unlock-confirm")! as HTMLButtonElement;
    const buyBtn = document.getElementById("unlock-buy")!;
    const codeInput = document.getElementById("unlock-code") as HTMLInputElement;
    const emailInput = document.getElementById("unlock-email") as HTMLInputElement;

    // 点击遮罩空白区域（Light-Dismiss）自动关闭弹窗
    dialogMask.addEventListener("click", (e) => {
      if (e.target === dialogMask) {
        this.hideDialog();
      }
    });

    cancelBtn.addEventListener("click", () => this.hideDialog());

    confirmBtn.addEventListener("click", () => {
      if (dialogMask.dataset.mode === "active") {
        if (!this.deactivateConfirmArmed) {
          this.deactivateConfirmArmed = true;
          confirmBtn.textContent = "确认注销";
          this.ui.showToast("再次点击确认注销");
          window.clearTimeout(this.deactivateConfirmTimer);
          this.deactivateConfirmTimer = window.setTimeout(() => this.resetDeactivateConfirm(), 3000);
          return;
        }
        void this.deactivate();
        return;
      }
      void this.activate(codeInput.value.trim(), emailInput.value.trim());
    });

    buyBtn.addEventListener("click", () => void this.openStore());

    const onEnter = (e: KeyboardEvent) => {
      if (e.key === "Enter") {
        void this.activate(codeInput.value.trim(), emailInput.value.trim());
      }
    };
    codeInput.addEventListener("keydown", onEnter);
    emailInput.addEventListener("keydown", onEnter);
  }
}
