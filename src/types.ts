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
