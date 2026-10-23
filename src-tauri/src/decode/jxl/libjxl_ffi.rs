//! libjxl v0.12.0 手写 FFI（按官方头文件逐字段转录，只声明解码用到的最小面）
//! 来源头文件：lib/include/jxl/{types,codestream_header,color_encoding,decode,parallel_runner,resizable_parallel_runner}.h
//! 仅在 `jxl-libjxl` 特性下编译；线程安全：每个 JxlDecoder 自身不跨线程共享。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::os::raw::c_int;

pub use std::os::raw::c_void;

pub type JxlDecoder = c_void;
pub type size_t = usize;

// ---------- types.h ----------

pub type JxlDataType = c_int;
pub const JXL_TYPE_FLOAT: JxlDataType = 0;
pub const JXL_TYPE_UINT8: JxlDataType = 2;
pub const JXL_TYPE_UINT16: JxlDataType = 3;
pub const JXL_TYPE_FLOAT16: JxlDataType = 5;

pub type JxlEndianness = c_int;
pub const JXL_NATIVE_ENDIAN: JxlEndianness = 0;
pub const JXL_LITTLE_ENDIAN: JxlEndianness = 1;
pub const JXL_BIG_ENDIAN: JxlEndianness = 2;

/// JXL_BOOL 即 c_int（JXL_TRUE = 1）
pub type JxlBool = c_int;
pub const JXL_TRUE: JxlBool = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlPixelFormat {
    pub num_channels: u32,
    pub data_type: JxlDataType,
    pub endianness: JxlEndianness,
    pub align: size_t,
}

// ---------- codestream_header.h ----------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlPreviewHeader {
    pub xsize: u32,
    pub ysize: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlAnimationHeader {
    pub tps_numerator: u32,
    pub tps_denominator: u32,
    pub num_loops: u32,
    pub have_timecodes: JxlBool,
}

pub type JxlOrientation = c_int;
pub const JXL_ORIENT_IDENTITY: JxlOrientation = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlBasicInfo {
    pub have_container: JxlBool,
    pub xsize: u32,
    pub ysize: u32,
    pub bits_per_sample: u32,
    pub exponent_bits_per_sample: u32,
    pub intensity_target: f32,
    pub min_nits: f32,
    pub relative_to_max_display: JxlBool,
    pub linear_below: f32,
    pub uses_original_profile: JxlBool,
    pub have_preview: JxlBool,
    pub have_animation: JxlBool,
    pub orientation: JxlOrientation,
    pub num_color_channels: u32,
    pub num_extra_channels: u32,
    pub alpha_bits: u32,
    pub alpha_exponent_bits: u32,
    pub alpha_premultiplied: JxlBool,
    pub preview: JxlPreviewHeader,
    pub animation: JxlAnimationHeader,
    pub intrinsic_xsize: u32,
    pub intrinsic_ysize: u32,
    pub padding: [u8; 100],
}

impl JxlBasicInfo {
    pub const fn zeroed() -> Self {
        // padding 内含保留字节，必须整体清零保证 ABI 安全
        unsafe { std::mem::zeroed() }
    }
}

