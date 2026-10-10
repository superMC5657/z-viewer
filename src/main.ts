/**
 * 入口与状态机：IPC 浏览、加载通道分发、拖拽打开、浮层唤醒、窗口控制
 * 依据《UI设计草图.md》与《需求报告与技术方案.md》
 *
 * 图片加载通道（8.3）：
 * - asset：常见静态格式 → WebView 原生解码（零拷贝）
 * - raw：RAW 解码 JPEG Blob → <img>
 * - animated：动画帧序列 → <canvas> 逐帧控制
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { Viewer } from "./viewer";
import { UI } from "./ui";
import { ImagePresenter } from "./presenter";
import { WindowState } from "./window-state";
import { Slideshow } from "./slideshow";
import { attachInput } from "./input";
import { ICONS } from "./icons";
import { setRawExts } from "./types";
import type { AppSettings, BrowseState, LicenseInfo, NavResult } from "./types";
import { checkForUpdate } from "./services/updater";
import { LicenseService } from "./services/license";
import { initZLog } from "./lib/z-log";
import "./ui.css";

// 统一日志先行：页面加载第一时间挂载 console/全局错误转发（写入统一日志文件），失败不阻塞启动
void initZLog().catch(() => undefined);

const stage = document.getElementById("stage")!;
const imgA = document.getElementById("image-a") as HTMLImageElement;
const imgB = document.getElementById("image-b") as HTMLImageElement;
const frameCanvas = document.getElementById("frame-canvas") as HTMLCanvasElement;
const dropOverlay = document.getElementById("drop-overlay")!;

const viewer = new Viewer(stage, imgA, imgB, frameCanvas);
const ui = new UI();
ui.buildToolbar({ onAction: handleToolbarAction });
const presenter = new ImagePresenter(viewer, ui, {
  onShown: () => slideshow.resetTimer(),
  onProRequired: () => void licenseService.openDialog(),
});
const appWindow = getCurrentWindow();
const windowState = new WindowState(appWindow, viewer, ui);
const slideshow = new Slideshow();

const LICENSE_STATUS_CHANGED = "license://status-changed";
let cacheLevel = 1;
/** 缓存等级设置请求代次：并发切换时只应用最后一次（防幻灯片自动切换与手动切换竞态） */
let cacheLevelGen = 0;
/** 幻灯片播放前的缓存等级（退出模式时恢复） */
let slideshowPrevCacheLevel: number | null = null;
/** 是否处于幻灯片模式（true=隐藏普通浮层只显示控制条；暂停不退出，返回按钮才退出） */
let slideshowMode = false;

// ---------- 薄接线：呈现策略归 ImagePresenter，本文件只留 window/input/slideshow/cache/license ----------

async function openPath(path: string): Promise<void> {
  exitSlideshow(); // 打开新图片：退出幻灯片模式回到浏览（含停止播放）
  presenter.clearContext(); // 新上下文，清空旧预解码池
  try {
    const result = await invoke<NavResult>("open_path", { path });
    if (result.state) await presenter.show(result.state);
  } catch (err) {
    ui.showToast(String(err));
  }
}

const licenseService = new LicenseService(ui, {
  onUnlocked: () => {
    const saved = Number(localStorage.getItem("cache-level"));
    if (Number.isFinite(saved) && saved >= 1 && saved <= 2) {
      void setCacheLevel(saved, undefined, true);
    }
  },
  onDowngraded: () => {
    cacheLevel = 0;
    syncCacheButton();
    try {
      localStorage.setItem("cache-level", "0");
    } catch {
      /* 忽略持久化失败 */
    }
  },
  onReloadRequested: () => {
    const p = presenter.lastPath;
    if (p) void openPath(p);
  },
});
// ---------- 工具栏动作（呈现经 presenter，窗口/幻灯片/缓存/license 直连） ----------

/** 付费功能入口统一拦截：免费版弹出解锁引导（不发起 IPC） */
function requirePro(): boolean {
  if (licenseService.isPro) return true;
  ui.showToast("专业版功能，解锁后可用");
  void licenseService.openDialog();
  return false;
}

// ---------- 工具栏动作 ----------

