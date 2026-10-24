//! libjxl 解码引擎：流式 JxlDecoder + 可伸缩并行 runner（SIMD + 多线程）
//! 统一输出 RGBA8；sRGB 输出档在 COLOR_ENCODING 时点请求（libjxl 无 CMS 时
//! 仅对 XYB 码流执行转换，其余保留原生色彩空间，见 ADR 0001 决策 5）

use std::ptr;

use super::libjxl_ffi as ffi;
use super::EngineOutput;

/// 并行线程数上限：libjxl 对小图自动收缩，超过 8 线程收益递减
const MAX_THREADS: usize = 8;

struct DecoderGuard(*mut ffi::JxlDecoder);
impl Drop for DecoderGuard {
    fn drop(&mut self) {
        unsafe { ffi::JxlDecoderDestroy(self.0) }
    }
}

struct RunnerGuard(*mut ffi::c_void);
impl Drop for RunnerGuard {
    fn drop(&mut self) {
        unsafe { ffi::JxlResizableParallelRunnerDestroy(self.0) }
    }
}

fn status_name(status: ffi::JxlDecoderStatus) -> &'static str {
    match status {
        ffi::JXL_DEC_ERROR => "解码错误",
        ffi::JXL_DEC_NEED_MORE_INPUT => "码流数据截断",
        _ => "未支持的状态",
    }
}

fn rgba8_format() -> ffi::JxlPixelFormat {
    ffi::JxlPixelFormat {
        num_channels: 4,
        data_type: ffi::JXL_TYPE_UINT8,
        endianness: ffi::JXL_LITTLE_ENDIAN,
        align: 0,
    }
}

fn setup_runner(dec: *mut ffi::JxlDecoder) -> Result<RunnerGuard, String> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_THREADS);
    let runner = unsafe { ffi::JxlResizableParallelRunnerCreate(ptr::null()) };
    if runner.is_null() {
        return Err("JXL 并行 runner 创建失败".into());
    }
    unsafe {
        ffi::JxlResizableParallelRunnerSetThreads(runner, threads);
        let st =
            ffi::JxlDecoderSetParallelRunner(dec, Some(ffi::JxlResizableParallelRunner), runner);
        if st != ffi::JXL_DEC_SUCCESS {
            ffi::JxlResizableParallelRunnerDestroy(runner);
            return Err(format!("JXL 并行 runner 绑定失败（status={st}）"));
        }
    }
    Ok(RunnerGuard(runner))
}

/// 轻量探测：只订阅 BASIC_INFO，读到头部即返回是否动画
pub(super) fn probe_animated(path: &str) -> Option<bool> {
    let data = std::fs::read(path).ok()?;
    let dec = unsafe { ffi::JxlDecoderCreate(ptr::null()) };
    if dec.is_null() {
        return None;
    }
    let _dec = DecoderGuard(dec);
    let _runner = setup_runner(dec).ok()?;

    unsafe {
        let mask = ffi::JXL_DEC_BASIC_INFO as ffi::size_t as u32;
        if ffi::JxlDecoderSubscribeEvents(dec, mask) != ffi::JXL_DEC_SUCCESS {
            return None;
        }
        if ffi::JxlDecoderSetInput(dec, data.as_ptr(), data.len()) != ffi::JXL_DEC_SUCCESS {
            return None;
        }
        ffi::JxlDecoderCloseInput(dec);

        // 每个分支都会立即返回：订阅的只有 BASIC_INFO 事件，一次 ProcessInput 即出结果
        match ffi::JxlDecoderProcessInput(dec) {
            ffi::JXL_DEC_BASIC_INFO => {
                let mut info = ffi::JxlBasicInfo::zeroed();
                if ffi::JxlDecoderGetBasicInfo(dec, &mut info) != ffi::JXL_DEC_SUCCESS {
                    return None;
                }
                Some(info.have_animation == ffi::JXL_TRUE)
            }
            ffi::JXL_DEC_SUCCESS => Some(false),
            _ => None,
        }
    }
}

