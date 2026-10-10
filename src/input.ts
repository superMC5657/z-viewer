/**
 * 交互词汇：键盘按键 → 输入意图的纯映射（《需求报告与技术方案.md》2.5）
 * 方向键翻页 / PgUp PgDn Home End 文件夹 / 空格-幻灯片 / R H V 1 0 / ↑↓ 缩放 / F 沉浸 / T 置顶 / Esc 退出
 *
 * 本模块零 Viewer 依赖：连续手势（滚轮缩放/平移/拖拽）与所有像素调用留在调用面，
 * 离散意图经 Presence.dispatch 统一决策（重复限流/INPUT 守卫/Esc 优先级归 Presence）。
 */

import type { InputIntent } from "./presence";

/** 键盘事件 → 输入意图；非快捷键返回 null（调用面忽略，不 preventDefault） */
export function keyToIntent(e: Pick<KeyboardEvent, "key" | "shiftKey">): InputIntent | null {
  switch (e.key) {
    case "ArrowLeft":
      return "prev";
    case "ArrowRight":
      return "next";
    case "PageUp":
      return "folder-prev";
    case "PageDown":
      return "folder-next";
    case "Home":
      return "folder-first";
    case "End":
      return "folder-last";
    case "ArrowUp":
      return "zoom-in";
    case "ArrowDown":
      return "zoom-out";
    case "1":
      return "mode-actual";
    case "0":
      return "mode-fit";
    case "r":
    case "R":
      return e.shiftKey ? "rotate-ccw" : "rotate-cw";
    case "h":
    case "H":
      return "flip-h";
    case "v":
    case "V":
      return "flip-v";
    case "F":
    case "f":
    case "F11":
      // 沉浸模式切换（草图 3.5）；F11 兼容需求文档
      return "immersive-toggle";
    case "Escape":
      // 弹窗优先（Presence 经 onDialogClose 判定）、否则退出沉浸
      return "immersive-exit";
    case "T":
    case "t":
      return "pin-toggle";
    case " ":
      // 幻灯片播放/暂停（草图 3.6）
      return "slideshow-toggle";
    default:
      return null;
  }
}
