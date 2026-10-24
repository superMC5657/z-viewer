//! 深导航模块：导航 + 预取 + 代次协议的唯一实现（深模块，窄接口）。
//!
//! 外部接口 3 个方法 + 1 个策略结构体 + 2 个失活阈值常量（全部 infallible，无 Result；
//! 预取失败 log-and-drop，只记 basename）：
//! - [`PrefetchPolicy`]：预取策略快照（`From<&AppSettings>` 5 行薄映射）
//! - [`Navigator::ABORT_LAG`] / [`Navigator::SKIP_PUT_LAG`]：失活/写盘滞后阈值
//!   （接口可见的调谐常量；调用方只读不推导，派生细节收归 `is_aborted` /
//!   `should_skip_put` 语义谓词）
//! - [`Navigator::navigate`]：next/prev/jump_folder（推进代次 + 首图提升 + 双队列派生）
//! - [`Navigator::did_open`]：open_path 尾巴（推进代次 + 双队列派生，无提升）
//! - [`Navigator::frontend_context`]：get_context 只读投影（无代次、无派生，含 asset）
//!
//! 固定内部顺序（调用方不可重排）：导航产出 `Some(state)` 时恰好推进一次代次
//! （Boundary/无模型/缓存关闭时零推进）→ 提升首图（队列 A→B）→ 派生上下文
//! 队列 B → 派生首图队列 A。[`Navigator::did_open`] 相同但无提升。
//! 代次递增先于一切派生，任务捕获的是导航后代次。
//!
//! 缝背后的实现（locality：调用方不可见）：
//! - 代次时钟归本模块所有（`generation: Arc<AtomicUsize>`，`Clone` 共享同一时钟；
//!   命令层每次现造 Navigator，共享格取自同一被管 [`DecodeCache`] 句柄，跨导航
//!   失活协议不断）。生产推进只经 [`Navigator::navigate`] / [`Navigator::did_open`]，
//!   `current/advance` 仅 pub(crate) 供测试驱动；无泛型/trait 污染公开接口。
//! - [`DecodeCache`] 不再自持时钟：新鲜度判定对照传入的捕获代次在本模块完成
//!   （`is_aborted` / `should_skip_put`），缓存侧只上报命中/失效（peek/get/put）。
//!   [`DecodeCache::put`]
//!   内部保留 animated 单槽 vs LRU 分流（本模块盲传 [`LoadResult`]）。
//! - 预取纪律：peek（不碰 LRU）vs get_entry（-demand 触碰）；解码后紧邻 put
//!   再做一次 peek 复查；begin_prefetch→end_prefetch 每条退出路径配对
//!   （失活丢弃/解码失败/规则跳过/首图缺失）；过滤只经 crate::cache_policy
//!   （should_prefetch 预过滤 + should_cache 写过滤），此处不复述规则。
//!
//! 调用方只传快照：`&BrowseModel`（已快照的 Arc，绝不在内部持 AppState 锁——
//! Condvar 等待 locality 不变）+ `&PrefetchPolicy`（AppSettings 快照）。
//! Tauri State/Mutex、license pro 门控、日志、NavResult 边缘映射留在命令层。
//!
//! 词汇：module / interface / implementation / depth / seam / adapter /
//! leverage / locality 见实现注释。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget, Nav};
use crate::cache::{DecodeCache, FolderFirst, FolderFirstCache};
use crate::commands::AppSettings;
use crate::decode::LoadResult;
use crate::util::{basename_of, first_line_of};

/// 深模块：持有克隆的 [`DecodeCache`] + [`FolderFirstCache`] 句柄，
/// 集中导航 + 预取 + 代次协议。Clone 仅复制句柄（同一缓存、同一代次时钟）。
#[derive(Clone)]
pub struct Navigator {
    cache: DecodeCache,
    first_cache: FolderFirstCache,
    /// 新鲜度时钟：归推进它的本模块所有（共享格，见 [`DecodeCache::generation_cell`]）。
    generation: Arc<AtomicUsize>,
    decoder: Decoder,
    scanner: FolderScanner,
}

impl Navigator {
    /// 失活滞后：后台任务落后当前代次超过 2 代直接丢弃（模块内部调谐常量）。
    const ABORT_LAG: usize = 2;
    /// 写盘滞后：解码完成后落后超过 3 代则跳过 put（模块内部调谐常量）。
    const SKIP_PUT_LAG: usize = 3;
}