pub(in crate::decode) fn decode(path: &str) -> Result<EngineOutput, String> {
    let data = std::fs::read(path).map_err(|e| format!("JXL 文件读取失败: {e}"))?;
    let dec = unsafe { ffi::JxlDecoderCreate(ptr::null()) };
    if dec.is_null() {
        return Err("JXL 解码器创建失败".into());
    }
    let _dec = DecoderGuard(dec);
    let _runner = setup_runner(dec)?;

    let mask = (ffi::JXL_DEC_BASIC_INFO
        | ffi::JXL_DEC_COLOR_ENCODING
        | ffi::JXL_DEC_FRAME
        | ffi::JXL_DEC_FULL_IMAGE) as u32;
    check_status(unsafe { ffi::JxlDecoderSubscribeEvents(dec, mask) })?;

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut delays_ms: Vec<u32> = Vec::new();
    let mut basic: Option<ffi::JxlBasicInfo> = None;
    let mut pending_dur_ticks: u32 = 0;
    let mut buf: Vec<u8> = Vec::new();

    unsafe {
        if ffi::JxlDecoderSetInput(dec, data.as_ptr(), data.len()) != ffi::JXL_DEC_SUCCESS {
            return Err("JXL 输入绑定失败".into());
        }
        ffi::JxlDecoderCloseInput(dec);

        loop {
            match ffi::JxlDecoderProcessInput(dec) {
                ffi::JXL_DEC_BASIC_INFO => {
                    let mut info = ffi::JxlBasicInfo::zeroed();
                    check_status(ffi::JxlDecoderGetBasicInfo(dec, &mut info))?;
                    basic = Some(info);
                }
                ffi::JXL_DEC_COLOR_ENCODING => {
                    // 输出色彩档仅可在 COLOR_ENCODING 事件时点设置（decode.h 契约）。
                    // 无 CMS 时 libjxl 只对 XYB 码流执行 sRGB 转换；非 XYB 且非 sRGB
                    // 的文件（宽色域/嵌入式 ICC）此调用报错——按 ADR 0001 决策 5
                    // 保留原生色彩空间继续解码（轻微偏色可接受），置为 best-effort
                    let enc = ffi::srgb_color_encoding();
                    let _ = ffi::JxlDecoderSetPreferredColorProfile(dec, &enc);
                }
                ffi::JXL_DEC_NEED_IMAGE_OUT_BUFFER => {
                    let info = basic.as_ref().ok_or("JXL 基础信息缺失")?;
                    let size = (info.xsize as usize) * (info.ysize as usize) * 4;
                    buf = vec![0u8; size];
                    check_status(ffi::JxlDecoderSetImageOutBuffer(
                        dec,
                        &rgba8_format(),
                        buf.as_mut_ptr(),
                        buf.len(),
                    ))?;
                }
                ffi::JXL_DEC_FRAME => {
                    let mut hdr = ffi::JxlFrameHeader::zeroed();
                    check_status(ffi::JxlDecoderGetFrameHeader(dec, &mut hdr))?;
                    pending_dur_ticks = hdr.duration;
                }
                ffi::JXL_DEC_FULL_IMAGE => {
                    frames.push(buf.clone());
                    let info = basic.as_ref().ok_or("JXL 基础信息缺失")?;
                    let anim = info.animation;
                    delays_ms.push(super::ticks_to_ms(
                        pending_dur_ticks,
                        anim.tps_numerator,
                        anim.tps_denominator,
                    ));
                }
                ffi::JXL_DEC_SUCCESS => break,
                st => return Err(format!("JXL 解码失败: {}", status_name(st))),
            }
        }
    }

    let info = basic.as_ref().ok_or("JXL 码流缺少基础信息")?;
    if frames.is_empty() {
        return Err("JXL 码流不含图像帧".into());
    }

    let (w, h) = (info.xsize, info.ysize);
    let have_anim = info.have_animation == ffi::JXL_TRUE;
    let animated = have_anim && frames.len() >= 2;
    if !animated {
        return Ok(EngineOutput {
            animated: false,
            width: w,
            height: h,
            frames: vec![std::mem::take(&mut frames[0])],
            delays_ms: vec![0],
            capped: false,
        });
    }

    Ok(EngineOutput {
        animated: true,
        width: w,
        height: h,
        frames,
        delays_ms,
        capped: false,
    })
}

fn check_status(st: ffi::JxlDecoderStatus) -> Result<(), String> {
    if st == ffi::JXL_DEC_SUCCESS {
        Ok(())
    } else {
        Err(format!("JXL 解码失败: {}（status={st}）", status_name(st)))
    }
}
