/**
 * UI 层：工具栏构建、顶部信息条、边界 Toast、标题栏、浮层闲置自动隐藏
 * 布局与视觉依据《UI设计草图.md》第 2 / 3 节
 */

import { ICONS, TOOLBAR_GROUPS, type ToolbarButton } from "./icons";
import type { BrowseState, LicenseInfo } from "./types";
import { LicenseDialog } from "./components/dialog";
import { formatSize, truncateMiddle, formatPosition, formatFrame } from "./utils/format";

const TOAST_MS = 1500; // 边界 Toast 持续 1.5s
const FILENAME_MAX = 26; // 文件名中间截断长度（近似）

export interface ToolbarHandlers {
  onAction: (id: string) => void;
}

export class UI {
  readonly licenseDialog: LicenseDialog;
  private toolbar: HTMLElement;
  private slideshowBar: HTMLElement;
  private toast: HTMLElement;
  private toastText: HTMLElement;
  private tbFile: HTMLElement;
  private emptyState: HTMLElement;
  /** 正常模式位置计数（#info-pos），幻灯片启动时复用 */
  private infoPos: HTMLElement;
  /** 缩放百分比读数（#info-zoom） */
  private infoZoom: HTMLElement;
  /** 幻灯片位置计数（#ss-progress） */
  private ssProgress: HTMLElement;
  /** 帧条元素（构造时缓存：updateFrameCount 每帧播放回调都访问，不走 DOM 查询） */
  private framePlayBtn: HTMLElement;
  private frameCountEl: HTMLElement;

  private toastTimer: number | undefined;

  constructor(bars: { toolbar: HTMLElement; slideshowBar: HTMLElement }) {
    // 浮层容器由 Presence 统一查询并注入（本文件零 overlay-bar 查找）
    this.toolbar = bars.toolbar;
    this.slideshowBar = bars.slideshowBar;
    this.toast = document.getElementById("toast")!;
    this.toastText = document.getElementById("toast-text")!;
    this.tbFile = document.getElementById("tb-file")!;
    this.emptyState = document.getElementById("empty-state")!;
    this.infoPos = document.getElementById("info-pos")!;
    this.infoZoom = document.getElementById("info-zoom")!;
    this.ssProgress = document.getElementById("ss-progress")!;
    this.framePlayBtn = document.getElementById("frame-play")!;
    this.frameCountEl = document.getElementById("frame-count")!;

    this.licenseDialog = new LicenseDialog();
  }

  /** 工具栏按钮 Map 缓存（消除高频 DOM 查询） */
  private toolbarButtons = new Map<string, HTMLElement>();
  /** 幻灯片控制栏全屏按钮缓存 */
  private ssFullscreenBtn: HTMLElement | null = null;