/// 预取策略快照：`From<&AppSettings>` 薄映射（level 0/1/2 经 neighbor_window +
/// folder_first_depth，folder_depth 钳制 ≤3）。
#[derive(Clone, Copy, Debug)]
pub struct PrefetchPolicy {
    /// 缓存总开关（cache_level > 0）。
    pub enabled: bool,
    /// 向前预取窗口。
    pub prev: usize,
    /// 向后预取窗口。
    pub next: usize,
    /// 首图队列深度（已钳制 ≤3）。
    pub folder_depth: usize,
    /// 是否派生上下文队列 B。
    pub decode_paths: bool,
    /// 是否派生首图队列 A。
    pub folder_firsts: bool,
}

impl From<&AppSettings> for PrefetchPolicy {
    fn from(s: &AppSettings) -> Self {
        let (prev, next) = s.neighbor_window();
        let folder_depth = s.folder_first_depth.min(3);
        let enabled = s.is_enabled();
        Self {
            enabled,
            prev,
            next,
            folder_depth,
            decode_paths: enabled && prev + next > 0,
            folder_firsts: enabled && folder_depth > 0,
        }
    }
}

/// 导航步进：next/prev/文件夹跳转（深接口的统一输入，命令层只做目标解析）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavStep {
    Next,
    Prev,
    Folder(FolderTarget),
}

/// 导航结果：boundary 为 Some 时表示撞到全局边界（前端弹 Toast），state 不变。
/// boundary 取值："first-image" | "last-image" | "first-folder" | "last-folder" | "pro-required"
#[derive(serde::Serialize)]
pub struct NavResult {
    pub boundary: Option<String>,
    pub state: Option<BrowseState>,
}

impl NavResult {
    pub(crate) fn ok(state: BrowseState) -> Self {
        Self {
            boundary: None,
            state: Some(state),
        }
    }
    pub(crate) fn boundary(b: Boundary, state: BrowseState) -> Self {
        let msg = match b {
            Boundary::FirstImage => "first-image",
            Boundary::LastImage => "last-image",
            Boundary::FirstFolder => "first-folder",
            Boundary::LastFolder => "last-folder",
        };
        Self {
            boundary: Some(msg.into()),
            state: Some(state),
        }
    }
    /// 专业版功能锁定（免费版调用 jump_folder）：前端弹出解锁引导。
    pub fn pro_required() -> Self {
        Self {
            boundary: Some("pro-required".into()),
            state: None,
        }
    }
    pub(crate) fn none() -> Self {
        Self {
            boundary: None,
            state: None,
        }
    }
}

/// 解码器 seam（pub(crate) 上限，生产默认 [`crate::decode::load_image`]；
/// 无泛型/trait bound 污染 [`Navigator::new`]）。
pub(crate) type Decoder = fn(&str, bool) -> Result<LoadResult, String>;
/// 文件夹扫描 seam（pub(crate) 上限，生产默认 [`BrowseModel::first_image_of`]）。
pub(crate) type FolderScanner = fn(&Path) -> Option<PathBuf>;

impl Navigator {
    /// 生产构造：默认 seam（解码器 + 扫描器），调用方传克隆句柄即可。
    /// 代次时钟取自缓存句柄的共享格：同一被管缓存克隆出的 Navigator 共享同一时钟。
    pub fn new(cache: DecodeCache, first_cache: FolderFirstCache) -> Self {
        let generation = cache.generation_cell();
        Self {
            cache,
            first_cache,
            generation,
            decoder: crate::decode::load_image,
            scanner: BrowseModel::first_image_of,
        }
    }

    /// 测试 seam 注入（pub(crate) 上限）：自定义解码器/扫描器，
    /// 时钟归本模块所有，测试经 `advance_generation` 手动驱动。
    #[cfg(test)]
    pub(crate) fn with_seams(
        cache: DecodeCache,
        first_cache: FolderFirstCache,
        decoder: Decoder,
        scanner: FolderScanner,
    ) -> Self {
        let generation = cache.generation_cell();
        Self {
            cache,
            first_cache,
            generation,
            decoder,
            scanner,
        }
    }

