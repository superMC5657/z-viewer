/**
 * 入口与状态机：IPC 浏览、加载通道分发、拖拽打开、浮层唤醒、窗口控制
 * 依据《UI设计草图.md》与《需求报告与技术方案.md》
 *
 * 图片加载通道（8.3）：
 * - asset：常见静态格式 → WebView 原生解码（零拷贝）
 * - raw：RAW 解码 JPEG Blob → <img>
 * - animated：动画帧序列 → <canvas> 逐帧控制
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { Viewer, type FitMode } from "./viewer";
import { UI } from "./ui";
import { WindowState } from "./window-state";
import { attachInput } from "./input";
import { ICONS } from "./icons";
import type { BrowseState, LoadResult, NavResult } from "./types";
import "./ui.css";

const stage = document.getElementById("stage")!;
const img = document.getElementById("image") as HTMLImageElement;
const frameCanvas = document.getElementById("frame-canvas") as HTMLCanvasElement;
const dropOverlay = document.getElementById("drop-overlay")!;

const viewer = new Viewer(stage, img, frameCanvas);
const ui = new UI();
ui.buildToolbar({ onAction: handleToolbarAction });
const windowState = new WindowState(getCurrentWindow(), viewer, ui);

// ---------- 状态 ----------
let currentDims: { w: number; h: number } | null = null;
/** 当前 RAW 解码的 Blob URL（切换图片时 revoke，防止内存累积） */
let currentBlobUrl: string | null = null;

// ---------- 图片显示 ----------

async function showImage(state: BrowseState): Promise<void> {
  ui.setEmpty(false);
  ui.updateTitleFile(state.file_name);
  ui.updateInfo(state, null);
  currentDims = null;
  ui.setFrameBarVisible(false);
  // 旧 RAW Blob 不再需要（新图将覆盖显示）
  if (currentBlobUrl) {
    URL.revokeObjectURL(currentBlobUrl);
    currentBlobUrl = null;
  }

  let result: LoadResult;
  try {
    result = await invoke<LoadResult>("load_image", { path: state.path });
  } catch (err) {
    ui.showToast(`无法加载图片：${String(err)}`);
    return;
  }

  try {
    if (result.mode === "animated" && result.frames?.length) {
      // 动画：canvas 逐帧
      ui.setFrameBarVisible(true);
      await viewer.loadAnimation(result.frames);
    } else if (result.mode === "raw" && result.data) {
      // RAW：解码 JPEG Blob
      const url = URL.createObjectURL(base64ToBlob(result.data, "image/jpeg"));
      currentBlobUrl = url;
      await viewer.loadStatic(url);
    } else {
      // 常见格式：asset 协议直读
      await viewer.loadStatic(convertFileSrc(state.path));
    }
  } catch (err) {
    ui.showToast(`无法显示图片：${String(err)}`);
    ui.setFrameBarVisible(false);
    return;
  }

  currentDims = { w: viewer.naturalWidth, h: viewer.naturalHeight };
  ui.updateInfo(state, currentDims);
  ui.setFramePlaying(viewer.isPlaying);
}

/** 边界 Toast 文案（图片级 / 文件夹级，见《UI设计草图.md》第 6 节） */
const BOUNDARY_TEXT: Record<string, string> = {
  "first-image": "已经是第一张了",
  "last-image": "已经是最后一张了",
  "first-folder": "已经是第一个文件夹了",
  "last-folder": "已经是最后一个文件夹了",
};

async function nav(fn: () => Promise<NavResult>): Promise<void> {
  let result: NavResult;
  try {
    result = await fn();
  } catch (err) {
    console.error(err);
    return;
  }
  if (result.boundary) {
    // 撞边界：图片未变化，只弹 Toast，不重载图片（避免闪烁与变换重置）
    ui.showToast(BOUNDARY_TEXT[result.boundary] ?? "已经到边界了");
    return;
  }
  if (result.state) await showImage(result.state);
}

async function openPath(path: string): Promise<void> {
  try {
    const result = await invoke<NavResult>("open_path", { path });
    if (result.state) await showImage(result.state);
  } catch (err) {
    ui.showToast(String(err));
  }
}

// ---------- 缩放模式（同步按钮激活态） ----------

function setFitMode(mode: FitMode): void {
  viewer.setMode(mode);
  ui.setToolbarActive("zoom-actual", viewer.mode === "actual");
  ui.setToolbarActive("zoom-fit", viewer.mode === "fit");
}

// ---------- 工具栏动作 ----------

