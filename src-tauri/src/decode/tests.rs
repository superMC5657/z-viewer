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
    let result = load_image(&test_img("test-images/B/img_2.gif")).unwrap();
    assert_eq!(result.mode, "animated", "两帧 GIF 应识别为动画");
    let frames = result.frames.expect("动画帧应有数据");
    assert!(frames.len() >= 2, "GIF 至少 2 帧，实际 {}", frames.len());
    for f in &frames {
        assert!(!f.png.is_empty(), "帧 PNG 不应为空");
        assert!(f.delay_ms > 0, "帧延迟应 > 0，实际 {}", f.delay_ms);
    }
    // 测试生成的 GIF 每帧 400ms
    assert_eq!(frames[0].delay_ms, 400);
}

#[test]
fn static_png_goes_asset() {
    let result = load_image(&test_img("test-images/A/1.png")).unwrap();
    assert_eq!(result.mode, "asset");
    assert!(result.data.is_none());
    assert!(result.frames.is_none());
}

#[test]
fn raw_ext_detection() {
    assert!(is_raw_ext("CR2"));
    assert!(is_raw_ext("nef"));
    assert!(is_raw_ext("DNG"));
    assert!(!is_raw_ext("jpg"));
    assert!(!is_raw_ext("png"));
}

#[test]
fn sub_millisecond_delay_clamped_to_1ms() {
    // 5/10ms 整数除法截断为 0：应兜底为 1ms（否则前端 setTimeout(0) 全速疯转）
    let buffer = image::RgbaImage::new(1, 1);
    let delay = image::Delay::from_numer_denom_ms(5, 10);
    let frame = image::Frame::from_parts(buffer, 0, 0, delay);
    let fd = animation::encode_frame(frame).unwrap();
    assert_eq!(fd.delay_ms, 1);
}