pub type JxlBlendMode = c_int;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlBlendInfo {
    pub blendmode: JxlBlendMode,
    pub source: u32,
    pub alpha: u32,
    pub clamp: JxlBool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlLayerInfo {
    pub have_crop: JxlBool,
    pub crop_x0: i32,
    pub crop_y0: i32,
    pub xsize: u32,
    pub ysize: u32,
    pub blend_info: JxlBlendInfo,
    pub save_as_reference: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlFrameHeader {
    pub duration: u32,
    pub timecode: u32,
    pub name_length: u32,
    pub is_last: JxlBool,
    pub layer_info: JxlLayerInfo,
}

impl JxlFrameHeader {
    pub const fn zeroed() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

// ---------- color_encoding.h ----------

pub type JxlColorSpace = c_int;
pub const JXL_COLOR_SPACE_RGB: JxlColorSpace = 0;
pub const JXL_COLOR_SPACE_GRAY: JxlColorSpace = 1;
pub const JXL_COLOR_SPACE_XYB: JxlColorSpace = 2;
pub const JXL_COLOR_SPACE_UNKNOWN: JxlColorSpace = 3;

pub type JxlWhitePoint = c_int;
pub const JXL_WHITE_POINT_D65: JxlWhitePoint = 1;

pub type JxlPrimaries = c_int;
pub const JXL_PRIMARIES_SRGB: JxlPrimaries = 1;

pub type JxlTransferFunction = c_int;
pub const JXL_TRANSFER_FUNCTION_SRGB: JxlTransferFunction = 13;

pub type JxlRenderingIntent = c_int;
pub const JXL_RENDERING_INTENT_PERCEPTUAL: JxlRenderingIntent = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JxlColorEncoding {
    pub color_space: JxlColorSpace,
    pub white_point: JxlWhitePoint,
    pub white_point_xy: [f64; 2],
    pub primaries: JxlPrimaries,
    pub primaries_red_xy: [f64; 2],
    pub primaries_green_xy: [f64; 2],
    pub primaries_blue_xy: [f64; 2],
    pub transfer_function: JxlTransferFunction,
    pub gamma: f64,
    pub rendering_intent: JxlRenderingIntent,
}

/// sRGB 显示色域（查看器统一输出目标，与 jxl-oxide 通道的 sRGB8 渲染对齐）
pub fn srgb_color_encoding() -> JxlColorEncoding {
    JxlColorEncoding {
        color_space: JXL_COLOR_SPACE_RGB,
        white_point: JXL_WHITE_POINT_D65,
        white_point_xy: [0.0, 0.0],
        primaries: JXL_PRIMARIES_SRGB,
        primaries_red_xy: [0.0, 0.0],
        primaries_green_xy: [0.0, 0.0],
        primaries_blue_xy: [0.0, 0.0],
        transfer_function: JXL_TRANSFER_FUNCTION_SRGB,
        gamma: 0.0,
        rendering_intent: JXL_RENDERING_INTENT_PERCEPTUAL,
    }
}

// ---------- decode.h ----------

pub type JxlDecoderStatus = c_int;
pub const JXL_DEC_SUCCESS: JxlDecoderStatus = 0;
pub const JXL_DEC_ERROR: JxlDecoderStatus = 1;
pub const JXL_DEC_NEED_MORE_INPUT: JxlDecoderStatus = 2;
pub const JXL_DEC_NEED_IMAGE_OUT_BUFFER: JxlDecoderStatus = 5;

pub const JXL_DEC_BASIC_INFO: JxlDecoderStatus = 0x40;
pub const JXL_DEC_COLOR_ENCODING: JxlDecoderStatus = 0x100;
pub const JXL_DEC_FRAME: JxlDecoderStatus = 0x400;
pub const JXL_DEC_FULL_IMAGE: JxlDecoderStatus = 0x1000;

pub type JxlParallelRunInit =
    Option<unsafe extern "C" fn(jpegxl_opaque: *mut c_void, num_threads: size_t) -> c_int>;

pub type JxlParallelRunFunction = Option<
    unsafe extern "C" fn(jpegxl_opaque: *mut c_void, thread: u32, begin: size_t, end: size_t),
>;

/// parallel_runner.h 的 JxlParallelRunner 函数指针（实现方为 resizable runner）
pub type JxlParallelRunner = Option<
    unsafe extern "C" fn(
        runner_opaque: *mut c_void,
        jpegxl_opaque: *mut c_void,
        init: JxlParallelRunInit,
        func: JxlParallelRunFunction,
        start_range: u32,
        end_range: u32,
    ) -> c_int,
>;

extern "C" {
    pub fn JxlDecoderVersion() -> u32;
    pub fn JxlDecoderCreate(memory_manager: *const c_void) -> *mut JxlDecoder;
    pub fn JxlDecoderDestroy(dec: *mut JxlDecoder);
    pub fn JxlDecoderSubscribeEvents(
        dec: *mut JxlDecoder,
        event_mask: u32,
    ) -> JxlDecoderStatus;
    pub fn JxlDecoderSetParallelRunner(
        dec: *mut JxlDecoder,
        parallel_runner: JxlParallelRunner,
        runner_opaque: *mut c_void,
    ) -> JxlDecoderStatus;
    pub fn JxlDecoderSetInput(dec: *mut JxlDecoder, data: *const u8, size: size_t) -> JxlDecoderStatus;
    pub fn JxlDecoderCloseInput(dec: *mut JxlDecoder);
    pub fn JxlDecoderProcessInput(dec: *mut JxlDecoder) -> JxlDecoderStatus;
    pub fn JxlDecoderGetBasicInfo(dec: *const JxlDecoder, info: *mut JxlBasicInfo) -> JxlDecoderStatus;
    /// 请求解码输出色彩档（decode.h `JxlDecoderSetPreferredColorProfile`，2 参）。
    /// 注意区别于 4 参的 `JxlDecoderSetOutputColorProfile(dec, enc, icc, icc_size)`，
    /// 两者不可混转录；仅在 JXL_DEC_COLOR_ENCODING 事件时点可调用。
    pub fn JxlDecoderSetPreferredColorProfile(
        dec: *mut JxlDecoder,
        color_encoding: *const JxlColorEncoding,
    ) -> JxlDecoderStatus;
    pub fn JxlDecoderGetFrameHeader(dec: *const JxlDecoder, info: *mut JxlFrameHeader) -> JxlDecoderStatus;
    pub fn JxlDecoderSetImageOutBuffer(
        dec: *mut JxlDecoder,
        format: *const JxlPixelFormat,
        buffer: *mut u8,
        size: size_t,
    ) -> JxlDecoderStatus;
}

// ---------- resizable_parallel_runner.h ----------

extern "C" {
    pub fn JxlResizableParallelRunner(
        runner_opaque: *mut c_void,
        jpegxl_opaque: *mut c_void,
        init: JxlParallelRunInit,
        func: JxlParallelRunFunction,
        start_range: u32,
        end_range: u32,
    ) -> c_int;
    pub fn JxlResizableParallelRunnerCreate(memory_manager: *const c_void) -> *mut c_void;
    pub fn JxlResizableParallelRunnerSetThreads(runner_opaque: *mut c_void, num_threads: size_t);
    pub fn JxlResizableParallelRunnerDestroy(runner_opaque: *mut c_void);
}

// ---------- encode.h（仅供测试基准生成解码素材，不参与产品功能）----------

pub type JxlEncoder = c_void;
pub type JxlEncoderFrameSettings = c_void;
pub type JxlEncoderStatus = c_int;
pub const JXL_ENC_SUCCESS: JxlEncoderStatus = 0;
pub const JXL_ENC_NEED_MORE_OUTPUT: JxlEncoderStatus = 2;

/// JxlEncoderFrameSettingId::JXL_ENC_FRAME_SETTING_EFFORT
pub const JXL_ENC_FRAME_SETTING_EFFORT: c_int = 0;

#[cfg(test)]
extern "C" {
    pub fn JxlEncoderCreate(memory_manager: *const c_void) -> *mut JxlEncoder;
    pub fn JxlEncoderDestroy(enc: *mut JxlEncoder);
    pub fn JxlEncoderFrameSettingsSetOption(
        frame_settings: *mut JxlEncoderFrameSettings,
        option: c_int,
        value: i32,
    ) -> JxlEncoderStatus;
    pub fn JxlEncoderSetBasicInfo(enc: *mut JxlEncoder, info: *const JxlBasicInfo) -> JxlEncoderStatus;
    pub fn JxlEncoderSetColorEncoding(
        enc: *mut JxlEncoder,
        color_encoding: *const JxlColorEncoding,
    ) -> JxlEncoderStatus;
    pub fn JxlEncoderFrameSettingsCreate(
        enc: *mut JxlEncoder,
        frame_settings: *const JxlEncoderFrameSettings,
    ) -> *mut JxlEncoderFrameSettings;
    pub fn JxlEncoderSetFrameDistance(
        frame_settings: *mut JxlEncoderFrameSettings,
        distance: f32,
    ) -> JxlEncoderStatus;
    pub fn JxlEncoderAddImageFrame(
        frame_settings: *mut JxlEncoderFrameSettings,
        pixel_format: *const JxlPixelFormat,
        buffer: *const u8,
        size: size_t,
    ) -> JxlEncoderStatus;
    pub fn JxlEncoderProcessOutput(
        enc: *mut JxlEncoder,
        next_out: *mut *mut u8,
        avail_out: *mut size_t,
    ) -> JxlEncoderStatus;
    /// 关闭输入流（缓冲式 ProcessOutput API 的收尾信号；FlushInput 仅用于回调式输出）
    pub fn JxlEncoderCloseInput(enc: *mut JxlEncoder);
}