function handleToolbarAction(id: string): void {
  switch (id) {
    case "folder-prev":
      void nav(() => invoke("jump_folder", { target: "prev" }));
      break;
    case "image-prev":
      void nav(() => invoke("prev_image"));
      break;
    case "image-next":
      void nav(() => invoke("next_image"));
      break;
    case "folder-next":
      void nav(() => invoke("jump_folder", { target: "next" }));
      break;
    case "rotate-cw":
      viewer.rotate(90);
      break;
    case "rotate-ccw":
      viewer.rotate(-90);
      break;
    case "flip-h":
      viewer.flip("h");
      break;
    case "flip-v":
      viewer.flip("v");
      break;
    case "zoom-actual":
      setFitMode("actual");
      break;
    case "zoom-fit":
      setFitMode("fit");
      break;
    case "pin":
      void windowState.togglePin();
      break;
    case "fullscreen":
      void windowState.toggleImmersive();
      break;
    // 幻灯片为 M4 功能，先给出提示
    case "slideshow":
      ui.showToast("该功能将在后续版本提供");
      break;
  }
}

// ---------- 帧控制浮条（草图 3.7） ----------

function buildFrameBar(): void {
  const iconMap: Record<string, string> = {
    "frame-first": ICONS["skip-back"],
    "frame-prev": ICONS["chevron-left"],
    "frame-next": ICONS["chevron-right"],
    "frame-last": ICONS["skip-forward"],
  };
  for (const [id, icon] of Object.entries(iconMap)) {
    document.getElementById(id)!.innerHTML = icon;
  }
  ui.setFramePlaying(false);

  document.getElementById("frame-first")!.addEventListener("click", () => viewer.seekFrame(0));
  document.getElementById("frame-prev")!.addEventListener("click", () => viewer.stepFrame(-1));
  document.getElementById("frame-play")!.addEventListener("click", () => {
    viewer.togglePlay();
    ui.setFramePlaying(viewer.isPlaying);
  });
  document.getElementById("frame-next")!.addEventListener("click", () => viewer.stepFrame(1));
  document.getElementById("frame-last")!.addEventListener("click", () => viewer.seekFrame(Number.MAX_SAFE_INTEGER));

  viewer.onFrameChange = (index, total) => ui.updateFrameCount(index, total);
}

// ---------- 事件装配 ----------

function bindEvents(): void {
  // 标题栏窗口控制
  const appWindow = getCurrentWindow();
  document.getElementById("btn-minimize")!.addEventListener("click", () => void appWindow.minimize());
  document.getElementById("btn-maximize")!.addEventListener("click", () => void appWindow.toggleMaximize());
  document.getElementById("btn-close")!.addEventListener("click", () => void appWindow.close());

  // 快捷键 + 滚轮
  attachInput(viewer, {
    onPrev: () => void nav(() => invoke("prev_image")),
    onNext: () => void nav(() => invoke("next_image")),
    onJumpFolder: (t) => void nav(() => invoke("jump_folder", { target: t })),
    onSetMode: setFitMode,
    onToggleImmersive: () => void windowState.toggleImmersive(),
    onExitImmersive: () => void windowState.exitImmersive(),
    onTogglePin: () => void windowState.togglePin(),
    onWake: () => ui.wake(),
  });

  // 浮层唤醒：鼠标移动 / 快捷键
  window.addEventListener("mousemove", () => ui.wake());

  // 拖拽打开（草图 5.5：拖入时虚线框提示）
  const webview = getCurrentWebview();
  void webview.onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") {
      dropOverlay.classList.add("active");
    } else if (p.type === "leave") {
      dropOverlay.classList.remove("active");
    } else if (p.type === "drop") {
      dropOverlay.classList.remove("active");
      const path = p.paths[0];
      if (path) void openPath(path);
    }
  });

  // 图片平移拖拽
  stage.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    viewer.startPan(e.clientX, e.clientY);
  });
  window.addEventListener("mousemove", (e) => viewer.panTo(e.clientX, e.clientY));
  window.addEventListener("mouseup", () => viewer.endPan());

  // 双击：实际大小 ↔ 适应窗口（草图 5.4）
  stage.addEventListener("dblclick", () => {
    if (!viewer.hasImage) return;
    setFitMode(viewer.mode === "fit" ? "actual" : "fit");
  });

  // 窗口尺寸变化
  window.addEventListener("resize", () => viewer.onResize());
}

// ---------- 启动 ----------

async function init(): Promise<void> {
  bindEvents();
  buildFrameBar();
  ui.setToolbarActive("zoom-fit", true); // 默认适应窗口
  ui.setEmpty(true);
  // 空状态初始隐藏帧条（打开图片后由 showImage 按需设置）
  ui.setFrameBarVisible(false);

  try {
    // 命令行 / 双击打开：Rust 端已注入浏览模型
    const state = await invoke<BrowseState | null>("get_initial_state");
    if (state) {
      await showImage(state);
      ui.setEmpty(false);
    }
  } catch (err) {
    console.error(err);
  }
  // 启动闲置计时：空状态或看图状态下浮层都会按时自动隐藏
  ui.wake();
}

// ---------- 工具 ----------

/** base64 字符串 → Blob（RAW JPEG / 动画帧反序列化） */
function base64ToBlob(b64: string, mime: string): Blob {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type: mime });
}

void init();
