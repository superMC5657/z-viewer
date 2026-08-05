/**
 * UI 层：工具栏构建、顶部信息条、边界 Toast、标题栏、浮层闲置自动隐藏
 * 布局与视觉依据《UI设计草图.md》第 2 / 3 节
 */

import { ICONS, TOOLBAR_GROUPS, type ToolbarButton } from "./icons";
import type { BrowseState } from "./types";

const IDLE_HIDE_MS = 2000; // 闲置 2s 浮层自动隐藏
const TOAST_MS = 1500; // 边界 Toast 持续 1.5s
const FILENAME_MAX = 26; // 文件名中间截断长度（近似）

export interface ToolbarHandlers {
  onAction: (id: string) => void;
}

export class UI {
  private infoBar: HTMLElement;
  private toolbar: HTMLElement;
  private frameBar: HTMLElement;
  private slideshowBar: HTMLElement;
  private toast: HTMLElement;
  private toastText: HTMLElement;
  private tbFile: HTMLElement;
  private emptyState: HTMLElement;

  private idleTimer: number | undefined;
  private toastTimer: number | undefined;

  constructor() {
    this.infoBar = document.getElementById("info-bar")!;
    this.toolbar = document.getElementById("toolbar")!;
    this.frameBar = document.getElementById("frame-bar")!;
    this.slideshowBar = document.getElementById("slideshow-bar")!;
    this.toast = document.getElementById("toast")!;
    this.toastText = document.getElementById("toast-text")!;
    this.tbFile = document.getElementById("tb-file")!;
    this.emptyState = document.getElementById("empty-state")!;
  }

  /** 构建工具栏 DOM（按钮定义见 icons.ts，布局按草图 3.2） */
  buildToolbar(handlers: ToolbarHandlers): void {
    const frag = document.createDocumentFragment();
    for (const group of TOOLBAR_GROUPS) {
      if (frag.childNodes.length > 0) {
        frag.appendChild(sep());
      }
      for (const btn of group.items) {
        frag.appendChild(this.makeButton(btn, handlers));
      }
    }
    this.toolbar.appendChild(frag);
  }

  private makeButton(btn: ToolbarButton, handlers: ToolbarHandlers): HTMLButtonElement {
    const el = document.createElement("button");
    el.className = "tb-btn-icon";
    el.dataset.id = btn.id;
    if (btn.icon) el.innerHTML = ICONS[btn.icon];
    else if (btn.text) el.innerHTML = `<span class="btn-text">${btn.text}</span>`;
    el.dataset.tip = btn.tip;
    if (!btn.enabled) el.classList.add("disabled");
    el.addEventListener("click", () => handlers.onAction(btn.id));
    return el;
  }

  /** 工具栏按钮激活态（缩放模式 / 播放中显示 accent 色） */
  setToolbarActive(id: string, active: boolean): void {
    const btn = this.toolbar.querySelector<HTMLElement>(`[data-id="${id}"]`);
    btn?.classList.toggle("active", active);
  }

  /** 缩放模式按钮：适应窗口在纯 fit 态激活（accent），否则置灰（仍可点击恢复）；1:1 在 actual 态激活 */
  setZoomButtons(fit: boolean, actual: boolean): void {
    const fitBtn = this.toolbar.querySelector<HTMLElement>('[data-id="zoom-fit"]');
    const actualBtn = this.toolbar.querySelector<HTMLElement>('[data-id="zoom-actual"]');
    fitBtn?.classList.toggle("active", fit);
    fitBtn?.classList.toggle("dimmed", !fit);
    actualBtn?.classList.toggle("active", actual);
    actualBtn?.classList.remove("dimmed");
  }

  /** 更新顶部信息条（草图 3.3） */
  updateInfo(state: BrowseState, dims: { w: number; h: number } | null): void {
    setText("info-file", truncateMiddle(state.file_name, FILENAME_MAX));
    setText("info-dims", dims ? `${dims.w}×${dims.h}` : "…");
    setText("info-size", formatSize(state.file_size));
    setText("info-pos", `${state.global_index + 1}/${state.global_total}`);
    setText("info-folder-name", truncateMiddle(state.folder_name, 24));
  }

  /** 标题栏当前文件名（草图 3.1） */
  updateTitleFile(name: string): void {
    this.tbFile.textContent = truncateMiddle(name, 40);
  }

  /** 空状态显隐 */
  setEmpty(empty: boolean): void {
    this.emptyState.classList.toggle("hidden", !empty);
  }

  // ---------- 帧控制浮条（草图 3.7） ----------

  /** 帧条显隐：与普通浮层联动（唤醒一起显示、闲置一起隐藏）；
   *  仅动画图片期间参与联动，静态图片强制隐藏 */
  setFrameBarVisible(visible: boolean): void {
    this.frameBarVisible = visible;
    if (!visible) {
      this.frameBar.classList.add("hidden");
      this.frameBar.setAttribute("aria-hidden", "true");
    } else if (!this.infoBar.classList.contains("hidden")) {
      // 浮层当前可见时同步显示（闲置隐藏期间保持隐藏，等 wake 唤醒）
      this.frameBar.classList.remove("hidden");
      this.frameBar.setAttribute("aria-hidden", "false");
    }
  }

  /** 播放/暂停按钮图标与激活态 */
  setFramePlaying(playing: boolean): void {
    const btn = document.getElementById("frame-play")!;
    btn.innerHTML = playing ? ICONS.pause : ICONS.play;
    btn.classList.toggle("active", playing);
  }

