//! browse 模块测试（#[cfg(test)] 独立文件，不混入产品代码）
//! 测试通过 `super::*` 访问 BrowseModel 及私有字段 inner。

use super::*;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_base() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("image-viewer-test-{}-{n}", std::process::id()))
}

fn touch(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

/// 树：A{a1,a2,a10}.png、B{b1,b2}.jpg、C{c1}.png、empty/（无图，应被排除）
fn build_tree() -> PathBuf {
    let base = temp_base();
    for n in ["a1", "a2", "a10"] {
        touch(&base.join("A").join(format!("{n}.png")));
    }
    for n in ["b1", "b2"] {
        touch(&base.join("B").join(format!("{n}.jpg")));
    }
    touch(&base.join("C").join("c1.png"));
    fs::create_dir_all(base.join("empty")).unwrap();
    base
}

fn cleanup(base: &Path) {
    let _ = fs::remove_dir_all(base);
}

fn open_sync(path: &Path) -> BrowseModel {
    let m = BrowseModel::open(path, None).unwrap();
    m.wait_ready(); // 等待后台枚举完成，保证断言稳定
    m
}

#[test]
fn natural_order_and_folders() {
    let base = build_tree();
    let m = open_sync(&base.join("A/a1.png"));
    {
        let d = m.inner.m.lock().unwrap();
        assert_eq!(d.folders.len(), 3, "无图文件夹应被排除");
        assert_eq!(d.folders[0].images.as_ref().unwrap().len(), 3);
        assert_eq!(
            d.folders[0].images.as_ref().unwrap()[2]
                .file_name()
                .unwrap(),
            "a10.png",
            "natord 自然排序：a1, a2, a10"
        );
    }
    cleanup(&base);
}

#[test]
fn cross_folder_next_and_boundary() {
    let base = build_tree();
    let mut m = open_sync(&base.join("A/a10.png"));
    assert!(matches!(m.next(), Nav::Ok));
    assert_eq!(m.state().folder_name, "B", "A 末尾无缝进入 B 开头");
    assert_eq!(m.state().file_name, "b1.jpg");
    assert!(matches!(m.next(), Nav::Ok));
    assert_eq!(m.state().file_name, "b2.jpg", "B 内第二张");
    assert!(matches!(m.next(), Nav::Ok));
    assert_eq!(m.state().folder_name, "C", "B 末尾无缝进入 C 开头");
    assert!(
        matches!(m.next(), Nav::Boundary(Boundary::LastImage)),
        "C 末尾撞全局边界"
    );
    assert_eq!(m.state().file_name, "c1.png", "边界时图片不变");
    cleanup(&base);
}

#[test]
fn prev_and_first_boundary() {
    let base = build_tree();
    let mut m = open_sync(&base.join("C/c1.png"));
    assert!(matches!(m.prev(), Nav::Ok));
    assert_eq!(m.state().folder_name, "B");
    assert_eq!(
        m.state().file_name,
        "b2.jpg",
        "反向跨文件夹进入上一文件夹最后一张"
    );
    while m.state().global_index > 0 {
        assert!(matches!(m.prev(), Nav::Ok));
    }
    assert!(matches!(m.prev(), Nav::Boundary(Boundary::FirstImage)));
    cleanup(&base);
}

#[test]
fn jump_folder_and_folder_boundary() {
    let base = build_tree();
    let mut m = open_sync(&base.join("A/a2.png"));
    assert!(
        matches!(
            m.jump_folder(FolderTarget::Prev),
            Nav::Boundary(Boundary::FirstFolder)
        ),
        "首个文件夹再往前跳 = 文件夹级边界"
    );
    assert!(matches!(m.jump_folder(FolderTarget::Next), Nav::Ok));
    assert_eq!(m.state().folder_name, "B");
    assert!(matches!(m.jump_folder(FolderTarget::Last), Nav::Ok));
    assert_eq!(m.state().folder_name, "C");
    assert!(
        matches!(
            m.jump_folder(FolderTarget::Next),
            Nav::Boundary(Boundary::LastFolder)
        ),
        "末个文件夹再往后跳 = 文件夹级边界"
    );
    assert!(matches!(m.jump_folder(FolderTarget::First), Nav::Ok));
    assert_eq!(m.state().folder_name, "A");
    cleanup(&base);
}

#[test]
fn global_index_accumulates() {
    let base = build_tree();
    let mut m = open_sync(&base.join("A/a1.png"));
    assert_eq!(m.state().global_index, 0);
    assert_eq!(m.state().global_total, 6, "A(3)+B(2)+C(1)");
    m.next();
    m.next();
    m.next(); // A 3 张走完 -> B 第一张
    assert_eq!(m.state().folder_name, "B");
    assert_eq!(m.state().global_index, 3, "跨文件夹后全局位置累计");
    cleanup(&base);
}

#[test]
fn open_dir_takes_first_image() {
    let base = build_tree();
    let m = open_sync(&base.join("B").join("b1.jpg"));
    assert_eq!(m.state().file_name, "b1.jpg");
    cleanup(&base);
}

#[cfg(windows)]
#[test]
fn open_case_insensitive() {
    let base = build_tree();
    // Windows 文件系统大小写不敏感：传大写文件名应能定位
    let m = open_sync(&base.join("A/A2.PNG"));
    assert_eq!(m.state().file_name, "a2.png");
    cleanup(&base);
}

#[test]
fn open_is_fast_on_current_folder() {
    // open() 应立即可用（不等待后台枚举）：state 可读且指向正确文件
    let base = build_tree();
    let m = BrowseModel::open(&base.join("A/a2.png"), None).unwrap();
    assert_eq!(
        m.state().file_name,
        "a2.png",
        "当前文件夹已同步枚举，立即可用"
    );
    assert!(m.state().loading, "后台枚举未完成时 loading=true");
    m.wait_ready();
    assert!(!m.state().loading, "完成后 loading=false");
    cleanup(&base);
}

#[test]
fn on_ready_fires_after_scan() {
    let base = build_tree();
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ready2 = ready.clone();
    let _m = BrowseModel::open(
        &base.join("A/a1.png"),
        Some(Box::new(move |_state| {
            ready2.store(true, Ordering::SeqCst);
        })),
    )
    .unwrap();
    // 等待回调触发（轮询 + 短暂 sleep）
    for _ in 0..100 {
        if ready.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ready.load(Ordering::SeqCst), "扫描完成后应触发 on_ready");
    cleanup(&base);
}

#[test]
fn async_open_large_library_is_immediate() {
    // 模拟大图库：20 个文件夹 × 各 30 张图
    let base = temp_base();
    for d in 0..20 {
        for i in 0..30 {
            touch(
                &base
                    .join(format!("D{d:02}"))
                    .join(format!("img_{i:03}.png")),
            );
        }
    }
    // open 应立即可用（不等后台枚举）
    let start = std::time::Instant::now();
    let m = BrowseModel::open(&base.join("D05/img_010.png"), None).unwrap();
    let open_elapsed = start.elapsed();
    assert!(
        open_elapsed < std::time::Duration::from_millis(50),
        "open 应 <50ms（仅枚举当前文件夹），实际 {open_elapsed:?}"
    );
    assert_eq!(m.state().file_name, "img_010.png", "首图立即可用");
    assert_eq!(
        m.state().global_total,
        30,
        "初始已知总数 = 当前文件夹图片数"
    );
    // 后台完成后总数 = 20×30=600
    m.wait_ready();
    assert_eq!(m.state().global_total, 600, "扫描完成后全局总数正确");
    assert_eq!(m.state().global_index, 5 * 30 + 10, "全局位置累计正确");
    cleanup(&base);
}

#[test]
fn nav_waits_for_unscanned_folder() {
    // 打开后立即跨文件夹导航：应等待目标文件夹枚举完成（Condvar）
    let base = build_tree();
    let mut m = BrowseModel::open(&base.join("A/a10.png"), None).unwrap();
    assert!(
        matches!(m.next(), Nav::Ok),
        "A 末尾 next 应进入 B（等待枚举）"
    );
    assert_eq!(m.state().folder_name, "B");
    assert_eq!(m.state().file_name, "b1.jpg");
    cleanup(&base);
}

#[test]
fn context_paths_forward_in_first_folder() {
    // 首文件夹中间图向前：应在当前文件夹内取（回归：fi==0 时被整段跳过）
    let base = build_tree();
    let m = open_sync(&base.join("A/a2.png"));
    let ctx = m.context_paths(2, 0);
    assert_eq!(ctx.len(), 1, "A 是第一文件夹，a2 向前只有 a1 一张");
    assert_eq!(ctx[0].file_name().unwrap(), "a1.png", "向前第 1 张");
    cleanup(&base);
}

#[test]
fn context_paths_cross_folder_both_directions() {
    // B/b1：向前 1 应到 A 的 a10（跨文件夹末尾），向后 2 应到 b2 与 C 的 c1
    let base = build_tree();
    let m = open_sync(&base.join("B/b1.jpg"));
    let ctx = m.context_paths(1, 2);
    let names: Vec<String> = ctx
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    assert_eq!(names, vec!["a10.png", "b2.jpg", "c1.png"], "前1后2（跨文件夹）");
    cleanup(&base);
}

#[test]
fn context_paths_at_global_boundaries() {
    let base = build_tree();
    // 全局第一张：向前 0，向后应 C 内 2 张 + B 的 2 张...
    let m = open_sync(&base.join("A/a1.png"));
    assert!(m.context_paths(1, 0).is_empty(), "全局第一张无向前");
    // 全局最后一张：向后 0
    let m2 = open_sync(&base.join("C/c1.png"));
    assert!(m2.context_paths(0, 2).is_empty(), "全局最后一张无向后");
    cleanup(&base);
}

#[test]
fn nav_skips_empty_sibling_folder() {
    // P2-2 回归：A 与 B 之间夹一个空目录 A0，从 A 末尾 next 应跳过 A0 直达 B
    let base = temp_base();
    for n in ["a1", "a2"] {
        touch(&base.join("A").join(format!("{n}.png")));
    }
    for n in ["b1", "b2"] {
        touch(&base.join("B").join(format!("{n}.jpg")));
    }
    // A < A0 < B（natord 自然序）
    fs::create_dir_all(base.join("A0")).unwrap();

    let m = open_sync(&base.join("A/a2.png"));
    // 扫描完成后空 A0 已被压缩移除，且当前文件夹无变化
    {
        let d = m.inner.m.lock().unwrap();
        assert_eq!(d.folders.len(), 2, "空文件夹 A0 应被压缩移除");
    }
    assert_eq!(m.state().file_name, "a2.png");
    // next 从 A 末尾无缝进入 B
    let mut m = m;
    assert!(matches!(m.next(), Nav::Ok), "跨文件夹跳过空 A0");
    assert_eq!(m.state().folder_name, "B");
    assert_eq!(m.state().file_name, "b1.jpg");
    cleanup(&base);
}

#[test]
fn nav_during_scan_never_panics() {
    // P2-2 竞态回归：扫描未完成时连续导航，期间后台压缩可能缩短 folders 列表，
    // 不得 panic（越界）或 poison 模型 Mutex
    let base = temp_base();
    for d in 0..10 {
        for i in 0..3 {
            touch(&base.join(format!("D{d:02}")).join(format!("{i}.png")));
        }
    }
    // 夹一个空目录（扫描后会被压缩移除）
    fs::create_dir_all(base.join("D05b")).unwrap();
    let mut m = BrowseModel::open(&base.join("D00/0.png"), None).unwrap();
    // 不等扫描完成立即连续导航（撞边界或扫描完成后停止）
    for _ in 0..60 {
        if !m.state().loading {
            break;
        }
        match m.next() {
            Nav::Ok => {}
            _ => break,
        }
    }
    m.wait_ready();
    // 最终状态有效且不 panic（当前文件夹非空，path 不应为空）
    assert!(!m.state().path.is_empty(), "导航后当前图片路径应有效");
    cleanup(&base);
}

// ---------- 专业版门控：免费版单文件夹模式 ----------

#[test]
fn free_mode_single_folder_no_cross() {
    let base = build_tree();
    // 免费版（cross_folder=false）：打开 A/a1.png 只浏览当前文件夹
    let m = BrowseModel::open_gated(&base.join("A/a1.png"), None, false).unwrap();
    {
        let d = m.inner.m.lock().unwrap();
        assert_eq!(d.folders.len(), 1, "免费版 folders 只含当前文件夹");
        assert!(!d.loading, "免费版无后台扫描，loading=false");
        assert_eq!(d.global_total, 3, "全局总数 = 当前文件夹图片数（不累计兄弟）");
    }
    // next 到文件夹末尾即停（不跨到 B）
    let mut m = m;
    assert_eq!(m.state().file_name, "a1.png");
    for _ in 0..2 {
        assert_eq!(m.next(), Nav::Ok, "文件夹内翻页正常");
    }
    assert_eq!(m.next(), Nav::Boundary(Boundary::LastImage), "文件夹末尾即全局边界（不跨文件夹）");
    // prev 回退正常，回到开头后再 prev 撞全局边界
    assert_eq!(m.prev(), Nav::Ok);
    assert_eq!(m.prev(), Nav::Ok);
    assert_eq!(m.prev(), Nav::Boundary(Boundary::FirstImage), "文件夹开头即全局边界");
    // jump_folder 无目标可跳
    assert_eq!(m.jump_folder(FolderTarget::Next), Nav::Boundary(Boundary::LastFolder));
    cleanup(&base);
}

#[test]
fn pro_mode_cross_folder_still_works() {
    let base = build_tree();
    // 专业版（cross_folder=true）：与旧行为一致，可跨到兄弟文件夹
    let m = BrowseModel::open_gated(&base.join("A/a10.png"), None, true).unwrap();
    {
        let d = m.inner.m.lock().unwrap();
        assert_eq!(d.folders.len(), 4, "专业版扫描全部同级目录（含空目录，后台压缩）");
        assert!(d.loading);
    }
    m.wait_ready();
    let m = m;
    {
        let d = m.inner.m.lock().unwrap();
        assert_eq!(d.folders.len(), 3, "后台压缩后无图文件夹被排除");
    }
    let mut m = m;
    // A 末张 a10.png → next 跨到 B/b1.jpg
    let nav = m.next();
    assert_eq!(nav, Nav::Ok);
    assert_eq!(m.state().file_name, "b1.jpg", "专业版跨文件夹衔接");
    cleanup(&base);
}
