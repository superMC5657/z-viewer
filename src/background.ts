/**
 * 看图背景（B 键循环）：黑 → 白 → 灰 → 棋盘格
 *
 * 动机：看图区默认纯黑，深色透明素材（logo/图标 PNG）在黑底上几乎不可见。
 * 提供背景感知能力：白色看浅色素材、棋盘格判断透明区域，选择持久化到
 * localStorage（"bg-style"），启动时恢复。样式类定义见 ui.css `#stage.bg-*`。
 */

const STORAGE_KEY = "bg-style";

interface BgStyleDef {
  id: string;
  label: string;
}

/** 循环顺序即数组顺序；black 为默认（无样式类，走 #stage 默认纯黑） */
const BG_STYLES: BgStyleDef[] = [
  { id: "black", label: "黑色" },
  { id: "white", label: "白色" },
  { id: "gray", label: "灰色" },
  { id: "checker", label: "棋盘格" },
];

/** 当前背景 id（模块内单一真源，restore 时初始化） */
let current = "black";

function apply(style: BgStyleDef): void {
  current = style.id;
  const stage = document.getElementById("stage");
  if (stage) {
    for (const s of BG_STYLES) stage.classList.toggle(`bg-${s.id}`, s.id === style.id);
  }
  try {
    localStorage.setItem(STORAGE_KEY, style.id);
  } catch {
    /* 忽略持久化失败（隐私模式等） */
  }
}

/** 启动恢复持久化的背景选择（无/非法记录回到默认黑） */
export function restoreBackground(): void {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(STORAGE_KEY);
  } catch {
    /* 忽略读取失败 */
  }
  apply(BG_STYLES.find((s) => s.id === saved) ?? BG_STYLES[0]);
}

/** 循环切换到下一档背景，返回新背景名称（供 Toast 提示） */
export function cycleBackground(): string {
  const idx = Math.max(
    0,
    BG_STYLES.findIndex((s) => s.id === current),
  );
  const next = BG_STYLES[(idx + 1) % BG_STYLES.length];
  apply(next);
  return next.label;
}
