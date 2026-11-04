//! decode 模块测试（#[cfg(test)] 独立文件，不混入产品代码）

use super::*;

/// 测试图库相对路径（从 src-tauri 工作目录运行）
fn test_img(rel: &str) -> String {
    std::env::current_dir()
        .unwrap()
        .join("..")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

#[test]
fn gif_animation_frames() {
    let result = load_image(&test_img("test-images/B/img_2.gif"), false).unwrap();
    assert_eq!(result.mode, "animated", "两帧 GIF 应识别为动画");
    assert!(
        result.frame_sizes.len() >= 2,
        "GIF 至少 2 帧，实际 {}",
        result.frame_sizes.len()
    );
    assert_eq!(
        result.frame_delays.len(),
        result.frame_sizes.len(),
        "延迟与帧一一对应"
    );
    let total: u32 = result.frame_sizes.iter().sum();
    assert_eq!(
        total,
        result.bytes.len() as u32,
        "帧长总和应等于 payload 长度"
    );
    for d in &result.frame_delays {
        assert!(*d > 0, "帧延迟应 > 0，实际 {d}");
    }
    // 测试生成的 GIF 每帧 400ms
    assert_eq!(result.frame_delays[0], 400);
    // 每帧切片应是合法 PNG 字节
    let mut off = 0usize;
    for size in &result.frame_sizes {
        let slice = &result.bytes[off..off + *size as usize];
        assert!(
            slice.starts_with(&[0x89, b'P', b'N', b'G']),
            "帧应为 PNG 字节"
        );
        off += *size as usize;
    }
}

#[test]
fn static_png_goes_asset() {
    let result = load_image(&test_img("test-images/A/1.png"), false).unwrap();
    assert_eq!(result.mode, "asset");
    assert!(result.bytes.is_empty());
    assert!(result.frame_sizes.is_empty());
}

#[test]
fn jxl_decode_test() {
    let p = test_img("test-images/B/img_6.jxl");
    let result = load_image(&p, false).expect("JXL 静态解码应成功");
    assert_eq!(result.mode, "static");
    assert_eq!(result.mime.as_deref(), Some("image/x-rgba"));
    assert_eq!(result.width, Some(3));
    assert_eq!(result.height, Some(3));
    assert_eq!(result.bytes.len(), 3 * 3 * 4);
}

#[test]
fn jxl_animation_test() {
    let p = test_img("test-images/B/img_7_anim.jxl");
    assert!(is_animated(&p), "is_animated 应识别 JXL 动画");
    let p_static = test_img("test-images/B/img_6.jxl");
    assert!(!is_animated(&p_static), "静态 JXL 不应识别为动画");

    let result = load_image(&p, false).expect("JXL 动画解码应成功");
    assert_eq!(result.mode, "animated");
    // 小动画走原始 RGBA 帧直通通道（超预算大动画才回退 PNG）
    assert_eq!(result.mime.as_deref(), Some("image/x-rgba"));
    let (w, h) = (
        result.width.expect("动画通道应提供宽度"),
        result.height.expect("动画通道应提供高度"),
    );
    let frame_len = (w as u64) * (h as u64) * 4;
    assert!(result.frame_sizes.len() >= 2, "至少 2 帧");
    assert_eq!(result.frame_delays.len(), result.frame_sizes.len());
    for s in &result.frame_sizes {
        assert_eq!(*s as u64, frame_len, "每帧应为 w×h×4 原始 RGBA");
    }
    let total: u64 = result.frame_sizes.iter().map(|s| *s as u64).sum();
    assert_eq!(total, result.bytes.len() as u64);
    for d in &result.frame_delays {
        assert!(*d > 0, "帧延迟应 > 0，实际 {d}");
    }
}

#[test]
fn raw_ext_detection() {
    assert!(is_raw_ext("CR2"));
    assert!(is_raw_ext("nef"));
    assert!(is_raw_ext("DNG"));
    assert!(!is_raw_ext("jpg"));
    assert!(!is_raw_ext("png"));
    assert!(TIFF_EXTS.contains(&"tif"));
    assert!(TIFF_EXTS.contains(&"tiff"), "大小写不敏感判断由调用方负责");
    assert!(is_jxl_ext("jxl"));
    assert!(is_jxl_ext("JXL"));
    assert_eq!(classify_ext("jxl"), ImageFormatCategory::Jxl);
    assert_eq!(classify_ext("JXL"), ImageFormatCategory::Jxl);
    assert!(is_image_file("sample.jxl"));
    assert!(!is_asset("sample.jxl"));
}

#[test]
fn sub_millisecond_delay_clamped_to_1ms() {
    // 5/10ms 整数除法截断为 0：应兜底为 1ms（否则前端 setTimeout(0) 全速疯转）
    let buffer = image::RgbaImage::new(1, 1);
    let delay = image::Delay::from_numer_denom_ms(5, 10);
    let frame = image::Frame::from_parts(buffer, 0, 0, delay);
    let (_, delay_ms) = animation::encode_frame(frame, 1).unwrap();
    assert_eq!(delay_ms, 1);
}

#[test]
fn envelope_roundtrip() {
    let result = LoadResult::animated(vec![vec![1, 2, 3], vec![4, 5]], vec![100, 200]);
    let buf = pack_envelope(&result);
    assert_eq!(&buf[..4], ENVELOPE_MAGIC, "信封魔数");
    let hlen = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    let header: serde_json::Value = serde_json::from_slice(&buf[8..8 + hlen]).unwrap();
    assert_eq!(header["mode"], "animated");
    assert_eq!(header["frame_sizes"], serde_json::json!([3, 2]));
    assert_eq!(header["capped"], serde_json::json!(false));
    assert_eq!(&buf[8 + hlen..], &result.bytes[..], "payload 原样透传");
}

#[test]
fn jpeg_sof_dimensions_parse() {
    // 构造最小合法 JPEG：SOI + SOF0(高2宽3) + EOI
    let mut jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x03,
    ];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);
    let (w, h) = preview::jpeg_dimensions(&jpeg).expect("应解析出尺寸");
    assert_eq!((w, h), (3, 2));
}

