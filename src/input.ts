/**
 * 交互层：快捷键映射（《需求报告与技术方案.md》2.5）与滚轮缩放
 * 方向键翻页 / PgUp PgDn 文件夹 / 空格-幻灯片(M4) / R H V 1 0 / ↑↓ 缩放
 *
 * 唤醒策略：键盘控制命令（翻页/文件夹跳转/缩放/旋转/翻转）**不触发浮层弹出**
 * —— 专注看图时不因操作打断；浮层只由鼠标移动（main.ts mousemove + 滚轮）与
 * 模式切换命令自身的 UI 逻辑唤醒。
 */

import type { FitMode, Viewer } from "./viewer";

export type FolderJump = "first" | "prev" | "next" | "last";

export interface InputHandlers {
  onPrev: () => void;
  onNext: () => void;
  onJumpFolder: (target: FolderJump) => void;
  onSetMode: (mode: FitMode) => void;
  onToggleImmersive: () => void;
  onExitImmersive: () => void;
  onTogglePin: () => void;
  onToggleSlideshow: () => void;
  onWake: () => void;
  /** 关闭弹窗（若有可见弹窗并成功关闭返回 true） */
  onCloseDialog?: () => boolean;
}

export function attachInput(viewer: Viewer, handlers: InputHandlers): () => void {
  /** 按住方向键的 repeat 限流：keydown 连发只放行每 NAV_REPEAT_MIN_MS 一次，
   *  减少冗余导航 IPC 与 RAW 冗余解码（正确性由 main.ts nav 在飞合并兜底） */
  const NAV_REPEAT_MIN_MS = 120;
  let lastNavAt = -Infinity;
  const navAllowed = (e: KeyboardEvent): boolean => {
    const now = performance.now();
    if (e.repeat && now - lastNavAt < NAV_REPEAT_MIN_MS) return false;
    lastNavAt = now;
    return true;
  };

  const onKeyDown = (e: KeyboardEvent): void => {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;

    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        if (navAllowed(e)) handlers.onPrev();
        break;
      case "ArrowRight":
        e.preventDefault();
        if (navAllowed(e)) handlers.onNext();
        break;
      case "PageUp":
        e.preventDefault();
        handlers.onJumpFolder("prev");
        break;
      case "PageDown":
        e.preventDefault();
        handlers.onJumpFolder("next");
        break;
      case "Home":
        e.preventDefault();
        handlers.onJumpFolder("first");
        break;
      case "End":
        e.preventDefault();
        handlers.onJumpFolder("last");
        break;
      case "ArrowUp":
        e.preventDefault();
        viewer.zoomByCenter(1.2);
        break;
      case "ArrowDown":
        e.preventDefault();
        viewer.zoomByCenter(1 / 1.2);
        break;
      case "1":
        e.preventDefault();
        handlers.onSetMode("actual");
        break;
      case "0":
        e.preventDefault();
        handlers.onSetMode("fit");
        break;
      case "r":
      case "R":
        e.preventDefault();
        viewer.rotate(e.shiftKey ? -90 : 90);
        break;
      case "h":
      case "H":
        e.preventDefault();
        viewer.flip("h");
        break;
      case "v":
      case "V":
        e.preventDefault();
        viewer.flip("v");
        break;
      case "F":
      case "f":
      case "F11":
        // 沉浸模式切换（草图 3.5）；F11 兼容需求文档
        e.preventDefault();
        handlers.onToggleImmersive();
        break;
      case "Escape":
        e.preventDefault();
        // 弹窗打开时优先关闭弹窗，未打开时退出沉浸模式
        if (handlers.onCloseDialog && handlers.onCloseDialog()) {
          return;
        }
        handlers.onExitImmersive();
        break;
      case "T":
      case "t":
        e.preventDefault();
        handlers.onTogglePin();
        break;
      case " ":
        // 幻灯片播放/暂停（草图 3.6）
        e.preventDefault();
        handlers.onToggleSlideshow();
        break;
    }
  };

  // 滚轮缩放与触控板手势
  const onWheel = (e: WheelEvent): void => {
    // 1. 位于表单、下拉菜单或弹窗内时不拦截滚轮，允许原生滚动
    const target = e.target as HTMLElement | null;
    if (target && target.closest("#slideshow-bar, #unlock-dialog, select, input, textarea")) {
      return;
    }
    handlers.onWake();
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
      const dx = e.deltaX !== 0 ? -e.deltaX : (e.shiftKey ? -e.deltaY : 0);
      const dy = e.deltaX !== 0 ? -e.deltaY : 0;
      viewer.panDelta(dx, dy);
      return;
    }

    // 4. 常规鼠标滚轮：以鼠标指针为锚点缩放（草图 5.3）
    const factor = Math.exp(-e.deltaY * 0.0016);
    viewer.zoomAt(e.clientX, e.clientY, factor);
  };

  // 鼠标侧键翻页（看图软件标配）：XBUTTON1=上一张，XBUTTON2=下一张
  // preventDefault 抑制 WebView2 潜在的前进/后退导航（单页应用无历史，防御性处理）
  const onMouseDown = (e: MouseEvent): void => {
    if (e.button === 3) {
      e.preventDefault();
      handlers.onPrev();
    } else if (e.button === 4) {
      e.preventDefault();
      handlers.onNext();
    }
  };
  const onAuxClick = (e: MouseEvent): void => {
    if (e.button === 3 || e.button === 4) e.preventDefault();
  };

  // 禁用网页默认右键菜单（防止弹出 Chromium 开发菜单）
  const onContextMenu = (e: MouseEvent): void => {
    e.preventDefault();
  };

  document.addEventListener("keydown", onKeyDown);
  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("mousedown", onMouseDown);
  document.addEventListener("auxclick", onAuxClick);
  window.addEventListener("contextmenu", onContextMenu);

  return () => {
    document.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("mousedown", onMouseDown);
    document.removeEventListener("auxclick", onAuxClick);
    window.removeEventListener("contextmenu", onContextMenu);
  };
}
