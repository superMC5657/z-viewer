//! cache 模块测试（#[cfg(test)] 独立文件，不混入产品代码）
//! 测试通过 `super::*` 访问 LruQueue 与两个缓存类型。

use super::*;

fn sample(path: &str) -> Arc<LoadResult> {
    Arc::new(LoadResult {
        mode: "raw".into(),
        data: Some(format!("data-{path}")),
        frames: None,
    })
}

#[test]
fn lru_hit_and_eviction() {
    let cache = DecodeCache::new(3);
    cache.put("a".into(), sample("a"));
    cache.put("b".into(), sample("b"));
    cache.put("c".into(), sample("c"));
    assert!(cache.get("a").is_some(), "最近写入可命中");
    // 访问 a 后 a 变为 MRU，写入 d 应淘汰最久未用的 b
    cache.put("d".into(), sample("d"));
    assert!(cache.get("b").is_none(), "超出容量淘汰 LRU(b)");
    assert!(cache.get("a").is_some());
    assert!(cache.get("c").is_some());
    assert!(cache.get("d").is_some());
}

#[test]
fn put_updates_existing() {
    let cache = DecodeCache::new(2);
    cache.put("a".into(), sample("a1"));
    cache.put("a".into(), sample("a2"));
    let hit = cache.get("a").expect("命中");
    assert_eq!(hit.data.as_deref(), Some("data-a2"), "重复写入应更新");
    // 容量 2：a(更新后) + b
    cache.put("b".into(), sample("b"));
    cache.put("c".into(), sample("c"));
    assert!(cache.get("a").is_none(), "a 被新写入挤出");
    assert!(cache.get("b").is_some());
    assert!(cache.get("c").is_some());
}

#[test]
fn cache_survives_clone() {
    let cache = DecodeCache::new(2);
    cache.put("a".into(), sample("a"));
    let c2 = cache.clone();
    assert!(c2.get("a").is_some(), "clone 共享内部 Arc");
    c2.put("b".into(), sample("b"));
    assert!(cache.get("b").is_some(), "clone 写入对原实例可见");
}

#[test]
fn prefetch_dedup() {
    let cache = DecodeCache::new(4);
    assert!(cache.begin_prefetch("a"), "首次登记成功");
    assert!(!cache.begin_prefetch("a"), "重复登记被拒");
    assert!(!cache.begin_prefetch("a"), "仍被拒（未释放）");
    cache.end_prefetch("a");
    assert!(cache.begin_prefetch("a"), "释放后可再次登记");
    cache.end_prefetch("a");
}

#[test]
fn peek_does_not_move() {
    let cache = DecodeCache::new(3);
    cache.put("a".into(), sample("a"));
    cache.put("b".into(), sample("b"));
    cache.put("c".into(), sample("c"));
    assert!(cache.peek("a"), "peek 应命中");
    // a 仍是最旧：写入 d 应淘汰 a（若 peek 移了位则淘汰 b）
    cache.put("d".into(), sample("d"));
    assert!(cache.get("a").is_none(), "peek 不应把 a 移到 MRU");
    assert!(cache.get("b").is_some());
}

#[test]
fn prefetch_put_skips_if_already_cached() {
    // 模拟：实时加载先 put，预取任务后 decode 完成 → peek 命中则跳过 put（不覆盖/不重复 LRU 操作）
    let cache = DecodeCache::new(4);
    cache.put("a".into(), sample("realtime"));
    // 预取任务逻辑：decode 完成后 peek 检查
    let skip = cache.peek("a");
    assert!(skip, "实时已缓存 → 预取应跳过 put");
    let hit = cache.get("a").expect("命中实时数据");
    assert_eq!(hit.data.as_deref(), Some("data-realtime"), "未被预取覆盖");
}

#[test]
fn set_capacity_evicts_immediately() {
    let cache = DecodeCache::new(4);
    for i in 0..4 {
        cache.put(format!("p{i}"), sample(&format!("p{i}")));
    }
    cache.set_capacity(2);
    assert!(cache.get("p0").is_none(), "容量收紧后淘汰最旧");
    assert!(cache.get("p1").is_none());
    assert!(cache.get("p2").is_some());
    assert!(cache.get("p3").is_some());
}

#[test]
fn folder_first_cache_lru() {
    let cache = FolderFirstCache::new(2);
    cache.put("A".into(), sample("a"));
    cache.put("B".into(), sample("b"));
    assert!(cache.peek("A"));
    assert!(cache.get("A").is_some(), "命中 A 并移到 MRU");
    cache.put("C".into(), sample("c"));
    assert!(cache.get("B").is_none(), "A 被访问后 B 是 LRU 被淘汰");
    assert!(cache.get("A").is_some());
    assert!(cache.get("C").is_some());
    // 关闭缓存：set_capacity(0) 清空
    cache.set_capacity(0);
    assert!(cache.get("A").is_none(), "容量 0 全清");
}
