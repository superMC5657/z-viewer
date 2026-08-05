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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { Viewer, type FitMode } from "./viewer";
import { UI } from "./ui";
import { WindowState } from "./window-state";
import { Slideshow } from "./slideshow";
import { PrefetchPool } from "./prefetch";
import { attachInput } from "./input";
import { ICONS } from "./icons";
import type { AppSettings, BrowseState, LoadResult, NavResult } from "./types";
import { needsIpc } from "./types";
import "./ui.css";

const stage = document.getElementById("stage")!;
const img = document.getElementById("image") as HTMLImageElement;
const frameCanvas = document.getElementById("frame-canvas") as HTMLCanvasElement;
const dropOverlay = document.getElementById("drop-overlay")!;

const viewer = new Viewer(stage, img, frameCanvas);
const ui = new UI();
ui.buildToolbar({ onAction: handleToolbarAction });
const windowState = new WindowState(getCurrentWindow(), viewer, ui);
const slideshow = new Slideshow();
const prefetch = new PrefetchPool();
let cacheStrength = 1;

// ---------- 状态 ----------
let currentDims: { w: number; h: number } | null = null;
/** 当前 RAW 解码的 Blob URL（切换图片时 revoke，防止内存累积） */
let currentBlobUrl: string | null = null;
/** showImage 代次：播放中手动翻页等并发加载时，丢弃过期响应 */
let showSeq = 0;

// ---------- 图片显示 ----------

async function showImage(state: BrowseState): Promise<void> {
  const seq = ++showSeq;
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

  // 预取下一批上下文（方案三/四：asset 预热 + Rust 缓存）
  void refreshContext();

  // 方案二：asset 快速通道 —— 浏览器原生解码格式直接 convertFileSrc，跳过 IPC
  if (!needsIpc(state.path)) {
    try {
      await viewer.loadStatic(convertFileSrc(state.path));
    } catch (err) {
      if (seq === showSeq) {
        ui.showToast(`无法显示图片：${String(err)}`);
      }
      return;
    }
    if (seq !== showSeq) return;
    currentDims = { w: viewer.naturalWidth, h: viewer.naturalHeight };
    ui.updateInfo(state, currentDims);
    ui.setFramePlaying(viewer.isPlaying);
    ui.setSlideshowProgress(state.global_index + 1, state.global_total);
    return;
  }

  let result: LoadResult;
  try {
    result = await invoke<LoadResult>("load_image", { path: state.path });
  } catch (err) {
    if (seq === showSeq) ui.showToast(`无法加载图片：${String(err)}`);
    return;
  }
  if (seq !== showSeq) return; // 已被更新的加载抢占

  try {
    if (result.mode === "animated" && result.frames?.length) {
      // 动画：canvas 逐帧
      ui.setFrameBarVisible(true);
      await viewer.loadAnimation(result.frames);
    } else if (result.mode === "raw" && result.data) {
      // RAW：解码 JPEG Blob
      const url = URL.createObjectURL(base64ToBlob(result.data, "image/jpeg"));
      await viewer.loadStatic(url);
      if (seq !== showSeq) {
        // 已被抢占：立即释放本函数创建的 Blob
        URL.revokeObjectURL(url);
        return;
      }
      currentBlobUrl = url;
    } else {
      // 动画格式但单帧：asset 协议直读
      await viewer.loadStatic(convertFileSrc(state.path));
    }
  } catch (err) {
    if (seq === showSeq) {
      ui.showToast(`无法显示图片：${String(err)}`);
      ui.setFrameBarVisible(false);
    }
    return;
  }
  if (seq !== showSeq) return;

  currentDims = { w: viewer.naturalWidth, h: viewer.naturalHeight };
  ui.updateInfo(state, currentDims);
  ui.setFramePlaying(viewer.isPlaying);
  // 幻灯片进度计数（播放中实时更新）
  ui.setSlideshowProgress(state.global_index + 1, state.global_total);
}

/** 获取当前上下文路径并预热（方案三/四） */
async function refreshContext(): Promise<void> {
  try {
    const paths = await invoke<string[]>("get_context");
    // asset 图：WebView2 预解码池
    prefetch.warm(paths, needsIpc);
    // RAW/动画图：逐个 load_image 走 Rust DecodeCache（等价 Rust 预取，scan-ready 后补跨文件夹邻居）
    for (const p of paths) {
      if (needsIpc(p)) {
        void invoke<LoadResult>("load_image", { path: p }).catch(() => undefined);
      }
    }
  } catch (err) {
    console.error("预取上下文失败", err);
  }
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
  if (result.state) {
    await showImage(result.state);
    // 手动翻页不中断幻灯片，但从当前图重置计时
    slideshow.resetTimer();
  }
}

async function openPath(path: string): Promise<void> {
  slideshow.stop(); // 打开新图片停止幻灯片
  prefetch.clear(); // 新上下文，清空旧预解码池
  try {
    const result = await invoke<NavResult>("open_path", { path });
    if (result.state) await showImage(result.state);
  } catch (err) {
    ui.showToast(String(err));
  }
}

