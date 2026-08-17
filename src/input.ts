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
}

export function attachInput(viewer: Viewer, handlers: InputHandlers): () => void {
  const onKeyDown = (e: KeyboardEvent): void => {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;

    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        handlers.onPrev();
        break;
      case "ArrowRight":
        e.preventDefault();
        handlers.onNext();
        break;
      case "PageUp":
        e.preventDefault();
        handlers.onJumpFolder("prev");
        break;
      case "PageDown":
        e.preventDefault();
        handlers.onJumpFolder("next");
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
        handlers.onSetMode("actual");
        break;
      case "0":
        handlers.onSetMode("fit");
        break;
      case "r":
      case "R":
        e.preventDefault();
        viewer.rotate(e.shiftKey ? -90 : 90);
        break;
      case "h":
      case "H":
        viewer.flip("h");
        break;
      case "v":
      case "V":
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
        // 仅退出沉浸模式（草图 3.5）
        e.preventDefault();
        handlers.onExitImmersive();
        break;
      case "T":
      case "t":
        handlers.onTogglePin();
        break;
      case " ":
        // 幻灯片播放/暂停（草图 3.6）
        e.preventDefault();
        handlers.onToggleSlideshow();
        break;
    }
  };

  // 滚轮缩放：以鼠标指针为锚点（草图 5.3）
  const onWheel = (e: WheelEvent): void => {
    handlers.onWake();
    if (!viewer.hasImage) return;
    e.preventDefault();
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

  document.addEventListener("keydown", onKeyDown);
  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("mousedown", onMouseDown);
  document.addEventListener("auxclick", onAuxClick);

  return () => {
    document.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("mousedown", onMouseDown);
    document.removeEventListener("auxclick", onAuxClick);
  };
}
