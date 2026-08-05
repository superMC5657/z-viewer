/**
 * 窗口状态控制：沉浸模式（F/F11）+ 窗口置顶（T）
 * 依据《UI设计草图.md》3.5 与《需求报告与技术方案.md》2.6
 *
 * - 沉浸：全屏 + 纯黑 + 隐藏标题栏/浮层；鼠标移动唤出浮层，闲置 2s 淡出
 * - 置顶：窗口置顶切换，工具栏 pin 按钮显示 accent 激活态
 *
 * 状态只在 IPC 成功后翻转，失败回滚并 Toast 提示；
 * 切换期间 in-flight 标志防止连按竞态。
 */

import type { Window } from "@tauri-apps/api/window";
import type { Viewer } from "./viewer";
import type { UI } from "./ui";

export class WindowState {
  private immersive = false;
  private pinned = false;
  private toggling = false;

  constructor(
    private appWindow: Window,
    private viewer: Viewer,
    private ui: UI
  ) {}

  get isImmersive(): boolean {
    return this.immersive;
  }

  get isPinned(): boolean {
    return this.pinned;
  }

  /** 切换沉浸模式（草图 3.5） */
  async toggleImmersive(): Promise<void> {
    if (this.toggling) return;
    this.toggling = true;
    const next = !this.immersive;
    try {
      await this.appWindow.setFullscreen(next);
      this.immersive = next;
      this.ui.setImmersive(next);
      this.ui.setToolbarActive("fullscreen", next);
      this.viewer.setImmersive(next);
    } catch (err) {
      this.ui.showToast(`沉浸模式切换失败：${String(err)}`);
    } finally {
      this.toggling = false;
    }
  }

  /** 退出沉浸模式（Esc / F） */
  async exitImmersive(): Promise<void> {
    if (!this.immersive || this.toggling) return;
    await this.toggleImmersive();
  }

  /** 切换窗口置顶 */
  async togglePin(): Promise<void> {
    const next = !this.pinned;
    try {
      await this.appWindow.setAlwaysOnTop(next);
      this.pinned = next;
      this.ui.setToolbarActive("pin", next);
    } catch (err) {
      this.ui.showToast(`置顶切换失败：${String(err)}`);
    }
  }
}
