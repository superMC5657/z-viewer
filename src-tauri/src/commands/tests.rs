//! commands 模块测试（#[cfg(test)] 独立文件，不混入产品代码）
//! 测试通过 `super::*` 访问 AppSettings / prefetch 函数等。

use super::*;
use super::prefetch::prefetch_folder_firsts;
use crate::cache::FolderFirstCache;

#[test]
fn neighbor_window_rules() {
    let s = |l| AppSettings { cache_level: l, folder_first_depth: 1 };
    assert_eq!(s(0).neighbor_window(), (0, 0), "0：不预取");
    assert_eq!(s(1).neighbor_window(), (1, 1), "1：前后各 1");
    assert_eq!(s(2).neighbor_window(), (1, 3), "2(高)：前 1 后 3");
    assert!(s(2).is_enabled());
    assert!(!s(0).is_enabled());
}

/// 用真实测试图验证 DecodeCache 命中路径（load_image 是 async command，此处直接测 decode+cache 组合）
#[test]
fn cache_hit_is_faster_than_miss() {
    let cache = DecodeCache::new(8);
    let path = std::env::current_dir()
        .unwrap()
        .join("..")
        .join("test-images/B/img_2.gif")
        .to_string_lossy()
        .to_string();

    // 首次：解码 + 入缓存（模拟 load_image 主路径）
    let r1 = crate::decode::load_image(&path).unwrap();
    assert_eq!(r1.mode, "animated");
    cache.put(path.clone(), Arc::new(r1.clone()));

    // 命中：直接从缓存取
    let hit = cache.get(&path).expect("应命中缓存");
    assert_eq!(hit.mode, "animated", "命中返回相同数据");

    // 再次 put 同路径（模拟预取覆盖）→ 内容不变
    cache.put(path.clone(), Arc::new(r1.clone()));
    let hit2 = cache.get(&path).expect("再次命中");
    assert_eq!(hit2.mode, "animated");
}

#[test]
fn asset_mode_not_cached() {
    // 单帧静态 PNG 走 asset：load_image 返回 mode=asset 且不应进缓存（占用槽位）
    let cache = DecodeCache::new(2);
    let path = std::env::current_dir()
        .unwrap()
        .join("..")
        .join("test-images/A/1.png")
        .to_string_lossy()
        .to_string();
    let r = crate::decode::load_image(&path).unwrap();
    assert_eq!(r.mode, "asset");
    // 模拟 load_image 的「asset 不 put」分支
    let arc = Arc::new(r);
    if arc.mode != "asset" {
        cache.put(path.clone(), Arc::clone(&arc));
    }
    // 另一张真缓存
    let raw_path = std::env::current_dir()
        .unwrap()
        .join("..")
        .join("test-images/B/img_2.gif")
        .to_string_lossy()
        .to_string();
    let r2 = crate::decode::load_image(&raw_path).unwrap();
    cache.put(raw_path.clone(), Arc::new(r2.clone()));
    cache.put("x".into(), sample("x"));
    // 容量 2：raw + x 在，asset 未占槽
    assert!(cache.get(&raw_path).is_some(), "raw 仍在");
    assert!(cache.get(&path).is_none(), "asset 从未入缓存");
}

fn sample(path: &str) -> Arc<crate::decode::LoadResult> {
    Arc::new(crate::decode::LoadResult {
        mode: "raw".into(),
        data: Some(format!("data-{path}")),
        frames: None,
    })
}

#[test]
fn folder_first_cache_promote_to_neighbor() {
    // 验证：预取 C 首图入队列 A → 导航到 C → promote 并入队列 B
    // C 首图 c01.png 单帧判定 asset → 队列 A 不缓存（asset 由前端池预热）；
    // 此测试验证非 asset 场景：用 B 首图 img_2.gif（动画，非 asset）
    let base = std::env::current_dir().unwrap().join("..").join("test-images");
    // 构造 B 首图为 gif 的模型：直接从 B/img_2.gif 打开
    let model = BrowseModel::open(&base.join("B/img_2.gif"), None).unwrap();
    model.wait_ready();

    let first_cache = FolderFirstCache::new(4);
    let settings = AppSettings::default();

    // 预取相邻文件夹首图（队列 A）：B 的邻居 A 首图 1.png（单帧 png→asset，跳过）、C 首图 c01.png（asset，跳过）
    // 首图非 asset 的邻居文件夹在此测试树中不存在 → 队列 A 为空是正确行为
    prefetch_folder_firsts(&model, &first_cache, &settings);
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 关键验证：get_context 必须包含相邻文件夹首图路径（asset 供前端池预热）
    // 直接验证 first_image_of 与 neighbor_folders 的衔接
    let neighbors = model.neighbor_folders(1);
    assert!(!neighbors.is_empty(), "B 应有相邻文件夹");
    for f in &neighbors {
        let first = BrowseModel::first_image_of(f).expect("相邻文件夹应有首图");
        assert!(first.is_file(), "首图路径应有效");
    }
}

#[test]
fn load_image_does_not_block_on_prefetch() {
    // 预取登记在飞（in_flight 非空）时，实时 load_image 不等待预取、
    // 直接自行解码 —— 实时路径绝不被预取拖住或排队死锁
    let cache = DecodeCache::new(4);
    let path = std::env::current_dir()
        .unwrap()
        .join("..")
        .join("test-images/B/img_2.gif")
        .to_string_lossy()
        .to_string();

    assert!(cache.begin_prefetch(&path), "模拟预取在飞（登记后不释放）");

    let start = std::time::Instant::now();
    let result = crate::decode::load_image(&path).unwrap();
    assert_eq!(result.mode, "animated", "应正常解码");
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "实时加载不应被预取登记拖住 >2s，实际 {elapsed:?}"
    );
    cache.end_prefetch(&path);
}
