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
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

import { Viewer, type FitMode } from "./viewer";
import { feLog } from "./logger";
import { UI } from "./ui";
import { WindowState } from "./window-state";
import { Slideshow } from "./slideshow";
import { PrefetchPool } from "./prefetch";
import { attachInput } from "./input";
import { ICONS } from "./icons";
import type { AppSettings, BrowseState, LicenseInfo, LoadResult, NavResult } from "./types";
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
let cacheLevel = 1;
/** 专业版是否已解锁（启动时查询；控制按钮锁定态与功能入口） */
let pro = false;
/** 缓存等级设置请求代次：并发切换时只应用最后一次（防幻灯片自动切换与手动切换竞态） */
let cacheLevelGen = 0;
/** 幻灯片播放前的缓存等级（退出模式时恢复） */
let slideshowPrevCacheLevel: number | null = null;
/** 是否处于幻灯片模式（true=隐藏普通浮层只显示控制条；暂停不退出，返回按钮才退出） */
let slideshowMode = false;

// ---------- 状态 ----------
let currentDims: { w: number; h: number } | null = null;
/** 最近一次浏览状态（解锁后重开当前图片以启用跨文件夹扫描） */
let lastState: BrowseState | null = null;
/** 当前 RAW 解码的 Blob URL（切换图片时 revoke，防止内存累积） */
let currentBlobUrl: string | null = null;
/** showImage 代次：播放中手动翻页等并发加载时，丢弃过期响应 */
let showSeq = 0;

// ---------- 图片显示 ----------

async function showImage(state: BrowseState): Promise<void> {
  lastState = state;
  const seq = ++showSeq;
  feLog(`显示图片: ${state.file_name} (${state.folder_name}) [${state.global_index + 1}/${state.global_total}]`);
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
    // needsIpc（RAW/动画）邻居已由 Rust prefetch_context（导航后自动触发）
    // 预取进 DecodeCache —— 前端不再重复 load_image，避免并发冗余解码
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
  "pro-required": "跨文件夹浏览是专业版功能",
};

/** 付费功能入口统一拦截：免费版弹出解锁引导（不发起 IPC） */
function requirePro(): boolean {
  if (pro) return true;
  ui.showToast("专业版功能，解锁后可用");
  ui.showUnlockDialog();
  return false;
}

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
    // 专业版功能被锁定（jump_folder 拦截）：引导解锁
    if (result.boundary === "pro-required") ui.showUnlockDialog();
    return;
  }
  if (result.state) {
    await showImage(result.state);
    // 手动翻页不中断幻灯片，但从当前图重置计时
    slideshow.resetTimer();
  }
}

async function openPath(path: string): Promise<void> {
  exitSlideshow(); // 打开新图片：退出幻灯片模式回到浏览（含停止播放）
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
      if (!requirePro()) return;
      void nav(() => invoke("jump_folder", { target: "prev" }));
      break;
    case "image-prev":
      void nav(() => invoke("prev_image"));
      break;
    case "image-next":
      void nav(() => invoke("next_image"));
      break;
    case "folder-next":
      if (!requirePro()) return;
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
    case "cache-toggle":
      if (!requirePro()) return;
      void toggleCache();
      break;
    case "update":
      void checkForUpdate();
      break;
    case "slideshow":
      slideshow.toggle();
      break;
  }
}

// ---------- 更新（tauri-plugin-updater） ----------

/**
 * 检查更新：check → download（带进度 Toast）→ install → relaunch
 * 无新版本直接 Toast 提示；任意环节失败 Toast 报错，不中断浏览
 */
async function checkForUpdate(): Promise<void> {
  ui.showToast("正在检查更新…");
  let update;
  try {
    update = await check();
  } catch (err) {
    ui.showToast(`检查更新失败：${String(err)}`);
    feLog(`检查更新失败: ${String(err)}`);
    return;
  }
  if (!update) {
    ui.showToast("已是最新版本");
    feLog("检查更新: 已是最新版本");
    return;
  }
  feLog(`发现新版本: v${update.currentVersion} → v${update.version}`);
  ui.showToast(`发现新版本 v${update.version}，开始下载…`);
  try {
    let total = 0;
    let downloaded = 0;
    let lastPct = 0;
    await update.download((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        downloaded = 0;
        lastPct = 0;
        feLog(`下载开始: 共 ${total} 字节`);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        // 进度 Toast（每 20% 刷一次，避免刷屏；未知总量时跳过百分比）
        if (total > 0) {
          const pct = Math.floor((downloaded / total) * 100);
          if (pct - lastPct >= 20) {
            lastPct = pct;
            ui.showToast(`正在下载更新 ${pct}%…`);
          }
        }
      } else if (event.event === "Finished") {
        ui.showToast("下载完成，正在安装…");
      }
    });
    await update.install();
  } catch (err) {
    ui.showToast(`更新失败：${String(err)}`);
    feLog(`更新失败: ${String(err)}`);
    return;
  }
  feLog("更新安装完成，重启应用");
  ui.showToast("更新完成，正在重启…");
  try {
    await relaunch();
  } catch (err) {
    ui.showToast(`重启失败，请手动重启应用：${String(err)}`);
    feLog(`重启失败: ${String(err)}`);
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
      result = await Promise.race([
        invoke<NavResult>("next_image"),
        new Promise<null>((res) => setTimeout(() => res(null), 3000)),
      ]);
    } catch (err) {
      feLog(`幻灯片跳转 IPC 失败: ${String(err)}`);
      console.error("幻灯片 next_image 失败:", err);
      return true; // 网络/IPC 异常不停止，下一轮重试
    }
    if (!result) {
      feLog("幻灯片跳转超时（3s 无响应），跳过本跳");
      console.warn("幻灯片 next_image 超时，跳过本跳");
      return true;
    }
    feLog(`幻灯片跳转完成: boundary=${result.boundary ?? "无"} 下一张=${result.state?.file_name ?? "无"}`);
    // 调试探针：记录每次 advance 的返回（CDP 可读）
    (window as unknown as Record<string, unknown>).__lastAdvance = result;
    if (result.boundary === "last-image") {
      // 播放到全局最后一张：自动停止并提醒（需求 2.4），退出幻灯片模式回到浏览
      ui.showToast("已播放完所有图片");
      exitSlideshow();
      return false;
    }
    if (!result.state) {
      console.warn("幻灯片空 state，退出");
      exitSlideshow(); // 空模型（未打开图片）：停止空转并退出
      return false;
    }
    // showImage 超时兜底：5s 未完成按成功处理（避免挂起杀死幻灯片）
    await Promise.race([
      showImage(result.state),
      new Promise<void>((res) => setTimeout(res, 5000)),
    ]);
    return true;
  };

  slideshow.onStateChange = (running) => {
    // 播放/暂停只切换播放按钮图标与工具栏激活态；**不切换模式**
    //（暂停是真正停住，仍停留在幻灯片模式；退出模式由 exitSlideshow 显式完成）
    feLog(`幻灯片 onStateChange: running=${running}, slideshowMode=${slideshowMode}`);
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
        if (pro && cacheLevel !== 2) void setCacheLevel(2);
      }
    }
    // running=false（暂停）：保持模式、保留进度计数、保留高等级缓存——随时可继续播放
  };
}

