# 07. 大文件夹导航 Condvar 等待优化（先量化）

Status: needs-info
Type: research
Priority: P2
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状① R5

## 问题

导航命令在目标索引未填充时于 `browse/state.rs` `wait_filled` 的 Condvar 上
同步等待后台扫描（`commands/mod.rs` 注释明示此等待不持 AppState 锁，但命令
本身阻塞）。本地 SSD + 数百文件无感；数千文件 / 机械盘 / 网络盘时，open 后
的头几次翻页可能等秒级——与用户「切换慢」的感知相关度未知。

## 缺的信息

- 用户实际遇到慢的文件夹规模（文件数）、盘类型（本地 SSD/HDD/网络盘）；
- 日志里 `browse nav`/`browse open` 时间戳与翻页请求时间戳的间隔。

## 候选方案（拿到数据后再选）

- **乐观快返**：wait_filled 加短超时（如 150ms），未就绪时按当前已知顺序
  猜测目标并标记 loading（BrowseState 已有 loading 位），扫描到位后校正/重发。
- **预取式导航**：扫描线程优先填充导航方向前若干索引（现在是顺序扫描）。
- **仅诊断**：若数据显示等待占比可忽略，wontfix 并记录。

## 验收

- `## Answer` 写明量化结论与方案选择；实施则另拆票。

## Comments

- 2026-10-26 建档，等用户提供场景信息或日志样本。