function handleToolbarAction(id: string): void {
  switch (id) {
    case "folder-prev":
      if (!requirePro()) return;
      void presenter.navigate(() => invoke("jump_folder", { target: "prev" }));
      break;
    case "image-prev":
      void presenter.navigate(() => invoke("prev_image"));
      break;
    case "image-next":
      void presenter.navigate(() => invoke("next_image"));
      break;
    case "folder-next":
      if (!requirePro()) return;
      void presenter.navigate(() => invoke("jump_folder", { target: "next" }));
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
      presenter.setFitMode("actual");
      break;
    case "zoom-fit":
      presenter.setFitMode("fit");
      break;
    case "pin":
      void windowState.togglePin();
      break;
    case "fullscreen":
      void windowState.toggleImmersive();
      break;
    case "cache-toggle":
      if (!requirePro()) return;
      void toggleCache();
      break;
    case "update":
      void checkForUpdate(ui);
      break;
    case "license":
      void licenseService.openDialog();
      break;
    case "slideshow":
      slideshow.toggle();
      break;
  }
}

// ---------- 幻灯片（草图 3.6） ----------

/** Promise 竞速 + 兜底定时器清理：主 Promise 先完成时 clearTimeout
 *  （否则兜底定时器每拍空转到点才释放，幻灯片播放中持续累积） */
function raceWithTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
  let timer: number | undefined;
  const timeout = new Promise<null>((res) => {
    timer = window.setTimeout(() => res(null), ms);
  });
  return Promise.race([p, timeout]).finally(() => window.clearTimeout(timer));
}

function buildSlideshowBar(): void {
  ui.setSlideshowPlaying(false);

  document.getElementById("ss-play")!.addEventListener("click", () => slideshow.toggle());
  document.getElementById("ss-play")!.innerHTML = ICONS.play;
  document.getElementById("ss-exit")!.innerHTML = ICONS.exit;
  document.getElementById("ss-exit")!.addEventListener("click", () => exitSlideshow());
  // 沉浸模式（与普通工具栏 fullscreen 按钮同一功能/激活态）
  document.getElementById("ss-fullscreen")!.innerHTML = ICONS["maximize-2"];
  document.getElementById("ss-fullscreen")!.addEventListener("click", () => void windowState.toggleImmersive());

  const intervalSel = document.getElementById("ss-interval") as HTMLSelectElement;
  // 下拉框初始显示持久化的间隔值（否则 select 显示默认 5s 但实际可能不是）
  const savedInterval = loadSsInterval();
  if (savedInterval !== null) intervalSel.value = String(savedInterval);
  intervalSel.addEventListener("change", () => {
    const ms = Number(intervalSel.value);
    slideshow.setInterval(ms);
    console.info(`[slideshow] interval changed to ${ms}ms`);
    try {
      localStorage.setItem("ss-interval", String(ms));
    } catch {
      /* 忽略持久化失败 */
    }
  });

  // 播放器回调
  slideshow.onAdvance = async () => {
    // IPC 超时兜底：next_image 3s 无响应按失败处理（避免 await 永久挂起杀死幻灯片）
    let result: NavResult | null = null;
    try {
      result = await raceWithTimeout(invoke<NavResult>("next_image"), 3000);
    } catch {
      return true; // 网络/IPC 异常不停止，下一轮重试
    }
    if (!result) {
      return true;
    }
    // 调试探针：记录每次 advance 的返回（CDP 可读）
    (window as unknown as Record<string, unknown>).__lastAdvance = result;
    if (result.boundary === "last-image") {
      // 播放到全局最后一张：自动停止并提醒（需求 2.4），退出幻灯片模式回到浏览
      console.info("[slideshow] finished: reached last image");
      ui.showToast("已播放完所有图片");
      exitSlideshow();
      return false;
    }
    if (!result.state) {
      exitSlideshow(); // 空模型（未打开图片）：停止空转并退出
      return false;
    }
    // 呈现超时兜底：5s 未完成按成功处理（避免挂起杀死幻灯片）
    await raceWithTimeout(presenter.show(result.state), 5000);
    return true;
  };

  slideshow.onStateChange = (running) => {
    // 播放/暂停只切换播放按钮图标与工具栏激活态；**不切换模式**
    //（暂停是真正停住，仍停留在幻灯片模式；退出模式由 exitSlideshow 显式完成）
    console.info(running ? `[slideshow] started (interval: ${intervalSel.value}ms)` : "[slideshow] paused");
    ui.setSlideshowPlaying(running);
    ui.setToolbarActive("slideshow", running);
    if (running) {
      // 首次进入：切换为幻灯片模式（隐藏普通浮层，仅显示控制条；
      // 模式切换由 UI 内部同步当前位置计数到 ss-progress，此处无需重复）
      if (slideshowMode === false) {
        slideshowMode = true;
        ui.setSlideshowMode(true);
        // 幻灯片模式自动启用高等级缓存（2=前1后3）：2s 短间隔下预取须提前 ≥2 拍
        // 才能命中（解码耗时常见 2.5s+ > 2s 间隔，蓝色前1后1 提前 1 拍总 miss）
        // 只在首次进入时记录播放前等级（暂停→继续不覆盖，恢复仍用最初的等级）
        slideshowPrevCacheLevel = cacheLevel;
        // 自动提级到高等级缓存（专业版功能）；免费版跳过（Rust 会拒绝，避免误弹错误）
        if (licenseService.isPro && cacheLevel !== 2) void setCacheLevel(2);
      }
    }
    // running=false（暂停）：保持模式、保留进度计数、保留高等级缓存——随时可继续播放
  };
}

