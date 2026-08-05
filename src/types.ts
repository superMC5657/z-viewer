/** 与 Rust 端 BrowseState / NavResult 对应的类型 */

export interface BrowseState {
  path: string;
  file_name: string;
  folder_name: string;
  file_size: number;
  /** 全局位置（0-based，跨文件夹累计） */
  global_index: number;
  global_total: number;
  folder_index: number;
  folder_total: number;
  /** 后台枚举是否仍在进行（true 时位置显示 "3/…"） */
  loading: boolean;
}

export interface NavResult {
  /** "first-image" | "last-image" | "first-folder" | "last-folder" | null */
  boundary: string | null;
  state: BrowseState | null;
}

/** Rust 端 decode::FrameData */
export interface FrameData {
  /** base64 编码的 PNG 帧 */
  png: string;
  delay_ms: number;
}

/** Rust 端 decode::LoadResult */
export interface LoadResult {
  /** "asset" | "raw" | "animated" */
  mode: string;
  /** raw 模式：base64 JPEG */
  data?: string | null;
  frames?: FrameData[] | null;
}

/** Rust 端 AppSettings */
export interface AppSettings {
  cache_strength: number;
}

/** 需走 IPC 的扩展名（可能多帧动画或 RAW），其余浏览器原生解码直接 asset */
const IPC_EXTS = new Set([
  "gif", "png", "webp", // 可能动画，需 Rust 拆帧判断
  "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
  "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari", // RAW
]);

/** 判断图片是否需走 IPC 通道（动画/RAW），false 则浏览器原生解码直接 asset */
export function needsIpc(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return IPC_EXTS.has(ext);
}