#[test]
fn preview_tiff_walk_finds_embedded_jpeg() {
    // 合成小 TIFF（little-endian）：IFD0 含 JpgFromRaw(0x002E) → 内嵌 JPEG
    // 结构：header(8) + IFD0（1 条目 + next 指针）+ JPEG 数据
    let mut jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x04, 0x00, 0x05,
    ];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);
    let ifd_off = 8u32;
    let jpeg_off = ifd_off + 2 + 12 + 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"II*\x00");
    bytes.extend_from_slice(&ifd_off.to_le_bytes());
    // IFD0：1 条目（JpgFromRaw：tag=0x002E, type=LONG, count=len, value=offset）
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0x002Eu16.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&jpeg_off.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // 下一个 IFD 指针 = 0
    bytes.extend_from_slice(&jpeg);
    let dir = std::env::temp_dir().join("iv_preview_test.tif");
    std::fs::write(&dir, &bytes).unwrap();
    let p = preview::extract_preview(&dir.to_string_lossy()).expect("应提取到内嵌 JPEG");
    assert_eq!((p.width, p.height), (5, 4));
    assert!(p.jpeg.starts_with(&[0xFF, 0xD8]));
    std::fs::remove_file(&dir).ok();
}

#[test]
fn preview_raf_extracts_embedded_jpeg() {
    let mut jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x04, 0x00, 0x05,
    ];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);

    let mut data = vec![0u8; 128];
    data[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    let jpeg_off = 128u32;
    let jpeg_len = jpeg.len() as u32;
    data[84..88].copy_from_slice(&jpeg_off.to_be_bytes());
    data[88..92].copy_from_slice(&jpeg_len.to_be_bytes());
    data.extend_from_slice(&jpeg);

    let dir = std::env::temp_dir().join("iv_preview_raf_test.raf");
    std::fs::write(&dir, &data).unwrap();
    let p = preview::extract_preview(&dir.to_string_lossy()).expect("应提取到富士 RAF 内嵌 JPEG");
    assert_eq!((p.width, p.height), (5, 4));
    assert!(p.jpeg.starts_with(&[0xFF, 0xD8]));
    std::fs::remove_file(&dir).ok();
}