  /** 帧计数：帧 12/48 */
  updateFrameCount(index: number, total: number): void {
    document.getElementById("frame-count")!.textContent = `帧 ${index}/${total}`;
  }

  // ---------- 幻灯片（草图 3.6） ----------

  /** 播放/暂停按钮图标与激活态（幻灯片控制条） */
  setSlideshowPlaying(playing: boolean): void {
    const btn = document.getElementById("ss-play")!;
    btn.innerHTML = playing ? ICONS.pause : ICONS.play;
    btn.classList.toggle("active", playing);
  }

  /** 进度计数：3/128（全局位置） */
  setSlideshowProgress(index: number, total: number): void {
    document.getElementById("ss-progress")!.textContent = `${index}/${total}`;
  }

  /** 幻灯片模式切换：播放时只显示控制浮条（信息条/工具栏/帧条隐藏） */
  setSlideshowMode(on: boolean): void {
    this.slideshowMode = on;
    if (on) {
      this.hideAll();
      this.slideshowBar.classList.remove("hidden");
      this.slideshowBar.setAttribute("aria-hidden", "false");
      this.wake(); // 启动闲置计时
    } else {
      this.slideshowBar.classList.add("hidden");
      this.slideshowBar.setAttribute("aria-hidden", "true");
      this.wake();
    }
  }

  // ---------- 边界 Toast（草图 3.4） ----------

  showToast(text: string): void {
    this.toastText.textContent = text;
    this.toast.classList.remove("dismiss");
    this.toast.classList.add("show");
    window.clearTimeout(this.toastTimer);
    this.toastTimer = window.setTimeout(() => {
      this.toast.classList.remove("show");
      this.toast.classList.add("dismiss"); // 消失带 8px 上移
    }, TOAST_MS);
  }

  /** 沉浸模式切换：body.immersive 驱动 CSS（标题栏/浮层位置），进入时隐藏全部浮层 */
  setImmersive(immersive: boolean): void {
    document.body.classList.toggle("immersive", immersive);
    if (immersive) {
      this.hideAll();
    } else {
      // 退出沉浸：唤醒浮层；帧条随 wake 联动显示
      this.wake();
    }
  }

  /** 立即隐藏所有浮层（进入沉浸模式时调用） */
  hideAll(): void {
    this.infoBar.classList.add("hidden");
    this.toolbar.classList.add("hidden");
    this.frameBar.classList.add("hidden");
    this.slideshowBar.classList.add("hidden");
    this.infoBar.setAttribute("aria-hidden", "true");
    this.toolbar.setAttribute("aria-hidden", "true");
    this.frameBar.setAttribute("aria-hidden", "true");
    this.slideshowBar.setAttribute("aria-hidden", "true");
    window.clearTimeout(this.idleTimer);
  }

  // ---------- 浮层闲置隐藏（设计语言「用完即走」） ----------

  /** 任何鼠标移动 / 按键都会唤醒浮层并重置闲置计时（草图 5.1）
   *  帧条与普通工具栏同步；幻灯片模式下只唤醒控制浮条 */
  wake(): void {
    if (this.slideshowMode) {
      // 播放中：仅控制浮条随鼠标唤醒，信息条/工具栏保持隐藏
      this.slideshowBar.classList.remove("hidden");
      this.slideshowBar.setAttribute("aria-hidden", "false");
      window.clearTimeout(this.idleTimer);
      this.idleTimer = window.setTimeout(() => {
        this.slideshowBar.classList.add("hidden");
        this.slideshowBar.setAttribute("aria-hidden", "true");
      }, IDLE_HIDE_MS);
      return;
    }
    this.infoBar.classList.remove("hidden");
    this.toolbar.classList.remove("hidden");
    if (this.frameBarVisible) this.frameBar.classList.remove("hidden");
    this.infoBar.setAttribute("aria-hidden", "false");
    this.toolbar.setAttribute("aria-hidden", "false");
    if (this.frameBarVisible) this.frameBar.setAttribute("aria-hidden", "false");
    window.clearTimeout(this.idleTimer);
    this.idleTimer = window.setTimeout(() => {
      this.infoBar.classList.add("hidden");
      this.toolbar.classList.add("hidden");
      if (this.frameBarVisible) this.frameBar.classList.add("hidden");
      this.infoBar.setAttribute("aria-hidden", "true");
      this.toolbar.setAttribute("aria-hidden", "true");
      if (this.frameBarVisible) this.frameBar.setAttribute("aria-hidden", "true");
    }, IDLE_HIDE_MS);
  }

  /** 帧条当前是否可见（由 main.ts 随图片类型设置） */
  private frameBarVisible = false;
  /** 幻灯片播放中（wake 只唤醒控制浮条） */
  private slideshowMode = false;
}

function sep(): HTMLElement {
  const el = document.createElement("div");
  el.className = "tb-sep";
  return el;
}

function setText(id: string, text: string): void {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

/** 文件名过长时中间截断：photo_very_long…name.jpg（草图 3.3） */
export function truncateMiddle(name: string, maxLen: number): string {
  if (name.length <= maxLen) return name;
  const keep = Math.floor((maxLen - 1) / 2);
  return name.slice(0, keep) + "…" + name.slice(name.length - keep);
}

/** 文件大小格式化（草图样例：4.2 MB） */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}
