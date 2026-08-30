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

/** Rust 端二进制信封头（load_image 返回 ArrayBuffer，parseLoadEnvelope 解析） */
export interface LoadEnvelope {
  /** "asset" | "raw" | "static"(TIFF) | "animated" */
  mode: string;
  /** payload 的 MIME（raw/static: image/jpeg；animated: image/png） */
  mime: string | null;
  /** raw 通道内嵌预览位：true 表示 payload 是内嵌预览，全量仍在后台解码 */
  isPreview: boolean;
  /** 图像尺寸（raw/static 通道提供） */
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
 * 需走 Rust IPC 解码通道的扩展名：RAW + TIFF。
 * 动画（GIF/APNG/WebP）由浏览器原生 <img> 播放（首帧即时、零 IPC），
 * 帧控制（暂停/步进）按需拆帧时再走 load_image。
 */
const RUST_EXTS = new Set([
  "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
  "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
  "tif", "tiff",
]);

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
  // 清掉旧的 RAW 贡献，保留 TIFF 常驻
  for (const e of [...RUST_EXTS]) {
    if (e !== "tif" && e !== "tiff") RUST_EXTS.delete(e);
  }
  RAW_EXTS.clear();
  for (const e of exts) {
    RAW_EXTS.add(e.toLowerCase());
    RUST_EXTS.add(e.toLowerCase());
  }
}

/** 判断图片是否需走 IPC 通道（RAW/TIFF），false 则浏览器原生解码直接 asset */
export function needsIpc(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return RUST_EXTS.has(ext);
}