/** 退出幻灯片模式，返回图片浏览（返回按钮 / 播放完 / 打开新图） */
function exitSlideshow(): void {
  if (slideshowMode === false) return; // 幂等
  console.info("[slideshow] exited");
  slideshow.stop();
  slideshowMode = false;
  ui.setSlideshowMode(false); // 恢复普通浮层（内部 wake）
  ui.setSlideshowProgress(0, 0);
  // 恢复播放前的缓存等级。
  if (licenseService.isPro && slideshowPrevCacheLevel !== null && (slideshowPrevCacheLevel !== 2 || cacheLevel !== 2)) {
    void setCacheLevel(slideshowPrevCacheLevel, undefined, true);
  }
  slideshowPrevCacheLevel = null;
}

// ---------- 缓存开关（三态：关/开/高） ----------

/**
 * 设置缓存等级：0(关·白) 1(开·蓝) 2(高·橙)
 * - level 与当前相同时跳过（force 除外：幻灯片停止时的恢复必须强制，覆盖在飞请求）
 * - 代次校验：invoke 返回时若期间又有新设置请求，本次结果作废（避免旧请求覆盖新状态）
 */
async function setCacheLevel(level: number, toast?: string, force = false): Promise<void> {
  const lv = Math.min(2, Math.max(0, level));
  if (!force && lv === cacheLevel) return;
  const gen = ++cacheLevelGen;
  try {
    await invoke<AppSettings>("set_cache_level", { level: lv });
    if (gen !== cacheLevelGen) return; // 已有更新的设置请求：本次结果作废
    cacheLevel = lv;
    try {
      localStorage.setItem("cache-level", String(lv));
    } catch {
      /* 忽略持久化失败 */
    }
    syncCacheButton();
    if (toast) ui.showToast(toast);
    if (lv > 0) presenter.refreshContext(); // 开启后立即按当前上下文预热
  } catch (err) {
    ui.showToast(`切换失败：${String(err)}`);
  }
}

/** 切换缓存等级：0(关·白) → 1(开·蓝) → 2(高·橙) → 0 循环 */
async function toggleCache(): Promise<void> {
  const next = (cacheLevel + 1) % 3;
  const toasts = ["预取缓存已关闭", "预取缓存已开启", "高等级预取：前 1 后 3"];
  await setCacheLevel(next, toasts[next]);
}

/** 同步缓存按钮视觉：0=白 1=蓝(active) 2=橙(level-2) */
function syncCacheButton(): void {
  ui.setToolbarActive("cache-toggle", cacheLevel >= 1);
  ui.setToolbarLevel("cache-toggle", cacheLevel >= 2);
}

// ---------- 事件装配 ----------

