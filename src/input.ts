/**
 * 交互层：快捷键映射（《需求报告与技术方案.md》2.5）与滚轮缩放
 * 方向键翻页 / PgUp PgDn 文件夹 / 空格-幻灯片(M4) / R H V 1 0 / ↑↓ 缩放
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
        handlers.onWake();
        handlers.onPrev();
        break;
      case "ArrowRight":
        e.preventDefault();
        handlers.onWake();
        handlers.onNext();
        break;
      case "PageUp":
        e.preventDefault();
        handlers.onWake();
        handlers.onJumpFolder("prev");
        break;
      case "PageDown":
        e.preventDefault();
        handlers.onWake();
        handlers.onJumpFolder("next");
        break;
      case "ArrowUp":
        e.preventDefault();
        handlers.onWake();
        viewer.zoomByCenter(1.2);
        break;
      case "ArrowDown":
        e.preventDefault();
        handlers.onWake();
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
        handlers.onWake();
        viewer.rotate(e.shiftKey ? -90 : 90);
        break;
      case "h":
      case "H":
        handlers.onWake();
        viewer.flip("h");
        break;
      case "v":
      case "V":
        handlers.onWake();
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

  document.addEventListener("keydown", onKeyDown);
  document.addEventListener("wheel", onWheel, { passive: false });

  return () => {
    document.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("wheel", onWheel);
  };
}
