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

/** 导航边界类型（与 Rust navigator::Boundary 序列化值一一对应，后端新增取值时两侧同步） */
export type BoundaryKind =
  | "first-image"
  | "last-image"
  | "first-folder"
  | "last-folder"
  | "pro-required";

export interface NavResult {
  /** 边界类型（见 BoundaryKind） */
  boundary: BoundaryKind | null;
  state: BrowseState | null;
}

/** Rust 端 AppSettings */
export interface AppSettings {
  /** 0=关闭 1=开启（前后各1） 2=高等级（前1后3） */
  cache_level: number;
  /** 文件夹首图队列深度（默认 1） */
  folder_first_depth: number;
}

/** Rust 端 license::LicenseInfo（跨 IPC 的富权益答案；features 与 status 同源） */
export interface LicenseInfo {
  /** 授权状态："pro"（专业版）| "free"（免费版） */
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
  /** JWT 权益位（与 status 同源；前端由 LicenseInfo 快照派生答案） */
  features: string[];
}

/** Rust 端 license::StoreInfo（购买入口） */
export interface StoreInfo {
  /** 产品标识（官网购买页 ?product= 参数） */
  product: string;
  /** 官网购买页 URL；null = 未配置 */
  buyUrl: string | null;
}

/** 解码加载模式（与 Rust decode::load_mode 常量一一对应，两侧修改需同步） */
export type LoadMode = "asset" | "raw" | "static" | "animated";

/** Rust 端二进制信封头（load_image 返回 ArrayBuffer，parseLoadEnvelope 解析） */
export interface LoadEnvelope {
  /** 加载模式（见 LoadMode） */
  mode: LoadMode;
  /** payload 的 MIME（raw/static: image/jpeg 或 image/x-rgba；animated: image/png 或 image/x-rgba 原始帧直通） */
  mime: string | null;
  /** raw 通道内嵌预览位：true 表示 payload 是内嵌预览，全量仍在后台解码 */
  isPreview: boolean;
  /** 图像尺寸（raw/static 通道与 image/x-rgba 动画通道提供） */
  width: number | null;
  height: number | null;
  /** animated：每帧延迟（ms） */
  frameDelays: number[];
  /** animated：每帧字节长（payload 按此切分） */
  frameSizes: number[];
}

/** 模块级单例：parseLoadEnvelope 每次调用复用（TextDecoder 实例可重复 decode） */
const envelopeDecoder = new TextDecoder();

/** 解析 Rust 端二进制信封：[魔数 IMGV][header_len LE][header JSON][payload] */
export function parseLoadEnvelope(buf: ArrayBuffer): { header: LoadEnvelope; payload: Uint8Array } {
  if (buf.byteLength < 8) throw new Error("解码响应过短");
  const magic = envelopeDecoder.decode(new Uint8Array(buf, 0, 4));
  if (magic !== "IMGV") throw new Error("非法解码响应");
  const dv = new DataView(buf);
  const headerLen = dv.getUint32(4, true);
  if (8 + headerLen > buf.byteLength) throw new Error("解码响应头损坏");
  const h = JSON.parse(envelopeDecoder.decode(new Uint8Array(buf, 8, headerLen)));
  return {
    header: {
      mode: h.mode,
      mime: h.mime ?? null,
      isPreview: !!h.is_preview,
      width: h.width ?? null,
      height: h.height ?? null,
      frameDelays: h.frame_delays ?? [],
      frameSizes: h.frame_sizes ?? [],
    },
    payload: new Uint8Array(buf, 8 + headerLen),
  };
}

/**
 * 需走 Rust IPC 解码通道的扩展名（RAW + TIFF + JXL）。
 * 动画由浏览器原生 <img> 播放，帧控制按需拆帧时走 load_image。
 */
const RUST_EXTS = new Set([
  "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
  "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
  "tif", "tiff",
  "jxl",
]);

/** 以后端 decode::ipc_exts 更新扩展名集合 */
export function setRawExts(exts: string[]): void {
  RUST_EXTS.clear();
  for (const e of exts) {
    RUST_EXTS.add(e.toLowerCase());
  }
}

/** 判断图片是否需走 IPC 通道（RAW/TIFF/JXL），false 则浏览器原生解码直接 asset */
export function needsIpc(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return RUST_EXTS.has(ext);
}

/** 潜在动画格式（原生 <img> 播放；仅这些格式需要 check_animation 判定）。
 *  兜底集：真源为后端 decode::format::ANIM_EXTS，启动时经 get_anim_extensions 替换 */
const ANIM_EXTS = new Set(["gif", "png", "webp"]);

/** 以后端 decode::ANIM_EXTS 更新动画扩展名集合（帧条判定唯一真源，避免前后端两份清单） */
export function setAnimExts(exts: string[]): void {
  ANIM_EXTS.clear();
  for (const e of exts) {
    ANIM_EXTS.add(e.toLowerCase());
  }
}

/** 判断扩展名是否为潜在动画格式（帧条显示/按需拆帧判定的前置闸） */
export function isPotentialAnim(ext: string): boolean {
  return ANIM_EXTS.has(ext);
}