    /// 当前代次（私有：生产推进只经 navigate/did_open；测试经此读取）。
    fn current_generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    /// 推进代次（私有：生产调用方一律走 navigate/did_open；
    /// 测试手动驱动模拟快速连续导航）。
    fn advance_generation(&self) -> usize {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 失活判定：捕获代次落后当前超过 max_lag（对照传入的代次上报，
    /// 阈值语义见 [`Navigator::ABORT_LAG`] / [`Navigator::SKIP_PUT_LAG`]）。
    fn is_stale(&self, gen: usize, max_lag: usize) -> bool {
        self.current_generation().saturating_sub(gen) > max_lag
    }

    /// 后台任务是否应直接丢弃（落后超过 [`Navigator::ABORT_LAG`] 代）。
    fn is_aborted(&self, gen: usize) -> bool {
        self.is_stale(gen, Self::ABORT_LAG)
    }

    /// 解码完成后是否应跳过 put（落后超过 [`Navigator::SKIP_PUT_LAG`] 代）。
    fn should_skip_put(&self, gen: usize) -> bool {
        self.is_stale(gen, Self::SKIP_PUT_LAG)
    }

    /// 导航：next/prev/jump_folder + 后置协议（代次→提升→队列 B→队列 A）。
    /// infallible：Boundary 时零代次推进、零提升、零派生，直接返回边界结果；
    /// 缓存关闭（!enabled）同样零推进（队列已清空，代次冻结）。
    pub fn navigate(
        &self,
        model: &BrowseModel,
        step: NavStep,
        policy: &PrefetchPolicy,
    ) -> NavResult {
        let nav = match step {
            NavStep::Next => model.next(),
            NavStep::Prev => model.prev(),
            NavStep::Folder(t) => model.jump_folder(t),
        };
        let ok = matches!(nav, Nav::Ok);
        let result = match nav {
            Nav::Ok => NavResult::ok(model.state()),
            Nav::Boundary(b) => NavResult::boundary(b, model.state()),
        };
        if !ok || !policy.enabled {
            return result;
        }
        // 固定顺序：代次递增恰好一次 → 提升 → 队列 B → 队列 A。
        self.advance_generation();
        self.promote_folder_first(model);
        self.spawn_context(model, policy);
        self.spawn_folder_firsts(model, policy);
        result
    }

    /// 打开尾巴：open_path 成功后调用（推进代次 + 双队列派生，无提升——
    /// 新模型无可提升的首图）。infallible；缓存关闭时空操作。
    pub fn did_open(&self, model: &BrowseModel, policy: &PrefetchPolicy) {
        if !policy.enabled {
            return;
        }
        self.advance_generation();
        self.spawn_context(model, policy);
        self.spawn_folder_firsts(model, policy);
    }

    /// 前端上下文：只读投影（无代次、无派生），返回 asset 供前端池预热
    /// （预取队列跳过 asset，此处保留——同一投影的两面）。
    /// 同步实现：调用方（async 命令边缘）负责 spawn_blocking 包装。
    pub fn frontend_context(&self, model: &BrowseModel, policy: &PrefetchPolicy) -> Vec<String> {
        let mut paths: Vec<String> = model
            .context_paths(policy.prev, policy.next)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        if policy.folder_firsts {
            for folder in model.neighbor_folders(policy.folder_depth) {
                if let Some(first) = (self.scanner)(&folder) {
                    let s = first.to_string_lossy().to_string();
                    if !paths.contains(&s) {
                        paths.push(s);
                    }
                }
            }
        }
        paths
    }

    /// 提升：导航进入新文件夹时把队列 A 命中的首图并入队列 B
    /// （前端 load_image 按图片路径查队列 B，命中即零解码）。
    /// 首图路径随缓存值保存，直接读取无需重复文件系统枚举；
    /// 写过滤同样经 should_cache（队列 A 照例只含可缓存条目，此处为守卫）。
    fn promote_folder_first(&self, model: &BrowseModel) {
        let folder = model.current_folder_path();
        let Some(first) = self.first_cache.get(&folder.to_string_lossy()) else {
            return;
        };
        if !crate::cache_policy::should_cache(&first.result) {
            return;
        }
        self.cache.put(first.path.clone(), first.result.clone());
    }

    /// 派生上下文队列 B：当前文件夹前后邻居预取（decode_paths 门控）。
    /// 预取规则只经 should_prefetch（asset 由前端池预热）；失败静默忽略。
    fn spawn_context(&self, model: &BrowseModel, policy: &PrefetchPolicy) {
        if !policy.decode_paths {
            return;
        }
        for p in model.context_paths(policy.prev, policy.next) {
            let path = p.to_string_lossy().to_string();
            if !crate::cache_policy::should_prefetch(&path) {
                continue;
            }
            if !self.cache.begin_prefetch(&path) {
                continue;
            }
            // 代次在导航后已递增：任务捕获导航后代次。
            let gen = self.current_generation();
            let this = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                this.prefetch_one(&path, gen);
            });
        }
    }

    /// 派生首图队列 A：相邻文件夹第一张图预取（跨文件夹跳转无延迟）。
    /// 目录枚举移入后台线程，不阻塞导航命令。
    fn spawn_folder_firsts(&self, model: &BrowseModel, policy: &PrefetchPolicy) {
        if !policy.folder_firsts {
            return;
        }
        // 锁内仅收集邻居文件夹路径（快速克隆），枚举与解码全部移入后台。
        let folders = model.neighbor_folders(policy.folder_depth);
        for folder in folders {
            let folder_str = folder.to_string_lossy().to_string();
            if self.first_cache.peek(&folder_str) {
                continue;
            }
            // 并发门控 + 去重登记（key 用文件夹路径，与图片路径登记互不冲突）。
            if !self.cache.begin_prefetch(&folder_str) {
                continue;
            }
            let gen = self.current_generation();
            let this = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                this.prefetch_first(folder, folder_str, gen);
            });
        }
    }

    /// 单路径预取核心（同步）：spawn 闭包与测试共用同一实现。
    /// 前提：调用方已 `begin_prefetch` 登记；本函数保证每条退出路径
    /// 配对 `end_prefetch`（失活丢弃/解码失败/规则跳过）。
    /// 学到的经验：peek 复查紧邻 put（实时加载可能已填充，跳过覆盖）。
    /// 私有：测试经 `super::*` 可见；外部调用方一律走派生循环（协议不可绕过）。
    fn prefetch_one(&self, path: &str, gen: usize) {
        // 用户快速切图已远离当前上下文：直接放弃，不占 CPU。
        if self.is_aborted(gen) {
            self.cache.end_prefetch(path);
            return;
        }
        // 规则前置（派生循环已预过滤，此处让核心自包含可独立测试）。
        if !crate::cache_policy::should_prefetch(path) {
            self.cache.end_prefetch(path);
            return;
        }
        let result = match (self.decoder)(path, true) {
            Ok(r) => r,
            // 后台预取失败：低价值，只 debug，记 basename + 原因首行。
            Err(e) => {
                log::debug!(
                    "cache prefetch failed (ctx) {}: {}",
                    basename_of(path),
                    first_line_of(&e)
                );
                self.cache.end_prefetch(path);
                return;
            }
        };
        // 写过滤经 should_cache：单帧降级与内嵌预览不占缓存槽位。
        if !crate::cache_policy::should_cache(&result) {
            self.cache.end_prefetch(path);
            return;
        }
        if !self.cache.peek(path) && !self.should_skip_put(gen) {
            self.cache.put(path.to_string(), Arc::new(result));
        }
        self.cache.end_prefetch(path);
    }

    /// 单文件夹首图预取核心（同步）：spawn 闭包与测试共用同一实现。
    /// 每条退出路径配对 `end_prefetch`（失活/首图缺失/asset 跳过/解码失败）。
    /// 私有：同 prefetch_one，协议不可绕过。
    fn prefetch_first(&self, folder: PathBuf, folder_str: String, gen: usize) {
        if self.is_aborted(gen) {
            self.cache.end_prefetch(&folder_str);
            return;
        }
        // 取该文件夹第一张图（后台线程，不阻塞导航命令）。
        let Some(first) = (self.scanner)(&folder) else {
            self.cache.end_prefetch(&folder_str);
            return;
        };
        let path = first.to_string_lossy().to_string();
        if !crate::cache_policy::should_prefetch(&path) {
            self.cache.end_prefetch(&folder_str);
            return; // asset 由前端池预热。
        }
        if !self.is_aborted(gen) {
            match (self.decoder)(&path, true) {
                Ok(result) => {
                    if crate::cache_policy::should_cache(&result) {
                        self.first_cache.put(
                            folder_str.clone(),
                            Arc::new(FolderFirst {
                                path,
                                result: Arc::new(result),
                            }),
                        );
                    }
                }
                // 首图预取失败：低价值，只 debug，记 basename + 原因首行。
                Err(e) => {
                    log::debug!(
                        "cache prefetch failed (first) {}: {}",
                        basename_of(&path),
                        first_line_of(&e)
                    );
                }
            }
        }
        self.cache.end_prefetch(&folder_str);
    }
}

