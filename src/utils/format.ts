/**
 * 纯文本与读数格式化工具函数
 * 提供文件名截断、文件大小、位置进度、动画帧计数等纯函数
 */

/** 文件名过长时中间截断：photo_very_long…name.jpg（草图 3.3） */
export function truncateMiddle(name: string, maxLen: number): string {
  if (name.length <= maxLen) return name;
  const keep = Math.floor((maxLen - 1) / 2);
  return name.slice(0, keep) + "…" + name.slice(name.length - keep);
}

/** 文件大小格式化（草图样例：4.2 MB） */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

/**
 * 位置进度格式化：如 "3/128" 或后台枚举中 "3/…"
 * @param current 当前序号（1-based）
 * @param total 总数量或占位符
 * @param loading 是否处于枚举/加载中
 */
export function formatPosition(
  current: number,
  total: number | string,
  loading: boolean = false,
): string {
  return loading ? `${current}/…` : `${current}/${total}`;
}

/**
 * 动画帧计数读数格式化：如 "帧 12/48"、未知态 "帧 …"
 * @param index 帧序号（1-based），缺省或 null 为未知态
 * @param total 帧总数，缺省或 null 为未知态
 */
export function formatFrame(
  index?: number | null,
  total?: number | null,
): string {
  if (index == null || total == null) {
    return "帧 …";
  }
  return `帧 ${index}/${total}`;
}
