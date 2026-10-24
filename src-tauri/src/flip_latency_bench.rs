//! 翻页延迟基准（票 02）：免费版（缓存关）vs 专业版（等级 1/2）的 Rust 侧翻页耗时对比。
//!
//! 运行（release 构建下数字才有效；libjxl 恒为 Release 构建，jxl-oxide dev 下已 O3）：
//! `cargo test --release --ignored flip_latency -- --nocapture`
//!
//! 链路与生产一致：`Navigator::navigate`（真实代次/提升/双队列预取派生，预取经
//! tauri 全局 runtime 后台解码）→ demand 路径（缓存命中取信封 / `decode::load_image`
//! 冷解码 + 写缓存）。测量含信封打包与 IPC 拷贝，不含前端渲染与传输。
//! 素材现场生成于系统临时目录（JXL 经 vendored libjxl 编码器 effort 3、TIFF 经
//! image crate 无压缩 RGB）；RAW 需真实机内文件无法合成，见票 02 的 Answer。
//! 数字解读与决策用途见 `.scratch/pm-followup-20261024/issues/02`。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::browse::BrowseModel;
use crate::cache::{DecodeCache, FolderFirstCache};
use crate::commands::AppSettings;
use crate::navigator::{NavStep, Navigator, PrefetchPolicy};

/// 每个素材文件夹文件数（一次往返 = 2N-2 次翻页 + 首图 = 2N-1 个样本）
const N_FILES: usize = 8;
/// 正常浏览节奏的翻页间隔（给后台预取消化窗口；快速连翻场景不等待）
const LOOK: Duration = Duration::from_millis(500);
/// JXL 素材：6000×4000（24MP 主流全画幅档位）
const JXL_W: u32 = 6000;
const JXL_H: u32 = 4000;
/// TIFF 素材：4000×3000（12MP，无压缩 RGB）
const TIFF_W: u32 = 4000;
const TIFF_H: u32 = 3000;

/// 确定性伪噪声（渐变底 + 噪声）：非平坦内容，避免压缩/解码走捷径
fn noise(x: u32, y: u32, salt: u32) -> u8 {
    (((x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519))
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9)))
        & 0x3f) as u8
}

/// 临时素材目录（Drop 时清理）
struct TempDir(PathBuf);