#[cfg(test)]
mod tests {
    //! Navigator 接口测试（seam 表面，只断言可观测结果；内部重构存活）。
    //! 用注入的假解码器绕过 spawn_blocking（同步调用核心），无时序 flake。

    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_base() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("z-viewer-nav-test-{}-{n}", std::process::id()))
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn cleanup(base: &Path) {
        let _ = fs::remove_dir_all(base);
    }

    /// 树：A{a1,a2}.png、B{b1,b2}.jpg（全 asset 扩展名：预取跳过解码，
    /// 前端上下文保留——测 (d) 的天然材料）。
    fn build_tree() -> PathBuf {
        let base = temp_base();
        for n in ["a1", "a2"] {
            touch(&base.join("A").join(format!("{n}.png")));
        }
        for n in ["b1", "b2"] {
            touch(&base.join("B").join(format!("{n}.jpg")));
        }
        base
    }

    fn open_sync(path: &Path) -> BrowseModel {
        let m = BrowseModel::open(path, None).unwrap();
        m.wait_ready();
        m
    }

    fn enabled_policy() -> PrefetchPolicy {
        PrefetchPolicy::from(&AppSettings::default())
    }

    fn closed_policy() -> PrefetchPolicy {
        PrefetchPolicy::from(&AppSettings {
            cache_level: 0,
            folder_first_depth: 1,
        })
    }

    /// 假解码器：恒返回非 asset 全量结果（不断言内容，只看是否入缓存）。
    fn fake_decoder(_path: &str, _full: bool) -> Result<LoadResult, String> {
        Ok(LoadResult::jpeg("raw", b"fake".to_vec(), 10, 10, false))
    }

    /// 恒失败解码器：走解码失败路径。
    fn failing_decoder(path: &str, _full: bool) -> Result<LoadResult, String> {
        Err(format!("boom {path}"))
    }

    fn test_nav(cache: DecodeCache, first_cache: FolderFirstCache, decoder: Decoder) -> Navigator {
        Navigator::with_seams(cache, first_cache, decoder, BrowseModel::first_image_of)
    }

    /// (a) navigate 在 Ok 时恰好推进一次代次，Boundary 时零推进。
    #[test]
    fn navigate_generation_once_on_ok_zero_on_boundary() {
        let base = build_tree();
        let model = open_sync(&base.join("A/a1.png"));
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);
        let nav = test_nav(cache.clone(), first_cache.clone(), fake_decoder);
        let policy = enabled_policy();

        assert_eq!(nav.current_generation(), 0);
        let r = nav.navigate(&model, NavStep::Next, &policy);
        assert!(r.boundary.is_none(), "A 内翻页应 Ok");
        assert_eq!(nav.current_generation(), 1, "Ok 恰好推进一次代次");

        // 走到全局末尾再 next → Boundary，代次不动。
        let r = nav.navigate(&model, NavStep::Next, &policy); // a2 → b1
        assert!(r.boundary.is_none());
        assert_eq!(nav.current_generation(), 2);
        let r = nav.navigate(&model, NavStep::Next, &policy); // b1 → b2
        assert!(r.boundary.is_none());
        let r = nav.navigate(&model, NavStep::Next, &policy); // b2 → 撞尾
        assert_eq!(r.boundary.as_deref(), Some("last-image"));
        assert_eq!(
            nav.current_generation(),
            3,
            "Boundary 零推进（之前 3 次 Ok 各 +1）"
        );

        // 缓存关闭：Ok 也不推进，且不提升（预置首图不得混入队列 B）。
        let b_key = base.join("B").to_string_lossy().to_string();
        let seeded = base.join("B").join("b1.jpg").to_string_lossy().to_string();
        first_cache.put(
            b_key,
            Arc::new(FolderFirst {
                path: seeded.clone(),
                result: Arc::new(fake_decoder(&seeded, true).unwrap()),
            }),
        );
        let r = nav.navigate(&model, NavStep::Prev, &closed_policy());
        assert!(r.boundary.is_none());
        assert_eq!(nav.current_generation(), 3, "缓存关闭时 Ok 也不推进代次");
        assert!(
            cache.get_entry(&seeded).is_none(),
            "缓存关闭时不得提升（防无条件 promote 回归）"
        );
        cleanup(&base);
    }

    /// (b) 第二次导航使第一代任务失活：旧代次同步核心跳过 put。
    #[test]
    fn stale_generation_skips_put() {
        let cache = DecodeCache::new(8);
        let nav = test_nav(cache.clone(), FolderFirstCache::new(4), fake_decoder);

        // 新鲜代次：同步核心正常 put（对照组）。
        let fresh = nav.current_generation();
        assert!(cache.begin_prefetch("fresh.raw"));
        nav.prefetch_one("fresh.raw", fresh);
        assert!(
            cache.get_entry("fresh.raw").is_some(),
            "新鲜代次应正常入缓存"
        );

        // 模拟快速连续导航：代次大幅前移后，旧代任务失活丢弃。
        let old = nav.current_generation();
        for _ in 0..10 {
            nav.advance_generation();
        }
        assert!(cache.begin_prefetch("stale.raw"));
        nav.prefetch_one("stale.raw", old);
        assert!(
            cache.get_entry("stale.raw").is_none(),
            "失活代次应跳过 put（第二次导航取消第一代）"
        );
        // 配对检查：登记已释放，可重新登记。
        assert!(
            cache.begin_prefetch("stale.raw"),
            "失活丢弃路径必须配对 end_prefetch"
        );
        cache.end_prefetch("stale.raw");
    }

    /// 时钟语义（由 cache/tests.rs 随重构迁入，行为断言不变）：lag 边界恰为
    /// 接口阈值常量；判定经语义谓词，调用方不推导滞后步数。
    #[test]
    fn generation_clock_thresholds() {
        let cache = DecodeCache::new(4);
        let nav = test_nav(cache, FolderFirstCache::new(4), fake_decoder);
        assert_eq!(Navigator::ABORT_LAG, 2);
        assert_eq!(Navigator::SKIP_PUT_LAG, 3);
        assert_eq!(nav.current_generation(), 0);
        assert!(!nav.is_aborted(0));
        assert!(!nav.should_skip_put(0));

        nav.advance_generation(); // 代次 = 1
        assert!(!nav.is_aborted(0));
        assert!(!nav.should_skip_put(0));

        nav.advance_generation(); // 代次 = 2
        assert!(!nav.is_aborted(0), "lag=2 未超过 ABORT_LAG");
        assert!(!nav.should_skip_put(0));

        nav.advance_generation(); // 代次 = 3
        assert!(nav.is_aborted(0), "超过 ABORT_LAG 视为已过期");
        assert!(!nav.is_aborted(1), "lag=2 未超过 ABORT_LAG");
        assert!(!nav.should_skip_put(1), "lag=2 未超过 SKIP_PUT_LAG");

        nav.advance_generation(); // 代次 = 4
        assert!(nav.should_skip_put(0), "超过 SKIP_PUT_LAG 跳过 put");
    }

    /// 时钟共享：同一缓存克隆出的 Navigator 共享同一时钟（命令层每次现造
    /// Navigator，跨导航失活协议不断——旧实例捕获的代次经新实例的推进而失活）。
    #[test]
    fn generation_clock_shared_across_navigators() {
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);
        let old_nav = test_nav(cache.clone(), first_cache.clone(), fake_decoder);
        // 旧实例捕获当前代次（模拟已派生的后台任务）。
        let old = old_nav.current_generation();
        // 新导航现造 Navigator（同生产路径）并推进。
        let new_nav = test_nav(cache.clone(), first_cache, fake_decoder);
        new_nav.advance_generation();
        assert_eq!(old_nav.current_generation(), 1, "推进经共享格全局可见");
        new_nav.advance_generation();
        new_nav.advance_generation();
        assert!(old_nav.is_aborted(old), "跨实例推进后旧代次失活");
        // 独立缓存的时钟互不干扰。
        let other = test_nav(DecodeCache::new(8), FolderFirstCache::new(4), fake_decoder);
        assert_eq!(other.current_generation(), 0);
    }

    /// (c) 跨文件夹提升：队列 A 命中在 navigate 返回时已同步进入队列 B，
    /// 生产 load_image 路径（get_entry）即时命中，无需重新枚举。
    #[test]
    fn folder_cross_promotes_synchronously() {
        let base = build_tree();
        // 停在 A 末张，下一步跨入 B。
        let model = open_sync(&base.join("A/a2.png"));
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);
        // 预置队列 A：B 文件夹首图（模拟此前预取；观察面只用公开 put）。
        let b_dir = base.join("B");
        let b_first = b_dir.join("b1.jpg").to_string_lossy().to_string();
        first_cache.put(
            b_dir.to_string_lossy().to_string(),
            Arc::new(FolderFirst {
                path: b_first.clone(),
                result: Arc::new(fake_decoder(&b_first, true).unwrap()),
            }),
        );
        // 关闭派生（避免后台任务干扰断言），保留提升（提升无门控）。
        let quiet = PrefetchPolicy {
            enabled: true,
            prev: 0,
            next: 0,
            folder_depth: 0,
            decode_paths: false,
            folder_firsts: false,
        };
        let nav = test_nav(cache.clone(), first_cache, fake_decoder);
        assert!(cache.get_entry(&b_first).is_none(), "提升前队列 B 应无命中");

        let r = nav.navigate(&model, NavStep::Next, &quiet);
        assert!(r.boundary.is_none(), "A 末张 next 应跨入 B");
        assert_eq!(model.state().folder_name, "B");
        // 同步断言（无 sleep）：生产 demand 路径即时命中 = 提升已发生。
        assert!(
            cache.get_entry(&b_first).is_some(),
            "跨文件夹后首图应已同步提升入队列 B（load_image 零解码）"
        );
        cleanup(&base);
    }

    /// (d) 前端上下文返回 asset（预取侧的跳过规则已收归 cache_policy 表测试，
    /// 此处只断言只读投影的 Navigator 缝）。
    #[test]
    fn frontend_context_returns_assets_for_frontend_pool() {
        let base = build_tree();
        let model = open_sync(&base.join("B/b1.jpg"));
        let cache = DecodeCache::new(8);
        let nav = test_nav(cache.clone(), FolderFirstCache::new(4), fake_decoder);
        let policy = enabled_policy();

        // 前端投影：含 asset 首图（A/a1.png）与上下文邻居。
        let ctx = nav.frontend_context(&model, &policy);
        let a_first = base.join("A").join("a1.png").to_string_lossy().to_string();
        assert!(
            ctx.contains(&a_first),
            "frontend_context 应返回 asset 首图供前端池预热：{ctx:?}"
        );
        cleanup(&base);
    }

    /// 假扫描器：恒返回非 asset 首图路径（不断言枚举，只看是否入首图缓存）。
    fn fake_scanner(_dir: &Path) -> Option<PathBuf> {
        Some(std::env::temp_dir().join("z-viewer-nav-first/img.raw"))
    }

    /// 首图成功路径：非 asset 首图正常入队列 A，且登记配对释放。
    #[test]
    fn prefetch_first_success_puts_and_pairs() {
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);
        let nav = Navigator::with_seams(
            cache.clone(),
            first_cache.clone(),
            fake_decoder,
            fake_scanner,
        );
        let key = "some-folder".to_string();
        assert!(cache.begin_prefetch(&key));
        nav.prefetch_first(PathBuf::from(&key), key.clone(), nav.current_generation());
        let hit = first_cache.get(&key).expect("首图成功路径应入队列 A");
        assert_eq!(
            hit.path,
            std::env::temp_dir()
                .join("z-viewer-nav-first/img.raw")
                .to_string_lossy()
                .to_string(),
            "首图路径随缓存值保存（提升时免重枚举）"
        );
        assert!(cache.begin_prefetch(&key), "成功路径必须配对 end_prefetch");
        cache.end_prefetch(&key);
    }

    /// (e) begin/end 在失活/失败路径下配对（无并发门控泄漏）。
    /// 观察面：连续多轮失败后仍可正常登记（若泄漏，门控将拒绝）。
    #[test]
    fn begin_end_paired_on_failure_paths() {
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);

        // 解码失败路径 × 多轮不同 key。
        let nav_fail = test_nav(cache.clone(), first_cache.clone(), failing_decoder);
        for i in 0..10 {
            let key = format!("fail-{i}.raw");
            assert!(cache.begin_prefetch(&key), "第 {i} 轮应可登记");
            nav_fail.prefetch_one(&key, nav_fail.current_generation());
            assert!(cache.get_entry(&key).is_none(), "解码失败不应入缓存");
        }

        // 失活丢弃路径。
        let nav_stale = test_nav(cache.clone(), first_cache.clone(), fake_decoder);
        for i in 0..10 {
            let key = format!("stale-{i}.raw");
            assert!(cache.begin_prefetch(&key));
            let old = nav_stale.current_generation();
            for _ in 0..10 {
                nav_stale.advance_generation();
            }
            nav_stale.prefetch_one(&key, old);
        }

        // 首图缺失路径（扫描器返回 None 的不存在目录）。
        let nav_first = test_nav(cache.clone(), first_cache.clone(), fake_decoder);
        for i in 0..10 {
            let key = format!("/nonexistent-dir-{i}");
            assert!(cache.begin_prefetch(&key));
            nav_first.prefetch_first(
                PathBuf::from(&key),
                key.clone(),
                nav_first.current_generation(),
            );
        }

        // 若任一路径泄漏登记/计数，门控此时将拒绝新登记。
        assert!(
            cache.begin_prefetch("after-all-failures.raw"),
            "全部失败路径后门控不得泄漏"
        );
        cache.end_prefetch("after-all-failures.raw");
    }

    /// 策略映射：level 0/1/2 经 neighbor_window，depth 钳制 ≤3。
    #[test]
    fn policy_mapping() {
        let mk = |level: usize, depth: usize| {
            PrefetchPolicy::from(&AppSettings {
                cache_level: level,
                folder_first_depth: depth,
            })
        };
        let p0 = mk(0, 1);
        assert!(!p0.enabled && !p0.decode_paths && !p0.folder_firsts);
        assert_eq!((p0.prev, p0.next), (0, 0));
        let p1 = mk(1, 1);
        assert!(p1.enabled && p1.decode_paths && p1.folder_firsts);
        assert_eq!((p1.prev, p1.next), (1, 1));
        assert_eq!(p1.folder_depth, 1);
        let p2 = mk(2, 99);
        assert_eq!((p2.prev, p2.next), (1, 3));
        assert_eq!(p2.folder_depth, 3, "folder_depth 钳制 ≤3");
        let p3 = mk(1, 0);
        assert!(!p3.folder_firsts, "depth=0 时不派生首图队列");
    }

    /// did_open 推进代次但不提升（新模型无可提升首图）。
    #[test]
    fn did_open_advances_without_promote() {
        let base = build_tree();
        let model = open_sync(&base.join("A/a1.png"));
        let cache = DecodeCache::new(8);
        let first_cache = FolderFirstCache::new(4);
        // 即使队列 A 有命中（旧会话残留），did_open 也不提升。
        let b_dir = base.join("B");
        let b_first = b_dir.join("b1.jpg").to_string_lossy().to_string();
        first_cache.put(
            b_dir.to_string_lossy().to_string(),
            Arc::new(FolderFirst {
                path: b_first.clone(),
                result: Arc::new(fake_decoder(&b_first, true).unwrap()),
            }),
        );
        let nav = test_nav(cache.clone(), first_cache, fake_decoder);
        nav.did_open(&model, &enabled_policy());
        assert_eq!(nav.current_generation(), 1, "did_open 推进一次代次");
        assert!(
            cache.get_entry(&b_first).is_none(),
            "did_open 不提升（残留首图不得混入新会话队列 B）"
        );
        // 缓存关闭时空操作。
        nav.did_open(&model, &closed_policy());
        assert_eq!(nav.current_generation(), 1);
        cleanup(&base);
    }
}
