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
import { Presence, loadSlideshowInterval, raceWithTimeout, type InputIntent, type NavigateKind } from "./presence";
import { ICONS } from "./icons";
import { setAnimExts, setRawExts } from "./types";
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
const appWindow = getCurrentWindow();
// Presence 先行：查询浮层容器并注入 UI；host 闭包延迟求值（presenter/license 后建亦可）
const presence: Presence = new Presence(
  {
    applyView: (intent, anchor) => {
      switch (intent) {
        case "mode-fit":
          presenter.setFitMode("fit");
          break;
        case "mode-actual":
          presenter.setFitMode("actual", anchor?.x, anchor?.y);
          break;
        case "zoom-in":
          viewer.zoomByCenter(1.2);
          break;
        case "zoom-out":
          viewer.zoomByCenter(1 / 1.2);
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
        default:
          break;
      }
    },
    navigate: (kind: NavigateKind) => {
      if (kind === "prev") {
        void presenter.navigate(() => invoke("prev_image"));
        return;
      }
      if (kind === "next") {
        void presenter.navigate(() => invoke("next_image"));
        return;
      }
      // 跨文件夹浏览自 ADR 0003 起全版本可用，此处不再做权益拦截
      const target = kind === "folder-first" ? "first" : kind === "folder-last" ? "last" : kind === "folder-prev" ? "prev" : "next";
      void presenter.navigate(() => invoke("jump_folder", { target }));
    },
    notify: (msg) => ui.showToast(msg),
    onSlideshowMode: (active) => {
      if (active) ui.syncSlideshowProgressFromInfo();
    },
    onSlideshowExit: () => {
      ui.setSlideshowProgress(0, 0);
    },
    getCacheLevel: () => cacheLevel,
    onCacheElevation: (active, elevatedFrom) => {
      if (active) {
        // 自动提级到高等级缓存（专业版功能）；免费版跳过（Rust 会拒绝，避免误弹错误）
        if (licenseService.can("prefetch-cache") && cacheLevel !== 2) void setCacheLevel(2);
      } else if (
        licenseService.can("prefetch-cache") &&
        elevatedFrom != null &&
        (elevatedFrom !== 2 || cacheLevel !== 2)
      ) {
        // 恢复进入前记录的等级（暂停→继续不覆盖，恢复用最初的等级）
        void setCacheLevel(elevatedFrom, undefined, true);
      }
    },
    onImmersiveChange: (active) => {
      viewer.setImmersive(active);
      ui.setImmersive(active);
      ui.setToolbarActive("fullscreen", active);
    },
    onPinChange: (active) => {
      ui.setToolbarActive("pin", active);
    },
    onDialogClose: () => {
      if (ui.isLicenseDialogOpen()) {
        licenseService.hideDialog();
        return true;
      }
      return false;
    },
    window: {
      setFullscreen: (b) => appWindow.setFullscreen(b),
      setAlwaysOnTop: (b) => appWindow.setAlwaysOnTop(b),
    },
  },
  { canUse: (e) => licenseService.can(e) },
);
const ui: UI = new UI(presence.bars);
ui.onFrameAvailability = (v) => presence.setFrameAvailable(v);
ui.buildToolbar({ onAction: handleToolbarAction });
const presenter: ImagePresenter = new ImagePresenter(viewer, ui, {
  onShown: () => presence.notifyManualNav(),
  onProRequired: () => void licenseService.openDialog(),
});

const LICENSE_STATUS_CHANGED = "license://status-changed";

// ---------- 薄接线：呈现策略归 ImagePresenter，本文件只留 window/input/slideshow/cache/license ----------

async function openPath(path: string): Promise<void> {
  presence.exitSlideshow(); // 打开新图片：退出幻灯片模式回到浏览（含停止播放）
  presenter.clearContext(); // 新上下文，清空旧预解码池
  try {
    const result = await invoke<NavResult>("open_path", { path });
    if (result.state) await presenter.show(result.state);
  } catch (err) {
    ui.showToast(String(err));
  }
}

const licenseService: LicenseService = new LicenseService(ui, {
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
  onDialogVisibility: (open) => presence.setDialogOpen(open),
});

// ---------- 缓存等级（起因聚合：unlock/downgrade/提级恢复的副作用归 license 回调之旁，
// cacheLevelGen 竞态就地可理解——见 setCacheLevel 代次校验） ----------
let cacheLevel = 1;
/** 缓存等级设置请求代次：并发切换时只应用最后一次（防幻灯片自动切换与手动切换竞态） */
let cacheLevelGen = 0;

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

// ---------- 工具栏动作（意图经 Presence.dispatch；缓存/license/更新直连） ----------