// ---------- 缩放模式（同步按钮激活态） ----------

/** 同步缩放按钮状态：适应窗口仅纯 fit 时激活，手动缩放后置灰 */
function syncZoomButtons(): void {
  ui.setZoomButtons(viewer.isFit, viewer.mode === "actual");
}

function setFitMode(mode: FitMode): void {
  viewer.setMode(mode);
  syncZoomButtons();
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
    case "settings":
      ui.setSettingsBarVisible(!ui.settingsVisible);
      break;
    case "slideshow":
      slideshow.toggle();
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

// ---------- 幻灯片（草图 3.6） ----------

function buildSlideshowBar(): void {
  ui.setSlideshowPlaying(false);

  document.getElementById("ss-play")!.addEventListener("click", () => slideshow.toggle());
  document.getElementById("ss-interval")!.addEventListener("change", (e) => {
    slideshow.setInterval(Number((e.target as HTMLSelectElement).value));
  });

  // 播放器回调
  slideshow.onAdvance = async () => {
    let result: NavResult;
    try {
      result = await invoke<NavResult>("next_image");
    } catch (err) {
      console.error(err);
      return true; // 网络/IPC 异常不停止，下一轮重试
    }
    if (result.boundary === "last-image") {
      // 播放到全局最后一张：自动停止并提醒（需求 2.4）
      ui.showToast("已播放完所有图片");
      return false;
    }
    if (!result.state) return false; // 空模型（未打开图片）：停止空转
    await showImage(result.state);
    return true;
  };

  slideshow.onStateChange = (running) => {
    ui.setSlideshowMode(running);
    ui.setSlideshowPlaying(running);
    ui.setToolbarActive("slideshow", running);
    if (!running) {
      // 停止后恢复浮层与帧条联动
      ui.setSlideshowProgress(0, 0);
    }
  };
}

// ---------- 设置（缓存强度） ----------

function buildSettingsBar(): void {
  const sel = document.getElementById("cache-strength") as HTMLSelectElement;
  sel.value = String(cacheStrength);
  sel.addEventListener("change", () => {
    const v = Number(sel.value);
    cacheStrength = v;
    try {
      localStorage.setItem("cache-strength", String(v));
    } catch {
      /* 忽略持久化失败 */
    }
    void invoke<AppSettings>("set_cache_strength", { strength: v })
      .then(() => {
        // 强度变化后立即按新窗口预取
        void refreshContext();
        ui.setSettingsBarVisible(false); // 选择完收起
      })
      .catch((err) => ui.showToast(`设置失败：${String(err)}`));
  });
}

// ---------- 事件装配 ----------

function bindEvents(): void {
  // 变换状态变化时同步缩放按钮（滚轮/键盘缩放、模式切换、图片加载后）
  viewer.onStateChange = syncZoomButtons;

  // 标题栏窗口控制
  const appWindow = getCurrentWindow();
  // 注入窗口控制图标（HTML 中按钮为空，图标在此填充）
  document.getElementById("btn-minimize")!.innerHTML = ICONS.minus;
  document.getElementById("btn-maximize")!.innerHTML = ICONS.square;
  document.getElementById("btn-close")!.innerHTML = ICONS.close;
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
    onToggleSlideshow: () => slideshow.toggle(),
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
  buildSlideshowBar();
  buildSettingsBar();
  ui.setEmpty(true);
  // 空状态初始隐藏帧条（打开图片后由 showImage 按需设置）
  ui.setFrameBarVisible(false);
  syncZoomButtons(); // 初始：适应窗口激活

  // 后台枚举完成：刷新信息条计数（图片不重载）
  await listen<BrowseState>("browse://scan-ready", (event) => {
    const st = event.payload;
    ui.updateInfo(st, currentDims);
    ui.setSlideshowProgress(st.global_index + 1, st.global_total);
    // 兄弟文件夹枚举完成：重新按强度预取（此前跨文件夹路径取不到）
    void refreshContext();
  });

  try {
    // 命令行 / 双击打开：Rust 端已注入浏览模型
    const state = await invoke<BrowseState | null>("get_initial_state");
    if (state) {
      await showImage(state);
      ui.setEmpty(false);
    }
    // 读取缓存强度设置并同步 UI（localStorage 持久化优先，否则 Rust 默认）
    const settings = await invoke<AppSettings>("get_settings");
    const saved = Number(localStorage.getItem("cache-strength"));
    const strength = Number.isFinite(saved) && saved >= 0 && saved <= 10 ? saved : settings.cache_strength;
    cacheStrength = strength;
    (document.getElementById("cache-strength") as HTMLSelectElement).value = String(strength);
    if (strength !== settings.cache_strength) {
      await invoke<AppSettings>("set_cache_strength", { strength }).catch(() => undefined);
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