impl TempDir {
    fn create(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("zviewer-flip-bench-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建基准素材目录");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn gen_jxl(dir: &PathBuf) {
    use crate::decode::bench_util::encode_jxl_fixture;
    for i in 0..N_FILES {
        let mut rgba = vec![0u8; JXL_W as usize * JXL_H as usize * 4];
        for y in 0..JXL_H {
            for x in 0..JXL_W {
                let o = (y as usize * JXL_W as usize + x as usize) * 4;
                let n = noise(x, y, i as u32);
                rgba[o] = ((x * 255 / JXL_W) as u8).wrapping_add(n);
                rgba[o + 1] = ((y * 255 / JXL_H) as u8).wrapping_add(n);
                rgba[o + 2] = (((x + y) & 0xff) as u8).wrapping_add(n);
                rgba[o + 3] = 255;
            }
        }
        let bytes = encode_jxl_fixture(&rgba, JXL_W, JXL_H);
        std::fs::write(dir.join(format!("{i:03}.jxl")), bytes).expect("写入 JXL 素材");
    }
}

fn gen_tiff(dir: &PathBuf) {
    for i in 0..N_FILES {
        let img = image::RgbImage::from_fn(TIFF_W, TIFF_H, |x, y| {
            let n = noise(x, y, i as u32);
            image::Rgb([
                ((x * 255 / TIFF_W) as u8).wrapping_add(n),
                ((y * 255 / TIFF_H) as u8).wrapping_add(n),
                (((x + y) & 0xff) as u8).wrapping_add(n),
            ])
        });
        img.save_with_format(dir.join(format!("{i:03}.tif")), image::ImageFormat::Tiff)
            .expect("写入 TIFF 素材");
    }
}

#[derive(Default)]
struct Run {
    n: usize,
    hits: usize,
    hit_ms: Vec<f64>,
    cold_ms: Vec<f64>,
    all_ms: Vec<f64>,
    nav_max_ms: f64,
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = (p / 100.0 * (v.len() - 1) as f64).round() as usize;
    v[idx]
}

fn ms_since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// demand 路径：`commands::load_image` 的缓存/解码/写回语义（去 IPC 壳，保留信封成本）
fn demand_load(cache: &DecodeCache, path: &str, caching: bool) -> (f64, bool) {
    let t0 = Instant::now();
    if caching {
        if let Some(hit) = cache.get_entry(path) {
            let envelope = hit.envelope();
            let _response = (*envelope).clone(); // 命中路径的 IPC 拷贝成本
            return (ms_since(t0), true);
        }
    }
    let result = crate::decode::load_image(path, false)
        .map(Arc::new)
        .expect("基准素材应可解码");
    if caching && crate::cache_policy::should_cache(&result) {
        cache.put(path.to_string(), Arc::clone(&result));
        cache.end_prefetch(path);
    }
    let _response = crate::decode::pack_envelope(&result); // 冷路径同样要打包
    (ms_since(t0), false)
}

fn record(run: &mut Run, ms: f64, hit: bool) {
    run.n += 1;
    run.all_ms.push(ms);
    if hit {
        run.hits += 1;
        run.hit_ms.push(ms);
    } else {
        run.cold_ms.push(ms);
    }
}

/// 一次往返扫描：open → 首图 → next×(N-1) → prev×(N-1)。
/// `look = Some(..)` 正常浏览节奏（预取有消化窗口），None 快速连翻（最坏情况）。
fn run_scan(dir: &PathBuf, level: usize, look: Option<Duration>) -> Run {
    let cache = DecodeCache::new(0);
    let first_cache = FolderFirstCache::new(0);
    let settings = AppSettings {
        cache_level: level,
        folder_first_depth: 1,
    };
    let policy = PrefetchPolicy::from(&settings);
    let caching = policy.enabled;
    if caching {
        // 与 set_cache_level 同参：level1 容量 4、level2 容量 8，首图队列 4
        cache.set_capacity(if level >= 2 { 8 } else { 4 });
        first_cache.set_capacity(4);
    }
    let model = BrowseModel::open_first_in_dir_gated(dir, None, true).expect("打开素材文件夹");
    let nav = Navigator::new(cache.clone(), first_cache.clone());
    if caching {
        nav.did_open(&model, &policy);
    }

    let mut run = Run::default();
    // 首图：open 后前端立即 demand load
    let path = model.state().path;
    let (t, hit) = demand_load(&cache, &path, caching);
    record(&mut run, t, hit);
    for _ in 0..N_FILES - 1 {
        flip_step(
            &nav,
            &model,
            &cache,
            &policy,
            caching,
            look,
            &mut run,
            NavStep::Next,
        );
    }
    for _ in 0..N_FILES - 1 {
        flip_step(
            &nav,
            &model,
            &cache,
            &policy,
            caching,
            look,
            &mut run,
            NavStep::Prev,
        );
    }
    run
}

#[allow(clippy::too_many_arguments)]
fn flip_step(
    nav: &Navigator,
    model: &BrowseModel,
    cache: &DecodeCache,
    policy: &PrefetchPolicy,
    caching: bool,
    look: Option<Duration>,
    run: &mut Run,
    step: NavStep,
) {
    let t0 = Instant::now();
    let r = nav.navigate(model, step, policy);
    run.nav_max_ms = run.nav_max_ms.max(ms_since(t0));
    if let Some(st) = r.state {
        let (t, hit) = demand_load(cache, &st.path, caching);
        record(run, t, hit);
    }
    if let Some(l) = look {
        std::thread::sleep(l);
    }
}

fn report(label: &str, run: &Run) {
    let hit_pct = if run.n > 0 {
        run.hits as f64 / run.n as f64 * 100.0
    } else {
        0.0
    };
    println!(
        "  {label:<18} n={:>2} 命中{:>4.0}%  冷p50={:>7.1}ms  命中p50={:>5.1}ms  总p95={:>7.1}ms  max={:>7.1}ms  navmax={:>4.1}ms",
        run.n,
        hit_pct,
        percentile(run.cold_ms.clone(), 50.0),
        percentile(run.hit_ms.clone(), 50.0),
        percentile(run.all_ms.clone(), 95.0),
        percentile(run.all_ms.clone(), 100.0),
        run.nav_max_ms,
    );
}

fn run_format(label: &str, dir: &PathBuf) {
    println!("== {label} ×{N_FILES}（往返扫描） ==");
    report("level 0 免费(冷)", &run_scan(dir, 0, None));
    report("level 1 正常节奏", &run_scan(dir, 1, Some(LOOK)));
    report("level 1 快速连翻", &run_scan(dir, 1, None));
    report("level 2 正常节奏", &run_scan(dir, 2, Some(LOOK)));
    report("level 2 快速连翻", &run_scan(dir, 2, None));
}

#[test]
#[ignore = "手动基准：cargo test --release --ignored flip_latency -- --nocapture"]
fn flip_latency_bench() {
    println!(
        "翻页延迟基准：免费 level0 vs 专业 level1/2；LOOK={LOOK:?}；每档 {N_FILES} 张往返扫描"
    );

    #[cfg(feature = "jxl-libjxl")]
    {
        let dir = TempDir::create("jxl");
        let t0 = Instant::now();
        gen_jxl(&dir.0);
        println!(
            "[setup] JXL {JXL_W}×{JXL_H} ×{N_FILES} 生成于 {:.1}s",
            t0.elapsed().as_secs_f64()
        );
        run_format(&format!("JXL {JXL_W}×{JXL_H}（24MP）"), &dir.0);
    }

    let dir = TempDir::create("tiff");
    let t0 = Instant::now();
    gen_tiff(&dir.0);
    println!(
        "[setup] TIFF {TIFF_W}×{TIFF_H} ×{N_FILES} 生成于 {:.1}s",
        t0.elapsed().as_secs_f64()
    );
    run_format(&format!("TIFF {TIFF_W}×{TIFF_H}（12MP 无压缩）"), &dir.0);
}
