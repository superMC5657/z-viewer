# 05. 预取与实时解码资源让路

Status: resolved
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

## Answer

**改动**（最小实现 = 方案 1）：
- `decode_cache.rs`：新增 `demand_active: AtomicUsize` 计数 + `begin_demand`/
  `end_demand`（配对调用，多路并发叠加）；`begin_prefetch` 前置 demand 判断
  （原子读先行，最便宜也最具决定性）——demand 进行中直接拒绝新预取登记，
  不去重不占锁。已在跑的预取无法廉价中断，按票接受（下一张起生效）。
- `commands/mod.rs load_image`：spawn_blocking 解码前后 begin/end 配对
  （缓存命中路径不登记——无解码不占池；await 后先释放再走错误映射，
  不留悬挂登记）。
- 方案 2（窗口自适应）未做：票 03 计时显示本机解码段不构成主导瓶颈，
  触发条件不成立。方案 3 核对：`libjxl_engine.rs` MAX_THREADS 已是
  `min(available_parallelism, 8)` 现状，未改动。

**测试**：cache 层 `demand_in_flight_blocks_new_prefetch`（含并发叠加：部分
end 后仍让路）与 `demand_yields_only_new_prefetch`（命中/写入/在跑预取释放
不受影响）；navigator 层 `navigate_yields_prefetch_while_demand_in_flight`
（门控解码器阻塞 + `#[cfg(test)] prefetching()` 观察面，确定性断言 demand
期间 navigate 派生不登记不 spawn，demand 结束后同门控恢复）。

**bench 复核**（TIFF 12MP，票 03 两趟法，与票 02 run 对照）：
level 2 快速连翻命中 73%→73%（无回退），冷p50 122.1/125.1→123.3/118.1ms；
level 1 快连 139.6→124.3ms；level 0 免费冷 75.9→70.3ms——均在噪声内同或更好。
说明：本机 TIFF 冷解仅 ~75ms、争核效应小，让路收益要在重格式（JXL 24MP
引擎解码数百 ms）上才显著；该档 bench 需默认 feature 构建（cmake），留
`decode stages` 打点在真实环境回收。`cargo test --no-default-features`
99 passed + clippy -D warnings 干净（默认构建待 cmake）。