function bindEvents(): void {
  // 标题栏窗口控制与最大化/还原状态同步
  const syncMaximize = async () => {
    try {
      const isMax = await appWindow.isMaximized();
      ui.updateMaximizeButton(isMax);
    } catch {
      /* 忽略失败 */
    }
  };

  // 标题栏窗口控制（appWindow 为模块级单例，见文件顶部）
  // 注入窗口控制图标（HTML 中按钮为空，图标在此填充）
  document.getElementById("btn-minimize")!.innerHTML = ICONS.minus;
  document.getElementById("btn-maximize")!.innerHTML = ICONS.square;
  document.getElementById("btn-close")!.innerHTML = ICONS.close;
  document.getElementById("btn-minimize")!.addEventListener("click", () => void appWindow.minimize());
  document.getElementById("btn-maximize")!.addEventListener("click", () => {
    void appWindow.toggleMaximize().then(() => syncMaximize());
  });
  document.getElementById("btn-close")!.addEventListener("click", () => void appWindow.close());

  // 双击自绘标题栏空白区域切换最大化/还原（Windows 标配）
  const titlebar = document.getElementById("titlebar");
  titlebar?.addEventListener("dblclick", (e) => {
    if ((e.target as HTMLElement).closest(".tb-controls")) return;
    void appWindow.toggleMaximize().then(() => syncMaximize());
  });

  // 快捷键 + 滚轮
  attachInput(viewer, {
    onPrev: () => void presenter.navigate(() => invoke("prev_image")),
    onNext: () => void presenter.navigate(() => invoke("next_image")),
    onJumpFolder: (t) => void presenter.navigate(() => invoke("jump_folder", { target: t })),
    onSetMode: (mode) => presenter.setFitMode(mode),
    onToggleImmersive: () => void windowState.toggleImmersive(),
    onExitImmersive: () => void windowState.exitImmersive(),
    onTogglePin: () => void windowState.togglePin(),
    onToggleSlideshow: () => slideshow.toggle(),
    onWake: () => ui.wake(),
    onCloseDialog: () => {
      if (ui.isLicenseDialogOpen()) {
        licenseService.hideDialog();
        return true;
      }
      return false;
    },
  });

  // 浮层唤醒 + 图片平移拖拽：合并为单个 mousemove 监听（panTo 无拖拽时零开销）
  window.addEventListener("mousemove", (e) => {
    ui.wake();
    viewer.panTo(e.clientX, e.clientY);
  });
  window.addEventListener("mouseup", () => viewer.endPan());

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

  // 图片平移拖拽（mousemove/mouseup 已在上方合并监听中装配）
  stage.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    viewer.startPan(e.clientX, e.clientY);
  });

  // 双击：以点击位置为锚点在实际大小 ↔ 适应窗口之间平滑切换（草图 5.4 体验升级）
  stage.addEventListener("dblclick", (e) => {
    if (!viewer.hasImage) return;
    presenter.setFitMode(viewer.isFit ? "actual" : "fit", e.clientX, e.clientY);
  });

  // 窗口尺寸变化：rAF 合并并同步最大化按钮状态
  let resizeRaf = 0;
  window.addEventListener("resize", () => {
    cancelAnimationFrame(resizeRaf);
    resizeRaf = requestAnimationFrame(() => {
      viewer.onResize();
      void syncMaximize();
    });
  });

  // 初始同步窗口最大化状态
  void syncMaximize();
}

// ---------- 启动 ----------

async function init(): Promise<void> {
  bindEvents();
  licenseService.bindDialogEvents();
  buildSlideshowBar();
  // 应用持久化的幻灯片间隔（buildSlideshowBar 已同步下拉框显示）
  const savedInterval = loadSsInterval();
  if (savedInterval !== null) slideshow.setInterval(savedInterval);
  ui.setEmpty(true);
  // 空状态初始隐藏帧条（打开图片后由呈现层按需设置）
  ui.setFrameBarVisible(false);

  await listen<LicenseInfo>(LICENSE_STATUS_CHANGED, (event) => {
    licenseService.applyStatus(event.payload);
  });

  try {
    // 授权状态：免费版锁定付费功能按钮
    const info = await invoke<LicenseInfo>("get_license_status");
    licenseService.applyStatus(info);
  } catch {
  }

  try {
    // RAW 扩展名注册：同步自后端 decode::RAW_EXTS
    setRawExts(await invoke<string[]>("get_raw_extensions"));
  } catch {
  }

  try {
    // 命令行 / 双击打开：Rust 端已注入浏览模型
    const state = await invoke<BrowseState | null>("get_initial_state");
    if (state) {
      await presenter.show(state);
      ui.setEmpty(false);
    }
    // 读取缓存设置并同步 UI（localStorage 持久化优先，否则 Rust 默认）
    const settings = await invoke<AppSettings>("get_settings");
    if (!licenseService.isPro) {
      // 免费版：缓存被锁定为关闭，忽略 localStorage 里的付费等级（避免显示橙色高等级态）
      cacheLevel = 0;
    } else {
      const saved = Number(localStorage.getItem("cache-level"));
      cacheLevel = Number.isFinite(saved) && saved >= 0 && saved <= 2 ? saved : settings.cache_level;
    }
    syncCacheButton();
    if (cacheLevel !== settings.cache_level) {
      await invoke<AppSettings>("set_cache_level", { level: cacheLevel }).catch(() => undefined);
    }
  } catch {
  }
  // 启动闲置计时：空状态或看图状态下浮层都会按时自动隐藏
  ui.wake();
}

// ---------- 工具 ----------

/** 读取持久化的幻灯片间隔（ms），无/非法返回 null（用默认 5s） */
function loadSsInterval(): number | null {
  try {
    const v = Number(localStorage.getItem("ss-interval"));
    return Number.isFinite(v) && v >= 1000 ? v : null;
  } catch {
    return null;
  }
}

void init();
