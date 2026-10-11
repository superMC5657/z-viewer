/**
 * 专业版授权服务模块（前端权益答案唯一源）
 * 激活码验证、注销、在线购买跳转与解锁对话框交互（支持 Light Dismiss 点击背景关闭）
 * 对话框 DOM 归 LicenseDialog 唯一拥有，本模块只发意图（render/showError/按钮文案），
 * 经 LicenseCallbacks adapter 收结果；本文件零 DOM 直抓。
 * 权益问题（can/require）一律由 LicenseInfo 快照派生，加一条权益只改
 * ENTITLEMENT_FEATURES 映射与 Entitlement 词表，不再四处动手。
 */

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { LicenseInfo, StoreInfo } from "../types";
import type { UI } from "../ui";

export interface LicenseCallbacks {
  onUnlocked: () => void;
  onDowngraded: () => void;
  onReloadRequested: () => void;
  /** 对话框显隐打点（调用面转交 Presence 抑制闲置计时） */
  onDialogVisibility?: (open: boolean) => void;
}

/** 付费权益词表（普通形状，非 module；调用面只传名） */
export type Entitlement = "cross-folder" | "prefetch-cache";

/** 富权益答案（与后端 LicenseInfo 同源快照；seam 携带此答案而非光秃 boolean） */
export interface Entitlements {
  pro: boolean;
  level: string | null;
  features: string[];
}

/** 权益 → 所需 JWT 功能位（当前捆绑为一个 pro；拆分时只改此处） */
const ENTITLEMENT_FEATURES: Record<Entitlement, string> = {
  "cross-folder": "pro",
  "prefetch-cache": "pro",
};

export class LicenseService {
  /** 授权快照（唯一答案源；entitlements/can/require 皆由此派生，不各处设 flag） */
  private lastInfo: LicenseInfo | null = null;
  private deactivateConfirmArmed = false;
  private deactivateConfirmTimer: number | undefined;

  constructor(
    private ui: UI,
    private callbacks: LicenseCallbacks
  ) {
    this.ui.licenseDialog.onClose(() => {
      this.resetDeactivateConfirm();
      this.callbacks.onDialogVisibility?.(false);
    });
  }

  /** 当前富权益答案（快照派生；无快照时全 free） */
  entitlements(): Entitlements {
    const info = this.lastInfo;
    return {
      pro: info?.status === "pro",
      level: info?.level ?? null,
      features: info?.features ?? [],
    };
  }

  /** 是否拥有某权益（pro 状态全授；features 供未来拆分） */
  can(e: Entitlement): boolean {
    const a = this.entitlements();
    return a.pro || a.features.includes(ENTITLEMENT_FEATURES[e]);
  }

  /** 付费权益统一拦截：无权益则 Toast + 弹解锁引导（不发起 IPC）；调用面只传权益名 */
  require(e: Entitlement): boolean {
    if (this.can(e)) return true;
    this.ui.showToast("专业版功能，解锁后可用");
    void this.openDialog();
    return false;
  }

  /** 同步在线续验或启动拉取的授权状态 */
  applyStatus(info: LicenseInfo): void {
    const wasPro = this.entitlements().pro;
    this.lastInfo = info;
    const pro = info.status === "pro";
    this.ui.setLocked(!pro);

    if (!wasPro || pro) return;

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
      this.callbacks.onDialogVisibility?.(true);
    } catch (err) {
      this.ui.showToast(String(err));
    }
  }

  hideDialog(): void {
    this.resetDeactivateConfirm();
    this.ui.hideLicenseDialog();
  }

  /** 重置注销确认状态（按钮文案归 Dialog，本处只管计时器与 armed 标志） */
  private resetDeactivateConfirm(): void {
    this.deactivateConfirmArmed = false;
    window.clearTimeout(this.deactivateConfirmTimer);
    this.ui.licenseDialog.resetConfirmLabel();
  }

  /** 激活专业版 */
  async activate(code: string, email: string): Promise<void> {
    const dialog = this.ui.licenseDialog;
    dialog.clearLicenseError();
    try {
      const info = await invoke<LicenseInfo>("activate_license", { code, email });
      if (info.status === "pro") {
        this.lastInfo = info;
        this.hideDialog();
        this.ui.setLocked(false);
        this.ui.showToast("专业版解锁成功");
        this.callbacks.onUnlocked();
        this.callbacks.onReloadRequested();
      } else {
        dialog.showLicenseError("激活失败，请检查激活码");
      }
    } catch (err) {
      dialog.showLicenseError(String(err));
    }
  }

  /** 取消激活（注销） */
  async deactivate(): Promise<void> {
    const dialog = this.ui.licenseDialog;
    dialog.clearLicenseError();
    try {
      const info = await invoke<LicenseInfo>("deactivate_license");
      this.lastInfo = info;
      this.ui.setLocked(info.status !== "pro");
      this.hideDialog();
      this.ui.showToast("已取消激活");
      this.callbacks.onReloadRequested();
    } catch (err) {
      this.resetDeactivateConfirm();
      dialog.showLicenseError(String(err));
    }
  }

  /** 打开官网购买页 */
  async openStore(): Promise<void> {
    const dialog = this.ui.licenseDialog;
    dialog.clearLicenseError();
    try {
      const info = await invoke<StoreInfo>("get_store_info");
      if (!info.buyUrl) {
        dialog.showLicenseError("在线购买地址尚未配置，请联系开发者");
        return;
      }
      await openUrl(info.buyUrl);
    } catch (err) {
      dialog.showLicenseError(String(err));
    }
  }

  /** 绑定解锁弹窗表单交互事件（零件经 Dialog 缝获取，本处只装配行为） */
  bindDialogEvents(): void {
    const dialog = this.ui.licenseDialog;
    const cancelBtn = dialog.cancelButton;
    const confirmBtn = dialog.confirmButton;
    const buyBtn = dialog.buyButton;

    cancelBtn.addEventListener("click", () => this.hideDialog());

    confirmBtn.addEventListener("click", () => {
      if (dialog.mode === "active") {
        if (!this.deactivateConfirmArmed) {
          this.deactivateConfirmArmed = true;
          dialog.armDeactivateConfirm();
          this.ui.showToast("再次点击确认注销");
          window.clearTimeout(this.deactivateConfirmTimer);
          this.deactivateConfirmTimer = window.setTimeout(() => this.resetDeactivateConfirm(), 3000);
          return;
        }
        void this.deactivate();
        return;
      }
      const { code, email } = dialog.readInputs();
      void this.activate(code, email);
    });

    buyBtn.addEventListener("click", () => void this.openStore());

    const onEnter = (e: KeyboardEvent) => {
      if (e.key === "Enter") {
        const { code, email } = dialog.readInputs();
        void this.activate(code, email);
      }
    };
    dialog.codeInput.addEventListener("keydown", onEnter);
    dialog.emailInput.addEventListener("keydown", onEnter);
  }
}
