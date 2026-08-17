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
import { openUrl } from "@tauri-apps/plugin-opener";

import { Viewer, type FitMode } from "./viewer";
import { feLog } from "./logger";
import { UI } from "./ui";
import { WindowState } from "./window-state";
import { Slideshow } from "./slideshow";
import { PrefetchPool } from "./prefetch";
import { attachInput } from "./input";
import { cycleBackground, restoreBackground } from "./background";
import { ICONS } from "./icons";
import { needsIpc, setRawExts, parseLoadEnvelope } from "./types";
import type { AppSettings, BrowseState, LicenseInfo, LoadEnvelope, NavResult, StoreInfo } from "./types";
import "./ui.css";

const stage = document.getElementById("stage")!;
const img = document.getElementById("image") as HTMLImageElement;
const frameCanvas = document.getElementById("frame-canvas") as HTMLCanvasElement;
const ghostCanvas = document.getElementById("ghost-canvas") as HTMLCanvasElement;
const dropOverlay = document.getElementById("drop-overlay")!;

const viewer = new Viewer(stage, img, frameCanvas, ghostCanvas);
const ui = new UI();
ui.buildToolbar({ onAction: handleToolbarAction });
const windowState = new WindowState(getCurrentWindow(), viewer, ui);
const slideshow = new Slideshow();
const prefetch = new PrefetchPool();
const LICENSE_STATUS_CHANGED = "license://status-changed";
let cacheLevel = 1;
/** 专业版是否已解锁（启动时查询；控制按钮锁定态与功能入口） */
let pro = false;
/** 注销按钮是否处于“再次点击确认”状态 */
let deactivateConfirmArmed = false;
let deactivateConfirmTimer: number | undefined;
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
/** 当前图片是否为原生 <img> 播放中的动画（帧未加载，帧条交互时按需拆帧） */
let currentAnimCandidate = false;

/** 潜在动画格式（原生 <img> 播放；仅这些格式需要 check_animation 判定） */
const ANIM_EXTS = new Set(["gif", "png", "webp"]);

// ---------- 图片显示 ----------

async function showImage(state: BrowseState): Promise<void> {
  lastState = state;
  const seq = ++showSeq;
  feLog(`显示图片: ${state.file_name} (${state.folder_name}) [${state.global_index + 1}/${state.global_total}]`);
  ui.setEmpty(false);
  ui.updateTitleFile(state.file_name);
  ui.updateInfo(state, null);
  currentDims = null;
  currentAnimCandidate = false;
  ui.setFrameBarVisible(false);
  ui.setFramePlaying(false);
  // 旧 RAW Blob 不再需要（新图将覆盖显示）
  if (currentBlobUrl) {
    URL.revokeObjectURL(currentBlobUrl);
    currentBlobUrl = null;
  }

  // 预取下一批上下文（方案三/四：asset 预热 + Rust 缓存）
  void refreshContext();

  // asset 快速通道：浏览器原生解码直接 convertFileSrc，跳过 IPC
  // （GIF/APNG/WebP 原生 <img> 播放；仅动画候选格式额外判定是否多帧）
  if (!needsIpc(state.path)) {
    const ext = state.path.split(".").pop()?.toLowerCase() ?? "";
    const animCheck = ANIM_EXTS.has(ext)
      ? invoke<boolean>("check_animation", { path: state.path }).catch(() => false)
      : Promise.resolve(false);
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
    // 动画文件：原生播放中，亮出帧条（计数未知，交互时按需拆帧）
    if (await animCheck) {
      if (seq !== showSeq) return; // await 期间被抢占
      currentAnimCandidate = true;
      ui.setFrameBarVisible(true);
      ui.setFrameCountUnknown();
      ui.setFramePlaying(true);
    }
    ui.setSlideshowProgress(state.global_index + 1, state.global_total);
    // 埋点：asset 通道显示成功（浏览器原生解码不经 load_image，必须单独计数）
    void invoke("record_view", { path: state.path });
    return;
  }

  // IPC 通道：RAW/TIFF 由 Rust 解码（二进制信封，payload 为 JPEG 字节）
  ui.beginDecoding();
  try {
    let header: LoadEnvelope;
    let payload: Uint8Array;
    try {
      const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: false });
      ({ header, payload } = parseLoadEnvelope(buf));
    } catch (err) {
      if (seq === showSeq) ui.showToast(`无法加载图片：${String(err)}`);
      return;
    }
    if (seq !== showSeq) return;

    try {
      if (header.mode === "animated" && header.frameSizes.length) {
        // 逐帧模式（帧控制按需触发）
        ui.setFrameBarVisible(true);
        const blobs = splitFrames(payload, header.frameSizes, header.mime ?? "image/png");
        await viewer.loadAnimation(blobs, header.frameDelays);
      } else if (header.mode === "raw") {
        // RAW：先显示内嵌预览（毫秒级），全量解码后台替换
        const url = URL.createObjectURL(new Blob([payload], { type: header.mime ?? "image/jpeg" }));
        try {
          await viewer.loadStatic(url);
        } catch (err) {
          URL.revokeObjectURL(url); // 加载失败也要释放，防止 Blob 泄漏
          throw err; // 交给外层统一 Toast
        }
        if (seq !== showSeq) {
          // 已被抢占：立即释放本函数创建的 Blob
          URL.revokeObjectURL(url);
          return;
        }
        currentBlobUrl = url;
        // 预览阶段立即显示预览尺寸（全量解码完成后 upgradeRawToFull 再更新）
        if (header.width && header.height) {
          currentDims = { w: header.width, h: header.height };
          ui.updateInfo(state, currentDims);
        }
        if (header.isPreview) {
          await upgradeRawToFull(state, seq);
        }
      } else {
        // 单帧动画降级：asset 协议直读
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
    // 埋点：IPC 通道显示成功（RAW/动画；load_image 命令本身已不再计数）
    void invoke("record_view", { path: state.path });
  } finally {
    ui.endDecoding();
  }
}