#[test]
fn preview_isobmff_cr3_extracts_embedded_jpeg() {
    let mut jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x04, 0x00, 0x05,
    ];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);

    let mut data = Vec::new();
    // ftyp 数据盒
    data.extend_from_slice(&16u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(b"crx \x00\x00\x00\x00");

    // uuid 数据盒 (PRVW 预览)
    let uuid_box_len = (8 + 16 + jpeg.len()) as u32;
    data.extend_from_slice(&uuid_box_len.to_be_bytes());
    data.extend_from_slice(b"uuid");
    data.extend_from_slice(b"PRVW\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
    data.extend_from_slice(&jpeg);

    let dir = std::env::temp_dir().join("iv_preview_cr3_test.cr3");
    std::fs::write(&dir, &data).unwrap();
    let p = preview::extract_preview(&dir.to_string_lossy()).expect("应提取到佳能 CR3 内嵌 JPEG");
    assert_eq!((p.width, p.height), (5, 4));
    assert!(p.jpeg.starts_with(&[0xFF, 0xD8]));
    std::fs::remove_file(&dir).ok();
}

// ============================================================
// JXL 引擎性能基准（jxl-libjxl 特性下）：libjxl 编码器现场生成
// 确定性纹理素材 → 双引擎解码计时。libjxl 静态库恒为 Release 构建
// （build.rs 显式锁定 CMAKE_BUILD_TYPE）、jxl-oxide 依赖在 dev 下已 O3，
// 数字可直接对比。
// ============================================================

#[cfg(feature = "jxl-libjxl")]
use crate::decode::bench_util::encode_jxl_fixture;

#[cfg(feature = "jxl-libjxl")]
#[test]
fn jxl_engine_bench() {
    use super::jxl::libjxl_engine;
    use std::time::Instant;

    const W: u32 = 3000;
    const H: u32 = 2000;

    // 确定性纹理（渐变 + 伪噪声），非平坦内容避免压缩器走捷径
    let mut rgba = vec![0u8; (W as usize) * (H as usize) * 4];
    for y in 0..H {
        for x in 0..W {
            let i = ((y as usize) * (W as usize) + x as usize) * 4;
            let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519)) & 0x3f) as u8;
            rgba[i] = ((x * 255 / W) as u8).wrapping_add(n);
            rgba[i + 1] = ((y * 255 / H) as u8).wrapping_add(n);
            rgba[i + 2] = (((x + y) & 0xff) as u8).wrapping_add(n);
            rgba[i + 3] = 255;
        }
    }

    let jxl_bytes = encode_jxl_fixture(&rgba, W, H);

    let tmp = std::env::temp_dir().join("z_viewer_bench.jxl");
    std::fs::write(&tmp, &jxl_bytes).expect("写入基准素材");
    let path = tmp.to_str().unwrap();

    // 各解码 3 次取最优（排除首跑冷启动/缓存效应）
    let mut t_libjxl = None;
    for _ in 0..3 {
        let t0 = Instant::now();
        let out = libjxl_engine::decode(path).expect("libjxl 引擎应解码成功");
        t_libjxl = Some(t_libjxl.map_or(t0.elapsed(), |best: std::time::Duration| {
            best.min(t0.elapsed())
        }));
        assert_eq!((out.width, out.height), (W, H));
        assert_eq!(out.frames[0].len(), rgba.len());
    }
    let t_libjxl = t_libjxl.unwrap();

    // jxl-oxide 对照（纯 Rust + rayon）
    let mut t_oxide = None;
    for _ in 0..3 {
        let t1 = Instant::now();
        let img = jxl_oxide::JxlImage::builder()
            .open(path)
            .expect("jxl-oxide 应解码成功");
        let render = img.render_frame(0).expect("jxl-oxide 应渲染成功");
        let mut s = render.stream();
        let mut buf =
            vec![0u8; (s.width() as usize) * (s.height() as usize) * (s.channels() as usize)];
        s.write_to_buffer(&mut buf);
        t_oxide = Some(t_oxide.map_or(t1.elapsed(), |best: std::time::Duration| {
            best.min(t1.elapsed())
        }));
    }
    let t_oxide = t_oxide.unwrap();

    let _ = std::fs::remove_file(&tmp);

    let mpx = (W as f64) * (H as f64) / 1e6;
    println!(
        "JXL 解码基准 {W}×{H}（{mpx:.0} MP，码流 {:.0} KB）",
        jxl_bytes.len() as f64 / 1024.0
    );
    println!(
        "libjxl   : {t_libjxl:?}  ({:.0} MP/s)",
        mpx / t_libjxl.as_secs_f64()
    );
    println!(
        "jxl-oxide: {t_oxide:?}  ({:.0} MP/s)",
        mpx / t_oxide.as_secs_f64()
    );
}

