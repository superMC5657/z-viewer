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
  /** 0=关闭 1=开启（前后各1） 2=高等级（前1后3） */
  cache_level: number;
  /** 文件夹首图队列深度（默认 1） */
  folder_first_depth: number;
}

/** Rust 端 license::LicenseInfo */
export interface LicenseInfo {
  /** "pro" | "free" */
  status: string;
  /** 当前设备指纹 */
  device_id?: string | null;
  /** 当前 JWT 的等级标识，如 pro / enterprise */
  level?: string | null;
  /** 当前 JWT 的等级名称，如 专业版 */
  levelLabel?: string | null;
  /** 本地保存的激活码 */
  code?: string | null;
  /** 本地保存的购买邮箱 */
  email?: string | null;
}

/** Rust 端 license::StoreInfo（购买入口） */
export interface StoreInfo {
  /** 产品标识（官网购买页 ?product= 参数） */
  product: string;
  /** 官网购买页 URL；null = 未配置 */
  buyUrl: string | null;
}

/** 动画候选扩展名（可能多帧，需 Rust 拆帧判定；格式集合稳定） */
const ANIM_EXTS = new Set(["gif", "png", "webp"]);

/**
 * RAW 扩展名：以后端 decode::RAW_EXTS 为唯一真源 —— init 时通过
 * get_raw_extensions 命令拉取并整表替换（见 setRawExts）。此处的内置
 * 副本仅作命令就绪前/失败时的兜底，避免初始化早期误判通道；后端新增
 * RAW 格式时无需同步改这里。
 */
const RAW_EXTS = new Set([
  "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
  "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
]);

/** 用后端 RAW_EXTS 整表替换本地兜底集（init 时调用一次） */
export function setRawExts(exts: string[]): void {
  RAW_EXTS.clear();
  for (const e of exts) RAW_EXTS.add(e.toLowerCase());
}

/** 判断图片是否需走 IPC 通道（动画/RAW），false 则浏览器原生解码直接 asset */
export function needsIpc(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return ANIM_EXTS.has(ext) || RAW_EXTS.has(ext);
}
