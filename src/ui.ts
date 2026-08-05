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
  private toast: HTMLElement;
  private toastText: HTMLElement;
  private tbFile: HTMLElement;
  private emptyState: HTMLElement;

  private idleTimer: number | undefined;
  private toastTimer: number | undefined;

  constructor() {
    this.infoBar = document.getElementById("info-bar")!;
    this.toolbar = document.getElementById("toolbar")!;
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
      this.wake();
    }
  }

  /** 立即隐藏所有浮层（进入沉浸模式时调用） */
  hideAll(): void {
    this.infoBar.classList.add("hidden");
    this.toolbar.classList.add("hidden");
    this.infoBar.setAttribute("aria-hidden", "true");
    this.toolbar.setAttribute("aria-hidden", "true");
    window.clearTimeout(this.idleTimer);
  }

  // ---------- 浮层闲置隐藏（设计语言「用完即走」） ----------

  /** 任何鼠标移动 / 按键都会唤醒浮层并重置闲置计时（草图 5.1） */
  wake(): void {
    this.infoBar.classList.remove("hidden");
    this.toolbar.classList.remove("hidden");
    this.infoBar.setAttribute("aria-hidden", "false");
    this.toolbar.setAttribute("aria-hidden", "false");
    window.clearTimeout(this.idleTimer);
    this.idleTimer = window.setTimeout(() => {
      this.infoBar.classList.add("hidden");
      this.toolbar.classList.add("hidden");
      this.infoBar.setAttribute("aria-hidden", "true");
      this.toolbar.setAttribute("aria-hidden", "true");
    }, IDLE_HIDE_MS);
  }
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