/** 工具栏 id → 输入意图（呈现词汇适配，模块只讲 InputIntent；null 走直连分支） */
function toolbarIdToIntent(id: string): InputIntent | null {
  switch (id) {
    case "image-prev":
      return "prev";
    case "image-next":
      return "next";
    case "folder-prev":
      return "folder-prev";
    case "folder-next":
      return "folder-next";
    case "zoom-fit":
      return "mode-fit";
    case "zoom-actual":
      return "mode-actual";
    case "rotate-cw":
      return "rotate-cw";
    case "rotate-ccw":
      return "rotate-ccw";
    case "flip-h":
      return "flip-h";
    case "flip-v":
      return "flip-v";
    case "pin":
      return "pin-toggle";
    case "fullscreen":
      return "immersive-toggle";
    case "slideshow":
      return "slideshow-toggle";
    default:
      return null;
  }
}

// ---------- 工具栏动作（意图经 Presence.dispatch；缓存/license/更新直连） ----------

function handleToolbarAction(id: string): void {
  const intent = toolbarIdToIntent(id);
  if (intent) {
    presence.dispatch(intent);
    return;
  }
  switch (id) {
    case "cache-toggle":
      if (!licenseService.require("prefetch-cache")) return;
      void toggleCache();
      break;
    case "update":
      void checkForUpdate((msg) => ui.showToast(msg));
      break;
    case "license":
      void licenseService.openDialog();
      break;
  }
}

// ---------- 幻灯片播放条接线（草图 3.6；节拍归 Presence，此处只做开始/停止/播放条接线） ----------

