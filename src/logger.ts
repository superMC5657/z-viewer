/**
 * 开发日志（仅 dev 模式打印，生产构建自动消除）
 *
 * 约定：
 * - 前端日志统一前缀 [FE]（青色）；后端 Rust 日志前缀 [BE]（黄色），便于区分
 * - 每条日志带本地时间戳 `HH:mm:ss.SSS`（与后端 chrono 格式对齐）
 * - import.meta.env.DEV 在 `vite build`（生产）时被替换为 false，日志代码被 tree-shake 消除
 */

const PREFIX = "%c[FE]";
const STYLE = "color: #00bcd4; font-weight: 600;";

/** 本地时间戳 HH:mm:ss.SSS（与后端 chrono 格式对齐） */
function timestamp(): string {
  const d = new Date();
  const pad = (n: number, l = 2) => String(n).padStart(l, "0");
  return (
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` +
    `.${pad(d.getMilliseconds(), 3)}`
  );
}

/** 关键步骤日志：仅 vite dev 模式输出，带本地时间戳 */
export function feLog(msg: string, ...args: unknown[]): void {
  if (!import.meta.env.DEV) return;
  // eslint-disable-next-line no-console
  console.log(PREFIX, STYLE, timestamp(), msg, ...args);
}