  /** 构建工具栏 DOM（按钮定义见 icons.ts，布局按草图 3.2） */
  buildToolbar(handlers: ToolbarHandlers): void {
    const frag = document.createDocumentFragment();
    for (const group of TOOLBAR_GROUPS) {
      if (frag.childNodes.length > 0) {
        frag.appendChild(sep());
      }
      for (const btn of group.items) {
        const el = this.makeButton(btn, handlers);
        this.toolbarButtons.set(btn.id, el);
        frag.appendChild(el);
      }
    }
    this.toolbar.appendChild(frag);
    this.ssFullscreenBtn = this.slideshowBar.querySelector<HTMLElement>('[data-id="fullscreen"]');
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

  /** 工具栏按钮激活态（缩放模式 / 播放中 / 沉浸显示 accent 色）。
   *  同时作用于普通工具栏与幻灯片控制条（如 fullscreen 按钮在两处共享激活态） */
  setToolbarActive(id: string, active: boolean): void {
    this.toolbarButtons.get(id)?.classList.toggle("active", active);
    if (id === "fullscreen") {
      this.ssFullscreenBtn?.classList.toggle("active", active);
    }
  }

  /** 工具栏按钮高等级态（缓存高等级显示橙色） */
  setToolbarLevel(id: string, high: boolean): void {
    this.toolbarButtons.get(id)?.classList.toggle("level-high", high);
  }

  /** 缩放模式按钮：适应窗口在纯 fit 态激活（accent），否则置灰（仍可点击恢复）；1:1 在 actual 态激活 */
  setZoomButtons(fit: boolean, actual: boolean): void {
    const fitBtn = this.toolbarButtons.get("zoom-fit");
    const actualBtn = this.toolbarButtons.get("zoom-actual");
    fitBtn?.classList.toggle("active", fit);
    fitBtn?.classList.toggle("dimmed", !fit);
    actualBtn?.classList.toggle("active", actual);
    actualBtn?.classList.remove("dimmed");
  }

  /** 标题栏最大化 / 还原状态视觉同步 */
  updateMaximizeButton(isMaximized: boolean): void {
    const btn = document.getElementById("btn-maximize");
    if (!btn) return;
    btn.innerHTML = isMaximized ? ICONS.restore : ICONS.square;
    const tip = isMaximized ? "向下还原" : "最大化";
    btn.title = tip;
    btn.setAttribute("aria-label", tip);
  }

  /** 更新顶部信息条（草图 3.3） */
  updateInfo(state: BrowseState, dims: { w: number; h: number } | null): void {
    setText("info-file", truncateMiddle(state.file_name, FILENAME_MAX));
    setText("info-dims", dims ? `${dims.w}×${dims.h}` : "…");
    setText("info-size", formatSize(state.file_size));
    setText("info-pos", formatPosition(state.global_index + 1, state.global_total, state.loading));
    setText("info-folder-name", truncateMiddle(state.folder_name, 24));
  }

  /** 标题栏当前文件名（草图 3.1） */
  updateTitleFile(name: string): void {
    this.tbFile.textContent = truncateMiddle(name, 40);
  }

  /** 缩放百分比读数（null = 无图片/未加载 → —）。随 onStateChange 同步 */
  setZoom(scale: number | null): void {
    this.infoZoom.textContent = scale == null ? "—" : `${Math.round(scale * 100)}%`;
  }

  /** 空状态显隐 */
  setEmpty(empty: boolean): void {
    this.emptyState.classList.toggle("hidden", !empty);
  }

  // ---------- 帧控制浮条（草图 3.7） ----------

  /** 帧可用性转发（presenter 调用；实际分组策略归 Presence，经回调路由） */
  onFrameAvailability: ((available: boolean) => void) | null = null;

  /** 帧条显隐意图：仅转发可用性，显隐决策与闲置联动归 Presence 分组表 */
  setFrameBarVisible(visible: boolean): void {
    this.onFrameAvailability?.(visible);
  }

  /** 播放/暂停按钮图标与激活态 */
  setFramePlaying(playing: boolean): void {
    this.framePlayBtn.innerHTML = playing ? ICONS.pause : ICONS.play;
    this.framePlayBtn.classList.toggle("active", playing);
  }

  /** 帧计数：帧 12/48 */
  updateFrameCount(index: number, total: number): void {
    this.frameCountEl.textContent = formatFrame(index, total);
  }

  /** 帧计数未知态：原生 <img> 播放中，帧未按需拆帧（"帧 …"） */
  setFrameCountUnknown(): void {
    this.frameCountEl.textContent = formatFrame();
  }

  /** 帧条加载态：按需拆帧中显示"正在加载帧…"，完成后恢复未知态 */
  setFrameLoading(loading: boolean): void {
    this.frameCountEl.textContent = loading ? "正在加载帧…" : formatFrame();
  }

  /** 帧条按钮统一接线（呈现层只传策略回调，按钮 DOM 细节收敛于此） */
  onFrameAction(h: (op: "first" | "prev" | "toggle" | "next" | "last") => void): void {
    const wire = (id: string, icon: string | null, op: "first" | "prev" | "toggle" | "next" | "last"): void => {
      const el = document.getElementById(id);
      if (!el) return;
      if (icon !== null) el.innerHTML = icon;
      el.addEventListener("click", () => h(op));
    };
    wire("frame-first", ICONS["skip-back"], "first");
    wire("frame-prev", ICONS["chevron-left"], "prev");
    wire("frame-play", null, "toggle");
    wire("frame-next", ICONS["chevron-right"], "next");
    wire("frame-last", ICONS["skip-forward"], "last");
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
    this.ssProgress.textContent = formatPosition(index, total);
  }

  /** 幻灯片启动：直接复用正常模式信息条的当前位置（含加载中 `3/…` 形态） */
  syncSlideshowProgressFromInfo(): void {
    this.ssProgress.textContent = this.infoPos.textContent;
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

  /** 沉浸 body 样式（浮层显隐与唤醒归 Presence，见沉浸切换扇出） */
  setImmersive(immersive: boolean): void {
    document.body.classList.toggle("immersive", immersive);
  }

  // ---------- 专业版解锁对话框 ----------

  /** 专业版解锁对话框当前是否处于显示状态 */
  isLicenseDialogOpen(): boolean {
    return this.licenseDialog.isOpen;
  }

  /** 激活/管理对话框显隐（内容渲染与显隐归 LicenseDialog，本处只做薄转发） */
  showLicenseDialog(info: LicenseInfo): void {
    this.licenseDialog.renderLicense(info);
  }

  hideLicenseDialog(): void {
    this.licenseDialog.hideModal();
  }

  /** 免费版锁定态：禁用缓存按钮（点击仍触发解锁引导，见 main.ts 拦截）。
   *  跨文件夹浏览（folder-prev/folder-next）自 ADR 0003 起全版本可用，不再锁定。 */
  setLocked(locked: boolean): void {
    for (const id of ["cache-toggle"]) {
      const btn = this.toolbarButtons.get(id);
      if (!btn) continue;
      if (locked) {
        if (!btn.dataset.origTip) btn.dataset.origTip = btn.dataset.tip;
        btn.dataset.tip = "专业版功能 · 点击解锁";
      } else {
        btn.dataset.tip = btn.dataset.origTip ?? btn.dataset.tip;
      }
      btn.classList.toggle("locked", locked);
    }
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

export { formatSize, truncateMiddle, formatPosition, formatFrame } from "./utils/format";