function buildSlideshowBar(): void {
  ui.setSlideshowPlaying(false);

  document.getElementById("ss-play")!.addEventListener("click", () => presence.dispatch("slideshow-toggle"));
  document.getElementById("ss-play")!.innerHTML = ICONS.play;
  document.getElementById("ss-exit")!.innerHTML = ICONS.exit;
  document.getElementById("ss-exit")!.addEventListener("click", () => presence.exitSlideshow());
  // 沉浸模式（与普通工具栏 fullscreen 按钮同一功能/激活态）
  document.getElementById("ss-fullscreen")!.innerHTML = ICONS["maximize-2"];
  document.getElementById("ss-fullscreen")!.addEventListener("click", () => presence.dispatch("immersive-toggle"));

  const intervalSel = document.getElementById("ss-interval") as HTMLSelectElement;
  // 间隔初始值与持久化归 Presence；此处只做下拉框显示同步、变更转发与启动应用
  const FIXED_INTERVALS = new Set(["1000", "2000", "5000", "10000"]);
  /** 自定义间隔范围（整秒），与 presence 的最小 1000ms 校验对齐 */
  const CUSTOM_MIN_S = 1;
  const CUSTOM_MAX_S = 3600;

  /** 同步下拉显示：非固定档位写入动态选项（自定义值重启后仍正确回显） */
  function syncIntervalDisplay(ms: number): void {
    const v = String(ms);
    if (!FIXED_INTERVALS.has(v)) {
      let opt = intervalSel.querySelector<HTMLOptionElement>("option[data-dynamic]");
      if (!opt) {
        opt = document.createElement("option");
        opt.dataset.dynamic = "";
        intervalSel.insertBefore(opt, intervalSel.querySelector('option[value="custom"]'));
      }
      opt.value = v;
      opt.textContent = `${ms / 1000}s`;
    }
    intervalSel.value = v;
  }

  /** 应用间隔：转发 Presence（含持久化）并回显到下拉 */
  function applyInterval(ms: number): void {
    presence.setIntervalMs(ms);
    console.info(`[slideshow] interval changed to ${ms}ms`);
    syncIntervalDisplay(ms);
  }

  /** 自定义间隔：下拉原位替换为数字输入，Enter/失焦提交，Esc 取消 */
  function promptCustomInterval(): void {
    const input = document.createElement("input");
    input.type = "number";
    input.min = String(CUSTOM_MIN_S);
    input.max = String(CUSTOM_MAX_S);
    input.step = "1";
    input.value = String(Math.round(loadSlideshowInterval() / 1000));
    let done = false;
    const finish = (commit: boolean): void => {
      if (done) return;
      done = true;
      input.replaceWith(intervalSel);
      const sec = Number(input.value);
      if (commit && Number.isInteger(sec) && sec >= CUSTOM_MIN_S && sec <= CUSTOM_MAX_S) {
        applyInterval(sec * 1000);
      } else {
        // 取消/非法输入：回显当前生效间隔
        syncIntervalDisplay(loadSlideshowInterval());
      }
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation(); // 输入中不让 Presence 键盘意图抢走按键
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
    intervalSel.replaceWith(input);
    input.focus();
    input.select();
  }

  const initialInterval = loadSlideshowInterval();
  syncIntervalDisplay(initialInterval);
  presence.setIntervalMs(initialInterval);
  intervalSel.addEventListener("change", () => {
    if (intervalSel.value === "custom") {
      promptCustomInterval();
      return;
    }
    applyInterval(Number(intervalSel.value));
  });

  // 播放器回调
  presence.onAdvance = async () => {
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
    if (result.boundary === "last-image") {
      // 播放到全局最后一张：自动停止并提醒（需求 2.4），退出幻灯片模式回到浏览
      console.info("[slideshow] finished: reached last image");
      ui.showToast("已播放完所有图片");
      presence.exitSlideshow();
      return false;
    }
    if (!result.state) {
      presence.exitSlideshow(); // 空模型（未打开图片）：停止空转并退出
      return false;
    }
    // 呈现超时兜底：5s 未完成按成功处理（避免挂起杀死幻灯片）
    await raceWithTimeout(presenter.show(result.state), 5000);
    return true;
  };

  presence.onRunningChange = (running) => {
    // 播放/暂停只切换播放按钮图标与工具栏激活态；模式与提级由 Presence 内部保证顺序
    console.info(running ? `[slideshow] started (interval: ${intervalSel.value}ms)` : "[slideshow] paused");
    ui.setSlideshowPlaying(running);
    ui.setToolbarActive("slideshow", running);
    // running=false（暂停）：保持模式、保留进度计数、保留高等级缓存——随时可继续播放
  };
}

// ---------- 事件装配 ----------

/** 滚轮缩放与触控板手势（连续手势保留 viewer 直连） */
function onWheel(e: WheelEvent): void {
  // 1. 位于表单、下拉菜单或弹窗内时不拦截滚轮，允许原生滚动
  const target = e.target as HTMLElement | null;
  if (target && target.closest("#slideshow-bar, #unlock-dialog, select, input, textarea")) {
    return;
  }
  presence.wake();
  if (!viewer.hasImage) return;
  e.preventDefault();

  // 2. 触控板双指捏合手势（Pinch-to-zoom）或 Ctrl + 滚轮
  if (e.ctrlKey) {
    const factor = Math.exp(-e.deltaY * 0.005);
    viewer.zoomAt(e.clientX, e.clientY, factor);
    return;
  }

  // 3. 图片放大超出视口时，双指滑动平移画布（支持水平 deltaX 与 Shift+垂直）
  if (viewer.isPannable() && (Math.abs(e.deltaX) > 0 || (e.shiftKey && Math.abs(e.deltaY) > 0))) {
    const dx = e.deltaX !== 0 ? -e.deltaX : e.shiftKey ? -e.deltaY : 0;
    const dy = e.deltaX !== 0 ? -e.deltaY : 0;
    viewer.panDelta(dx, dy);
    return;
  }

  // 4. 常规鼠标滚轮：以鼠标指针为锚点缩放（草图 5.3）
  const factor = Math.exp(-e.deltaY * 0.0016);
  viewer.zoomAt(e.clientX, e.clientY, factor);
}

/** 鼠标侧键翻页意图 */
function onSideButton(e: MouseEvent): void {
  if (e.button === 3) {
    e.preventDefault();
    presence.dispatch("prev");
  } else if (e.button === 4) {
    e.preventDefault();
    presence.dispatch("next");
  }
}

function onAuxClick(e: MouseEvent): void {
  if (e.button === 3 || e.button === 4) e.preventDefault();
}

/** 禁用网页默认右键菜单（防止弹出 Chromium 开发菜单） */
function onContextMenu(e: MouseEvent): void {
  e.preventDefault();
}

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

  // 键盘意图统一经 Presence（repeat 限流/表单守卫/Esc 优先级收敛一处）
  presence.attachKeyboard();

  // 滚轮缩放与触控板手势（连续手势直连 viewer；同步与提全量经统一订阅收敛）
  document.addEventListener("wheel", onWheel, { passive: false });
  // 鼠标侧键翻页（看图软件标配）：XBUTTON1=上一张，XBUTTON2=下一张
  // preventDefault 抑制 WebView2 潜在的前进/后退导航（单页应用无历史，防御性处理）
  document.addEventListener("mousedown", onSideButton);
  document.addEventListener("auxclick", onAuxClick);
  window.addEventListener("contextmenu", onContextMenu);

  // 浮层唤醒 + 图片平移拖拽：合并为单个 mousemove 监听（panTo 无拖拽时零开销）
  window.addEventListener("mousemove", (e) => {
    presence.wake();
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
    presence.dispatch(viewer.isFit ? "mode-actual" : "mode-fit", { x: e.clientX, y: e.clientY });
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
    // 动画扩展名注册：同步自后端 decode::ANIM_EXTS（帧条判定唯一真源）
    setAnimExts(await invoke<string[]>("get_anim_extensions"));
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
    if (!licenseService.can("prefetch-cache")) {
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
  presence.wake();
}

void init();