/** RAW 内嵌预览已显示：后台拉取全量解码结果并替换（seq 抢占安全；引用计数指示解码中）。
 *  预览 URL 在全量加载成功后才撤销 —— 解码失败时画面保持预览，不破碎。 */
async function upgradeRawToFull(state: BrowseState, seq: number): Promise<void> {
  ui.beginDecoding();
  const previewUrl = currentBlobUrl; // 保留预览引用（成功替换后才撤销）
  try {
    const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: true });
    if (seq !== showSeq) return;
    const { header, payload } = parseLoadEnvelope(buf);
    if (header.isPreview) return; // 防御：仍返回预览（异常路径），保持现有显示
    const url = URL.createObjectURL(new Blob([payload], { type: header.mime ?? "image/jpeg" }));
    try {
      await viewer.loadStatic(url);
    } catch (err) {
      URL.revokeObjectURL(url);
      // 全量 JPEG 异常：恢复预览显示（预览引用仍存活）
      if (previewUrl) await viewer.loadStatic(previewUrl).catch(() => undefined);
      throw err;
    }
    if (seq !== showSeq) {
      URL.revokeObjectURL(url);
      return;
    }
    currentBlobUrl = url;
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    // 信息条尺寸更新为全量尺寸
    currentDims = { w: viewer.naturalWidth, h: viewer.naturalHeight };
    ui.updateInfo(state, currentDims);
  } finally {
    ui.endDecoding();
  }
}

/** 按 payload 切分帧 PNG 字节为独立 Blob（frame_sizes 由 Rust 端给出） */
function splitFrames(payload: Uint8Array, sizes: number[], mime: string): Blob[] {
  const blobs: Blob[] = [];
  let off = 0;
  for (const s of sizes) {
    blobs.push(new Blob([payload.subarray(off, off + s)], { type: mime }));
    off += s;
  }
  return blobs;
}

