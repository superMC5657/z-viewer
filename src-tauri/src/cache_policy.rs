//! 缓存策略：写缓存 / 预取两条纯规则（Whether），Where 归 DecodeCache。
//!
//! 两条入口均为纯函数（无 I/O、无锁）。调用方不再在多个浅谓词
//! （is_asset / is_asset_ext / 手写 mode 比对 …）之间自行挑选：
//! - [`should_cache`]：解码后写过滤（load_image demand 路径 + 预取 put 前 + 提升前）
//! - [`should_prefetch`]：派生前扩展名过滤（纯查表，零 I/O，Navigator 唯一调用方）
//!
//! 清单唯一真源仍是 decode::format（ASSET_EXTS 等表）：本模块只转述规则，
//! 绝不复制扩展名表。DecodeCache::put 继续决定 WHERE（animated 独立单槽
//! vs LRU），cap_dimensions / finalize_rgba / pack_envelope 留在 decode。
//!
//! 动画探针刻意不合并：is_animated 是廉价 UI 提示（只读文件头、不解码），
//! 而动画真值（多帧 vs 单帧降级）只在解码后由 should_cache 裁决；
//! should_prefetch 因此保持 I/O-free 的纯扩展名查表（gif 照例走前端池——
//! 若某 gif 解码后确为多帧，demand 路径的 should_cache 仍会收留它）。

use crate::decode::{is_asset_ext, LoadResult};

/// 解码结果是否值得占缓存槽位：非 asset 全量 / 静态 / 动画帧序列。
/// 单帧降级（mode == asset）与内嵌预览（后续全量会覆盖）不占槽位。
pub fn should_cache(result: &LoadResult) -> bool {
    result.mode != "asset" && !result.is_preview
}

/// 该路径是否值得派生预取任务：仅扩展名查表（无 I/O）。
/// asset 通道由前端 PrefetchPool 预热，Rust 不重复解码。
pub fn should_prefetch(path: &str) -> bool {
    !is_asset_ext(path)
}

#[cfg(test)]
mod tests {
    //! 规则表测试：构造值覆盖全通道 + 真实解码入口抽查有真实文件的通道
    //! （raw/static 无真实测试文件，构造表覆盖；asset/animated 走真实解码）。

    use super::*;
    use crate::decode::load_image;

    #[test]
    fn cache_rule_table() {
        // asset 占位（单帧降级 / 无 Rust 解码需求）→ 不缓存。
        assert!(!should_cache(&LoadResult::asset()));
        // 内嵌预览 → 不缓存（后续全量请求会覆盖）。
        assert!(!should_cache(&LoadResult::jpeg("raw", vec![1], 4, 4, true)));
        // raw / static 全量 → 缓存。
        assert!(should_cache(&LoadResult::jpeg("raw", vec![1], 4, 4, false)));
        assert!(should_cache(&LoadResult::jpeg(
            "static",
            vec![1],
            4,
            4,
            false
        )));
        // RGBA 全量（TIFF / 全量 RAW 极速直显）→ 缓存。
        assert!(should_cache(&LoadResult::rgba(
            "raw",
            vec![1; 16],
            2,
            2,
            false
        )));
        // 动画帧序列 → 缓存（独立单槽还是 LRU 由 put 决定，不在这里）。
        assert!(should_cache(&LoadResult::animated(
            vec![vec![1, 2]],
            vec![40]
        )));
        // 单帧降级：即使带 payload，mode == asset 就不缓存。
        let mut degraded = LoadResult::animated(vec![vec![1]], vec![40]);
        degraded.mode = "asset".into();
        assert!(!should_cache(&degraded));
    }

    #[test]
    fn prefetch_rule_table() {
        // asset 扩展名 → 走前端池，Rust 不预取（含大小写）。
        for ext in ["jpg", "jpeg", "png", "gif", "webp", "bmp", "JPG", "PNG"] {
            assert!(!should_prefetch(&format!("img.{ext}")), "{ext} 应走前端池");
        }
        // RAW / TIFF → 值得预取（含大写）。
        for ext in ["cr2", "CR2", "nef", "tif", "tiff", "dng"] {
            assert!(should_prefetch(&format!("img.{ext}")), "{ext} 应预取");
        }
    }

    /// 真实解码入口抽查：asset 图与动画 GIF 经 load_image 的 verdict 与规则一致。
    #[test]
    fn real_decode_channels_agree() {
        let base = std::env::current_dir()
            .unwrap()
            .join("..")
            .join("test-images");
        // asset 通道：单帧 PNG → asset → 不缓存，但扩展名侧本就不预取。
        let asset = base.join("A/1.png").to_string_lossy().to_string();
        let r = load_image(&asset, false).unwrap();
        assert_eq!(r.mode, "asset");
        assert!(!should_cache(&r));
        assert!(!should_prefetch(&asset));
        // 动画通道：多帧 GIF → animated → 可缓存；扩展名侧仍走前端池
        // （预取不触发，demand 命中才收留——两条规则各司其职）。
        let anim = base.join("B/img_2.gif").to_string_lossy().to_string();
        let r = load_image(&anim, false).unwrap();
        assert_eq!(r.mode, "animated");
        assert!(should_cache(&r));
        assert!(!should_prefetch(&anim));
    }
}
