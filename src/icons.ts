/**
 * 线性风格 SVG 图标（Lucide 风格，stroke 统一 1.8）
 * 依据《UI设计草图.md》第 6 节「图标建议使用线性风格 SVG 图标库」
 */

/** 生成单个 SVG 图标字符串 */
export function svgIcon(d: string, size = 18): string {
  return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
}

export const ICONS: Record<string, string> = {
  // —— 文件夹跳转 ——
  "skip-back": svgIcon('<path d="M19 20 9 12l10-8v16Z"/><path d="M5 19V5"/>'),
  "chevron-left": svgIcon('<path d="m15 18-6-6 6-6"/>'),
  "chevron-right": svgIcon('<path d="m9 18 6-6-6-6"/>'),
  "skip-forward": svgIcon('<path d="M5 4l10 8-10 8V4Z"/><path d="M19 5v14"/>'),

  // —— 旋转 ——
  "rotate-cw": svgIcon(
    '<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>'
  ),
  "rotate-ccw": svgIcon(
    '<path d="M3 12a9 9 0 1 0 9-9c-2.52 0-4.93 1-6.74 2.74L3 8"/><path d="M3 3v5h5"/>'
  ),

  // —— 翻转（虚轴 + 对称箭头） ——
  "flip-h": svgIcon(
    '<path d="M12 3v18" stroke-dasharray="2 3"/><path d="M12 8l-4.5 4 4.5 4"/><path d="M12 8l4.5 4-4.5 4"/>'
  ),
  "flip-v": svgIcon(
    '<path d="M3 12h18" stroke-dasharray="2 3"/><path d="M8 12l4-4.5 4 4.5"/><path d="M8 12l4 4.5 4-4.5"/>'
  ),

  // —— 缩放模式（▣ 四角框 = 适应窗口） ——
  "zoom-fit": svgIcon(
    '<path d="M22 6V2h-4"/><path d="M6 22H2v-4"/><path d="M2 2h4"/><path d="M22 22h-4"/>'
  ),

  // —— 幻灯片 ——
  play: svgIcon('<path d="M6 4.5v15l13-7.5Z"/>'),
  pause: svgIcon('<path d="M8 5v14" stroke-width="2.2"/><path d="M16 5v14" stroke-width="2.2"/>'),
  /** 退出幻灯片模式，返回图片浏览 */
  exit: svgIcon('<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="m16 17 5-5-5-5"/><path d="M21 12H9"/>'),

  // —— 窗口 ——
  pin: svgIcon(
    '<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1z"/>'
  ),
  settings: svgIcon(
    '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z"/>'
  ),
  cache: svgIcon(
    '<path d="M4 7c0-1.5 3.6-3 8-3s8 1.5 8 3-3.6 3-8 3-8-1.5-8-3Z"/><path d="M4 7v5c0 1.5 3.6 3 8 3s8-1.5 8-3V7"/><path d="M4 12v5c0 1.5 3.6 3 8 3s8-1.5 8-3v-5"/>'
  ),
  "maximize-2": svgIcon(
    '<path d="M15 3h6v6"/><path d="M9 21H3v-6"/><path d="M21 3l-7 7"/><path d="M3 21l7-7"/>'
  ),

  // —— 更新（下载箭头） ——
  update: svgIcon(
    '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>'
  ),

  // —— 标题栏 ——
  minus: svgIcon('<path d="M5 12h14" stroke-width="1.6"/>', 12),
  square: svgIcon('<rect x="6.5" y="6.5" width="11" height="11" rx="1"/>', 12),
  close: svgIcon('<path d="m7 7 10 10M17 7 7 17" stroke-width="1.8"/>', 12),
};

/** 工具栏按钮定义（布局按草图 3.2） */
export interface ToolbarButton {
  id: string;
  icon?: string;
  text?: string;
  tip: string;
  enabled: boolean; // false = 后续阶段功能，点击提示
}

export interface ToolbarGroup {
  id: string;
  items: ToolbarButton[];
}

export const TOOLBAR_GROUPS: ToolbarGroup[] = [
  {
    id: "folder",
    items: [
      // 从左到右：上一个文件夹 / 上一张图片 / 下一张图片 / 下一个文件夹
      // 双箭头 = 文件夹跳转（与资源管理器一致），单箭头 = 图片翻页
      { id: "folder-prev", icon: "skip-back", tip: "上一个文件夹 · PgUp", enabled: true },
      { id: "image-prev", icon: "chevron-left", tip: "上一张图片 · ←", enabled: true },
      { id: "image-next", icon: "chevron-right", tip: "下一张图片 · →", enabled: true },
      { id: "folder-next", icon: "skip-forward", tip: "下一个文件夹 · PgDn", enabled: true },
    ],
  },
  {
    id: "rotate",
    items: [
      { id: "rotate-ccw", icon: "rotate-ccw", tip: "左旋 90° · Shift+R", enabled: true },
      { id: "rotate-cw", icon: "rotate-cw", tip: "右旋 90° · R", enabled: true },
    ],
  },
  {
    id: "flip",
    items: [
      { id: "flip-h", icon: "flip-h", tip: "水平翻转 · H", enabled: true },
      { id: "flip-v", icon: "flip-v", tip: "垂直翻转 · V", enabled: true },
    ],
  },
  {
    id: "zoom",
    items: [
      { id: "zoom-actual", text: "1:1", tip: "实际大小 · 1", enabled: true },
      { id: "zoom-fit", icon: "zoom-fit", tip: "适应窗口 · 0", enabled: true },
    ],
  },
  {
    id: "slideshow",
    items: [{ id: "slideshow", icon: "play", tip: "幻灯片播放 · 空格", enabled: true }],
  },
  {
    id: "window",
    items: [
      { id: "pin", icon: "pin", tip: "窗口置顶 · T", enabled: true },
      { id: "fullscreen", icon: "maximize-2", tip: "沉浸模式 · F", enabled: true },
    ],
  },
  {
    id: "settings",
    items: [
      { id: "cache-toggle", icon: "cache", tip: "预取缓存（开启后跨文件夹/翻页更流畅）", enabled: true },
      { id: "update", icon: "update", tip: "检查更新", enabled: true },
    ],
  },
];
