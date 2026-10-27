# 05. 预取与实时解码资源让路

Status: ready-for-agent
Type: task
Priority: P2
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状① R3

## 问题

`decode_cache.rs` PREFETCH_MAX=2：预取解码（libjxl 每实例最多 8 线程 +
fast_image_resize rayon 全局池）与用户触发的实时冷解码同在
`tauri::async_runtime::spawn_blocking` 池上并发。冷翻恰好多发生在快速连翻后
——正是预取任务刚派生的时刻，实时解码被 2 个全量预取抢核，最需要快的时候最慢。

## 方案

1. **demand 让路**：`load_image` 实时路径进入解码前设置全局「demand in-flight」
   标志；预取任务在解码循环的检查点（每张图开始前）见标志则让出
   （drop 当前任务或缩线程），结束恢复。最小实现：demand 进行中
   `begin_prefetch` 直接拒绝新任务（现有门控加一个条件）+ 已在跑的预取
   无法廉价中断则接受（下一张起生效）。
2. **窗口自适应**（可选，量力）：冷翻命中后按实际命中率动态建议窗口
   （专2 的前1后3 是静态值）；仅当 issue 03 计时显示解码仍明显慢时做。
3. `libjxl_engine.rs` MAX_THREADS=8 对低核数机器是超订阅（8 线程 on 4 核），
   改 `min(available_parallelism, 8)` 已是现状——确认无额外放大即可，不改动。

## 验收

- `cargo test` 全绿；cache/navigator 测试覆盖 demand 让路条件；
- bench 快速连翻场景（level 2, look=None）数字不回退、冷翻分位改善则记录。

## Comments

- 2026-10-26 建档。
