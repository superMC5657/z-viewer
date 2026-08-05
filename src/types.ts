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
}

export interface NavResult {
  /** "first-image" | "last-image" | "first-folder" | "last-folder" | null */
  boundary: string | null;
  state: BrowseState | null;
}