/** 获取当前上下文路径并预热（方案三/四） */
async function refreshContext(): Promise<void> {
  try {
    const paths = await invoke<string[]>("get_context");
    // asset 图（含动画）：WebView2 预解码池
    prefetch.warm(paths, needsIpc);
    // needsIpc（RAW/TIFF）邻居已由 Rust prefetch_context（导航后自动触发）
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
  void openLicenseDialog();
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
    if (result.boundary === "pro-required") void openLicenseDialog();
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

/** 同步缩放按钮状态与信息条缩放读数：适应窗口仅纯 fit 时激活，手动缩放后置灰 */
function syncZoomButtons(): void {
  ui.setZoomButtons(viewer.isFit, viewer.mode === "actual");
  ui.setZoom(viewer.hasImage ? viewer.currentScale : null);
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
    case "license":
      void openLicenseDialog();
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

/** 帧条按钮统一入口：帧已就绪（canvas 模式）直接控制；
 *  原生 <img> 播放中先按需拆帧，完成后执行动作（拆帧后 loadAnimation 自动播放，
 *  动作如 togglePlay 即暂停、stepFrame 即暂停并步进，语义自然） */
function frameControl(action: () => void): void {
  if (viewer.isAnimation) {
    action();
    ui.setFramePlaying(viewer.isPlaying);
    return;
  }
  if (!currentAnimCandidate) return;
  void loadAnimationOnDemand()
    .then(() => {
      if (viewer.isAnimation) {
        action();
        ui.setFramePlaying(viewer.isPlaying);
      }
    })
    .catch((err) => ui.showToast(String(err)));
}

/** 按需拆帧：用户第一次点帧条时把原生播放切换为 canvas 逐帧模式（命中 DecodeCache 秒出） */
async function loadAnimationOnDemand(): Promise<void> {
  const state = lastState;
  if (!state) return;
  ui.setFrameLoading(true);
  try {
    const buf = await invoke<ArrayBuffer>("load_image", { path: state.path, full: false });
    const { header, payload } = parseLoadEnvelope(buf);
    if (header.mode !== "animated" || header.frameSizes.length === 0) {
      // 单帧文件（如单帧 GIF / 静态 PNG）：收起帧条并提示
      ui.setFrameBarVisible(false);
      currentAnimCandidate = false;
      ui.showToast("该文件不是多帧动画");
      return;
    }
    ui.setFrameBarVisible(true);
    const blobs = splitFrames(payload, header.frameSizes, header.mime ?? "image/png");
    await viewer.loadAnimation(blobs, header.frameDelays);
  } finally {
    ui.setFrameLoading(false);
  }
}

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

  document.getElementById("frame-first")!.addEventListener("click", () => frameControl(() => viewer.seekFrame(0)));
  document.getElementById("frame-prev")!.addEventListener("click", () => frameControl(() => viewer.stepFrame(-1)));
  document.getElementById("frame-play")!.addEventListener("click", () => frameControl(() => viewer.togglePlay()));
  document.getElementById("frame-next")!.addEventListener("click", () => frameControl(() => viewer.stepFrame(1)));
  document.getElementById("frame-last")!.addEventListener("click", () => frameControl(() => viewer.seekFrame(Number.MAX_SAFE_INTEGER)));

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

/** 退出幻灯片模式，返回图片浏览（返回按钮 / 播放完 / 打开新图） */
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

async function activateLicense(code: string, email: string): Promise<void> {
  const errEl = document.getElementById("unlock-error")!;
  errEl.classList.add("hidden");
  try {
    const info = await invoke<LicenseInfo>("activate_license", { code, email });
    if (info.status === "pro") {
      pro = true;
      hideLicenseDialog();
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

/** 同步在线续验返回的授权状态；由 pro 降级时清理缓存 UI 并重建当前浏览模型 */
function applyLicenseStatus(info: LicenseInfo): void {
  const wasPro = pro;
  pro = info.status === "pro";
  ui.setLocked(!pro);
  if (!wasPro || pro) return;
  cacheLevel = 0;
  syncCacheButton();
  try {
    localStorage.setItem("cache-level", "0");
  } catch {
    /* 忽略持久化失败 */
  }
  hideLicenseDialog();
  ui.showToast("专业版授权已失效，请重新激活");
  if (lastState) void openPath(lastState.path);
}

/** 打开激活/管理对话框（每次打开读取最新本地记录） */
async function openLicenseDialog(): Promise<void> {
  resetDeactivateConfirm();
  try {
    const info = await invoke<LicenseInfo>("get_license_status");
    ui.showLicenseDialog(info);
  } catch (err) {
    ui.showToast(String(err));
  }
}

/** 注销激活：先提示释放设备名额，成功后再删除本地记录并停用专业功能 */
async function deactivateLicense(): Promise<void> {
  const errEl = document.getElementById("unlock-error")!;
  errEl.classList.add("hidden");
  try {
    const info = await invoke<LicenseInfo>("deactivate_license");
    pro = info.status === "pro";
    ui.setLocked(!pro);
    hideLicenseDialog();
    ui.showToast("已取消激活");
    // 重开当前图片：以免费版模式重建浏览模型（关闭跨文件夹扫描）
    if (lastState) void openPath(lastState.path);
  } catch (err) {
    resetDeactivateConfirm();
    errEl.textContent = String(err);
    errEl.classList.remove("hidden");
  }
}

function hideLicenseDialog(): void {
  resetDeactivateConfirm();
  ui.hideLicenseDialog();
}

function resetDeactivateConfirm(): void {
  deactivateConfirmArmed = false;
  window.clearTimeout(deactivateConfirmTimer);
  const dialog = document.getElementById("unlock-dialog") as HTMLElement | null;
  const btn = document.getElementById("unlock-confirm") as HTMLButtonElement | null;
  if (btn) btn.textContent = dialog?.dataset.mode === "active" ? "注销" : "激活";
}

/** 打开官网购买页（Rust 返回 buy_url，未配置时给出提示） */
async function openStorePage(): Promise<void> {
  const errEl = document.getElementById("unlock-error")!;
  errEl.classList.add("hidden");
  try {
    const info = await invoke<StoreInfo>("get_store_info");
    if (!info.buyUrl) {
      errEl.textContent = "在线购买地址尚未配置，请联系开发者";
      errEl.classList.remove("hidden");
      return;
    }
    await openUrl(info.buyUrl);
  } catch (err) {
    errEl.textContent = String(err);
    errEl.classList.remove("hidden");
  }
}

/** 装配解锁对话框事件（取消 / 激活 / 注销 / 在线购买 / Enter 提交） */
function bindUnlockDialog(): void {
  document.getElementById("unlock-cancel")!.addEventListener("click", () => hideLicenseDialog());
  document.getElementById("unlock-confirm")!.addEventListener("click", () => {
    const dialog = document.getElementById("unlock-dialog") as HTMLElement;
    const btn = document.getElementById("unlock-confirm") as HTMLButtonElement;
    if (dialog.dataset.mode === "active") {
      if (!deactivateConfirmArmed) {
        deactivateConfirmArmed = true;
        btn.textContent = "确认注销";
        ui.showToast("再次点击确认注销");
        window.clearTimeout(deactivateConfirmTimer);
        deactivateConfirmTimer = window.setTimeout(resetDeactivateConfirm, 3000);
        return;
      }
      void deactivateLicense();
      return;
    }
    const code = (document.getElementById("unlock-code") as HTMLInputElement).value;
    const email = (document.getElementById("unlock-email") as HTMLInputElement).value;
    void activateLicense(code, email);
  });
  document.getElementById("unlock-buy")!.addEventListener("click", () => void openStorePage());
  document.getElementById("unlock-code")!.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const code = (e.target as HTMLInputElement).value;
      const email = (document.getElementById("unlock-email") as HTMLInputElement).value;
      void activateLicense(code, email);
    }
  });
  document.getElementById("unlock-email")!.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const code = (document.getElementById("unlock-code") as HTMLInputElement).value;
      const email = (e.target as HTMLInputElement).value;
      void activateLicense(code, email);
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
    onCycleBackground: () => ui.showToast(`看图背景：${cycleBackground()}`),
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
  // 恢复持久化的看图背景（黑/白/灰/棋盘格）
  restoreBackground();
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
  await listen<LicenseInfo>(LICENSE_STATUS_CHANGED, (event) => {
    applyLicenseStatus(event.payload);
  });

  try {
    // 授权状态：免费版锁定付费功能按钮
    const info = await invoke<LicenseInfo>("get_license_status");
    applyLicenseStatus(info);
  } catch (err) {
    console.error(err);
  }

  try {
    // RAW 扩展名单一真源：后端 decode::RAW_EXTS（失败保留 types.ts 内置兜底集）
    setRawExts(await invoke<string[]>("get_raw_extensions"));
  } catch (err) {
    console.error("拉取 RAW 扩展名失败，使用内置兜底集", err);
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

void init();