// ============================================================
// 解码分段剖析：冷解码的耗时构成（读盘/引擎解码/收尾/信封）。
// 数字解读与优化决策见 .scratch/pm-followup-20261024/issues/06。
// ============================================================

#[cfg(feature = "jxl-libjxl")]
#[test]
#[ignore = "手动剖析：cargo test --release --ignored decode_component_profile -- --nocapture"]
fn decode_component_profile() {
    use super::jxl::libjxl_engine;
    use std::time::Instant;

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    println!("可用并行度: {cores}（JXL 引擎线程钳制 ≤8）");

    // ---- JXL 24MP（与 flip_latency_bench 同款确定性噪声纹理）----
    let (w, h) = (6000u32, 4000u32);
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let o = (y as usize * w as usize + x as usize) * 4;
            let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519)) & 0x3f) as u8;
            rgba[o] = ((x * 255 / w) as u8).wrapping_add(n);
            rgba[o + 1] = ((y * 255 / h) as u8).wrapping_add(n);
            rgba[o + 2] = (((x + y) & 0xff) as u8).wrapping_add(n);
            rgba[o + 3] = 255;
        }
    }
    let bytes = encode_jxl_fixture(&rgba, w, h);
    let tmp = std::env::temp_dir().join("zviewer-profile.jxl");
    std::fs::write(&tmp, &bytes).unwrap();
    let path = tmp.to_str().unwrap();

    let t = Instant::now();
    let _raw = std::fs::read(path).unwrap();
    let t_read = t.elapsed();

    let t = Instant::now();
    let out = libjxl_engine::decode(path).expect("libjxl 解码成功");
    let t_engine = t.elapsed();
    assert_eq!(out.frames[0].len(), rgba.len());

    let t = Instant::now();
    let img = image::RgbaImage::from_raw(out.width, out.height, out.frames[0].clone())
        .map(image::DynamicImage::ImageRgba8)
        .unwrap();
    let result = finalize_rgba(img, 1, load_mode::STATIC, MAX_DIM);
    let t_finalize = t.elapsed();

    let t = Instant::now();
    let envelope = pack_envelope(&result);
    let t_pack = t.elapsed();

    println!(
        "JXL {w}×{h}（24MP，码流 {:.1} MB）",
        bytes.len() as f64 / 1048576.0
    );
    println!("  读盘                 : {t_read:?}");
    println!("  引擎解码             : {t_engine:?}");
    println!(
        "  收尾(方向+缩放+RGBA) : {t_finalize:?} → {}×{}",
        result.width.unwrap_or(0),
        result.height.unwrap_or(0)
    );
    println!(
        "  信封打包             : {t_pack:?}（{:.1} MB）",
        envelope.len() as f64 / 1048576.0
    );
    let _ = std::fs::remove_file(&tmp);

    // ---- TIFF 12MP ----
    let (tw, th) = (4000u32, 3000u32);
    let tif_img = image::RgbImage::from_fn(tw, th, |x, y| {
        let n = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519)) & 0x3f) as u8;
        image::Rgb([
            ((x * 255 / tw) as u8).wrapping_add(n),
            ((y * 255 / th) as u8).wrapping_add(n),
            (((x + y) & 0xff) as u8).wrapping_add(n),
        ])
    });
    let tif_tmp = std::env::temp_dir().join("zviewer-profile.tif");
    tif_img
        .save_with_format(&tif_tmp, image::ImageFormat::Tiff)
        .unwrap();
    let tif_path = tif_tmp.to_str().unwrap();

    let t = Instant::now();
    let (file, orientation) = preview::open_with_orientation(tif_path).unwrap();
    let t_open = t.elapsed();
    let t = Instant::now();
    let decoded = image::ImageReader::new(std::io::BufReader::new(&file))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    let t_decode = t.elapsed();
    let t = Instant::now();
    let result2 = finalize_rgba(decoded, orientation, load_mode::STATIC, MAX_DIM);
    let t_finalize2 = t.elapsed();

    println!("TIFF {tw}×{th}（12MP 无压缩）");
    println!("  open(含 EXIF 方向)   : {t_open:?}");
    println!("  image 解码           : {t_decode:?}");
    println!(
        "  收尾(缩放+RGBA)      : {t_finalize2:?} → {}×{}",
        result2.width.unwrap_or(0),
        result2.height.unwrap_or(0)
    );
    let _ = std::fs::remove_file(&tif_tmp);
}

