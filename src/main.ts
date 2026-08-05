/**
 * 入口与状态机：IPC 浏览、拖拽打开、浮层唤醒、窗口控制
 * 依据《UI设计草图.md》与《需求报告与技术方案.md》
 */

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { Viewer, type FitMode } from "./viewer";
import { UI } from "./ui";
import { WindowState } from "./window-state";
import { attachInput } from "./input";
import type { BrowseState, NavResult } from "./types";
import "./ui.css";

const stage = document.getElementById("stage")!;
const img = document.getElementById("image") as HTMLImageElement;
const dropOverlay = document.getElementById("drop-overlay")!;

const viewer = new Viewer(stage, img);
const ui = new UI();
ui.buildToolbar({ onAction: handleToolbarAction });
const windowState = new WindowState(getCurrentWindow(), viewer, ui);

// ---------- 状态 ----------
let currentDims: { w: number; h: number } | null = null;

// ---------- 图片显示 ----------

function showImage(state: BrowseState): void {
  ui.setEmpty(false);
  ui.updateTitleFile(state.file_name);
  ui.updateInfo(state, null);
  currentDims = null;
  viewer.load(state.path).then(() => {
    currentDims = { w: img.naturalWidth, h: img.naturalHeight };
    ui.updateInfo(state, currentDims);
  });
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
  if (result.state) showImage(result.state);
}

async function openPath(path: string): Promise<void> {
  try {
    const result = await invoke<NavResult>("open_path", { path });
    if (result.state) showImage(result.state);
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
    case "folder-first":
      void nav(() => invoke("jump_folder", { target: "first" }));
      break;
    case "folder-prev":
      void nav(() => invoke("jump_folder", { target: "prev" }));
      break;
    case "folder-next":
      void nav(() => invoke("jump_folder", { target: "next" }));
      break;
    case "folder-last":
      void nav(() => invoke("jump_folder", { target: "last" }));
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
  ui.setToolbarActive("zoom-fit", true); // 默认适应窗口
  ui.setEmpty(true);

  try {
    // 命令行 / 双击打开：Rust 端已注入浏览模型
    const state = await invoke<BrowseState | null>("get_initial_state");
    if (state) {
      showImage(state);
      ui.setEmpty(false);
    }
  } catch (err) {
    console.error(err);
  }
  // 启动闲置计时：空状态或看图状态下浮层都会按时自动隐藏
  ui.wake();
}

void init();
