use std::path::{Path, PathBuf};

fn main() {
    // jxl-libjxl 特性：vendored 构建 libjxl（SIMD + 多线程并行 runner），静态链接进解码通道
    if std::env::var_os("CARGO_FEATURE_JXL_LIBJXL").is_some() {
        build_libjxl();
    }
    tauri_build::build()
}

/// 定位 libjxl 源码：优先环境变量覆盖，否则取 jpegxl-src 源码包内置副本
/// （版本由 Cargo.toml 锁定 =0.12.0，registry/src 下的解包目录唯一确定）
fn libjxl_source() -> PathBuf {
    if let Some(p) = std::env::var_os("Z_VIEWER_LIBJXL_SRC") {
        let p = PathBuf::from(p);
        assert!(
            p.join("lib/CMakeLists.txt").is_file(),
            "Z_VIEWER_LIBJXL_SRC={} 不是 libjxl 源码根目录",
            p.display()
        );
        return p;
    }

    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .expect("无法定位 CARGO_HOME，请设置 Z_VIEWER_LIBJXL_SRC 指向 libjxl 源码根目录");
    let registry_src = cargo_home.join("registry").join("src");
    let pinned = "jpegxl-src-0.12.0";

    if let Ok(hash_dirs) = std::fs::read_dir(&registry_src) {
        for hash in hash_dirs.flatten() {
            let candidate = hash.path().join(pinned).join("libjxl");
            if candidate.join("lib/CMakeLists.txt").is_file() {
                return candidate;
            }
        }
    }
    panic!(
        "registry/src 下未找到 {pinned} 的 libjxl 源码，请先 cargo fetch，\
         或设置 Z_VIEWER_LIBJXL_SRC 指向 libjxl v0.12.0 源码根目录"
    );
}

/// 在 PATH 与常见安装位置定位 clang-cl.exe（可用 CLANG_CL 环境变量显式指定）。
/// MSVC STL 与 clang 版本强耦合（如 VS 18 的 STL 要求 clang ≥ 19），
/// 因此对候选逐个运行 --version 并选最新；全部失败返回 None。
fn find_clang_cl() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLANG_CL") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let clang = dir.join("clang-cl.exe");
            if clang.is_file() {
                candidates.push(clang.clone());
                // 同级多版本安装（如 E:\envs\llvm-18 与 llvm-21 并存）：一并纳入版本比选
                // clang-cl 位于 <root>/<llvm-XX>/bin 下，<root> 是 bin 的祖父目录
                if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                    if let Ok(siblings) = std::fs::read_dir(root) {
                        for sib in siblings.flatten() {
                            let cand = sib.path().join("bin").join("clang-cl.exe");
                            if cand.is_file() {
                                candidates.push(cand);
                            }
                        }
                    }
                }
            }
        }
    }
    for dir in [
        "C:\\Program Files\\LLVM\\bin",
        "C:\\Program Files (x86)\\LLVM\\bin",
    ] {
        candidates.push(PathBuf::from(dir).join("clang-cl.exe"));
    }

    let mut best: Option<(u32, PathBuf)> = None;
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let Some(version) = clang_cl_version(&candidate) else {
            continue;
        };
        if best.as_ref().is_none_or(|(v, _)| version > *v) {
            best = Some((version, candidate));
        }
    }
    best.map(|(_, p)| p)
}

/// 解析 `clang-cl --version` 输出中的主版本号（"clang version 21.1.8" → 21）
fn clang_cl_version(clang: &Path) -> Option<u32> {
    let output = std::process::Command::new(clang)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let idx = text.find("clang version ")?;
    let rest = &text[idx + "clang version ".len()..];
    rest.split('.').next()?.parse().ok()
}

fn build_libjxl() {
    let source = libjxl_source();
    let mut config = cmake::Config::new(source);
    config
        .define("BUILD_TESTING", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("JPEGXL_ENABLE_TOOLS", "OFF")
        .define("JPEGXL_ENABLE_DOXYGEN", "OFF")
        .define("JPEGXL_ENABLE_MANPAGES", "OFF")
        .define("JPEGXL_ENABLE_BENCHMARK", "OFF")
        .define("JPEGXL_ENABLE_EXAMPLES", "OFF")
        .define("JPEGXL_ENABLE_JNI", "OFF")
        .define("JPEGXL_ENABLE_SJPEG", "OFF")
        .define("JPEGXL_ENABLE_OPENEXR", "OFF")
        .define("JPEGXL_BUNDLE_LIBPNG", "OFF")
        // 显式锁定 Release：cmake crate 默认按 OPT_LEVEL 推断构建档，release
        // profile 的 opt-level="s" 会被推断为 MinSizeRel，埋掉 libjxl 的 SIMD
        // 性能（cargo:--config 与 Ninja 单配置均以本定义为准，见 ADR 0001）
        .define("CMAKE_BUILD_TYPE", "Release");

    if let Ok(p) = std::thread::available_parallelism() {
        config.env("CMAKE_BUILD_PARALLEL_LEVEL", format!("{p}"));
    }

    if cfg!(windows) {
        // libjxl 的 SIMD 多目标动态派发在 MSVC cl 下没有 per-file /arch 支持，
        // 解码慢 1-2 个数量级（官方 Windows 包均用 ClangCL）。优先取 clang-cl +
        // Ninja（单配置生成器，CMAKE_BUILD_TYPE 真实生效）；找不到 clang-cl 时
        // 退回 MSVC + 全局 /arch:AVX2（代价：放弃 Haswell 之前的 CPU）。
        // CRT 维持 libjxl 默认的动态 /MD，与 Rust MSVC 工具链一致，免缺省库冲突。
        match find_clang_cl() {
            Some(clang) => {
                config
                    .generator("Ninja")
                    .define("CMAKE_C_COMPILER", &clang)
                    .define("CMAKE_CXX_COMPILER", &clang);
            }
            None => {
                config
                    .define("CMAKE_VS_GLOBALS", "UseMultiToolTask=true;EnforceProcessCountAcrossBuilds=true")
                    .cxxflag("/arch:AVX2");
            }
        }
    }

    let prefix = config.build();

    let mut lib_dir = prefix.join("lib");
    if !lib_dir.exists() {
        let lib64 = prefix.join("lib64");
        assert!(
            lib64.exists(),
            "libjxl 构建产物缺少 lib 目录，请检查 {prefix:?}"
        );
        lib_dir = lib64;
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    println!("cargo:rustc-link-lib=static=jxl");
    println!("cargo:rustc-link-lib=static=jxl_cms");
    println!("cargo:rustc-link-lib=static=jxl_threads");
    println!("cargo:rustc-link-lib=static=hwy");
    println!("cargo:rustc-link-lib=static=brotlidec");
    println!("cargo:rustc-link-lib=static=brotlienc");
    println!("cargo:rustc-link-lib=static=brotlicommon");

    if cfg!(any(target_vendor = "apple", target_os = "freebsd")) {
        println!("cargo:rustc-link-lib=c++");
    } else if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