#[test]
fn original_decode_bypasses_display_cap() {
    // 4000×2800 TIFF：常规解码 capped 到 3840 且带 capped 标记；
    // original 解码（深放大真像素）保持原始尺寸且不标记
    let dir = std::env::temp_dir().join(format!("zviewer-cap-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("big.tif");
    let img = image::RgbImage::from_fn(4000, 2800, |x, y| {
        image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
    });
    img.save_with_format(&p, image::ImageFormat::Tiff).unwrap();
    let path = p.to_str().unwrap();

    let r = load_image(path, false).unwrap();
    assert_eq!(r.width, Some(3840), "常规解码应 capped 到显示帽");
    assert!(r.capped, "常规解码应标记已降采样");

    let o = load_image_original(path).unwrap();
    assert_eq!(o.width, Some(4000), "original 解码应保持原始尺寸");
    assert!(!o.capped, "original 解码未降采样");
    let _ = std::fs::remove_dir_all(&dir);
}

// ============================================================
// 分阶段计时机制：force 开关 / 同名累计 / 会话清空
// ============================================================

#[test]
fn stage_timed_accumulates_and_begin_clears() {
    // 计时段用 sleep 下界断言（busy loop 会被常量折叠，可能测出 0ms）
    super::stage::force(true);
    assert!(super::stage::begin(), "force 开启时应激活");
    super::stage::timed("read", || {
        std::thread::sleep(std::time::Duration::from_millis(2))
    });
    super::stage::timed("read", || {
        std::thread::sleep(std::time::Duration::from_millis(2))
    }); // 同名累计
    super::stage::timed("engine", || {
        std::thread::sleep(std::time::Duration::from_millis(1))
    });
    let snap = super::stage::snapshot();
    assert_eq!(snap.iter().filter(|(n, _)| *n == "read").count(), 1);
    assert_eq!(snap.iter().filter(|(n, _)| *n == "engine").count(), 1);
    let read = snap.iter().find(|(n, _)| *n == "read").unwrap().1;
    assert!(read >= 4.0, "同名阶段应累计（实际 {read}ms）");
    let engine = snap.iter().find(|(n, _)| *n == "engine").unwrap().1;
    assert!(engine >= 1.0, "engine 段应有独立累计（实际 {engine}ms）");

    assert!(super::stage::begin(), "会话开始应再次激活");
    assert!(super::stage::snapshot().is_empty(), "会话开始应清空累计");

    super::stage::force(false);
    assert!(!super::stage::begin(), "关闭后不激活");
}