/// 退出幻灯片模式，返回图片浏览（返回按钮 / 播放完 / 打开新图）
function exitSlideshow(): void {
  if (slideshowMode === false) return; // 幂等
  feLog("exitSlideshow: 退出幻灯片模式");
  slideshow.stop();
  slideshowMode = false;
  ui.setSlideshowMode(false); // 恢复普通浮层（内部 wake）
  ui.setSlideshowProgress(0, 0);
  // 恢复播放前的缓存等级。条件：播放前不是 2（说明自动启用请求可能已发出/在飞，必须 force 打回），
  // 或播放前是 2 但播放中被人为改过（当前播放时工具栏隐藏、无快捷键，仅防御未来扩展）。
  // 两者都不满足（播放前 2 且期间未变）则无需恢复，避免多余 invoke。
  // 免费版跳过：无在飞提级请求（进入时 pro 才提级），force 恢复会被 Rust 拒绝且误弹错误
  if (pro && slideshowPrevCacheLevel !== null && (slideshowPrevCacheLevel !== 2 || cacheLevel !== 2)) {
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
    if (lv > 0) void refreshContext(); // 开启后立即按当前上下文预取
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

// ---------- 专业版解锁 ----------

async function activateLicense(code: string): Promise<void> {
  const errEl = document.getElementById("unlock-error")!;
  errEl.classList.add("hidden");
  try {
    const info = await invoke<LicenseInfo>("activate_license", { code });
    if (info.status === "pro") {
      pro = true;
      ui.hideUnlockDialog();
      ui.setLocked(false);
      ui.showToast("专业版解锁成功");
      // 恢复持久化的缓存等级（Rust 侧已放行）
      const saved = Number(localStorage.getItem("cache-level"));
      if (Number.isFinite(saved) && saved >= 1 && saved <= 2) {
        void setCacheLevel(saved, undefined, true);
      }
      // 重开当前图片：以专业版模式重建浏览模型（启用兄弟文件夹扫描）
      if (lastState) void openPath(lastState.path);
    } else {
      errEl.textContent = "激活失败，请检查激活码";
      errEl.classList.remove("hidden");
    }
  } catch (err) {
    errEl.textContent = String(err);
    errEl.classList.remove("hidden");
  }
}

/** 装配解锁对话框事件（取消 / 激活 / Enter 提交） */
function bindUnlockDialog(): void {
  document.getElementById("unlock-cancel")!.addEventListener("click", () => ui.hideUnlockDialog());
  document.getElementById("unlock-confirm")!.addEventListener("click", () => {
    const code = (document.getElementById("unlock-code") as HTMLInputElement).value;
    void activateLicense(code);
  });
  document.getElementById("unlock-code")!.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const code = (e.target as HTMLInputElement).value;
      void activateLicense(code);
    }
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
  bindUnlockDialog();
  buildFrameBar();
  buildSlideshowBar();
  // 应用持久化的幻灯片间隔（buildSlideshowBar 已同步下拉框显示）
  const savedInterval = loadSsInterval();
  if (savedInterval !== null) slideshow.setInterval(savedInterval);
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
    // 授权状态：免费版锁定付费功能按钮
    const info = await invoke<LicenseInfo>("get_license_status");
    pro = info.status === "pro";
    ui.setLocked(!pro);
  } catch (err) {
    console.error(err);
  }

  try {
    // 命令行 / 双击打开：Rust 端已注入浏览模型
    const state = await invoke<BrowseState | null>("get_initial_state");
    if (state) {
      await showImage(state);
      ui.setEmpty(false);
    }
    // 读取缓存设置并同步 UI（localStorage 持久化优先，否则 Rust 默认）
    const settings = await invoke<AppSettings>("get_settings");
    if (!pro) {
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
  } catch (err) {
    console.error(err);
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

/** base64 字符串 → Blob（RAW JPEG / 动画帧反序列化） */
function base64ToBlob(b64: string, mime: string): Blob {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type: mime });
}

void init();
