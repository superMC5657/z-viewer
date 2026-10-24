# 01. 修复 z_log 路径隐私泄漏(prune 打点 + crash 报告)

Status: resolved
Source: code review 2026-10-24, Standards 轴硬违规 #1/#2
Severity: P1

## 问题

- `src-tauri/src/z_log.rs:379,395`:prune 打点为中文消息,违反 logging.md「消息一律英文小写前缀模块词」;`p.display()` 记录**全路径**,违反「路径只许 basename」红线。redact 只折叠家目录,非家目录路径整条落盘。
- `src-tauri/src/z_log.rs:474-476`:crash 报告含 `last_viewed` **全路径**字段——logging.md 的 crash 字段表没有该字段;用户崩溃时正在看什么可随诊断包带出。

## 修改建议

1. prune 两处改为英文小写消息并用仓库现成的 basename 工具(参考 z_log.rs 内已有的 basename_of/redact 基建),例如:
   `log::debug!("zlog prune: removed expired {}", basename_of(&p));`
2. crash 报告的 `last_viewed` 改记 basename(保留诊断价值,去掉定位能力);同时在 logging.md 的 crash 字段表补上该字段(字段与文档二选一对齐:补文档,或删字段——倾向补文档+basename)。
3. 顺带核查 crash 报告的 payload/backtrace 两个字段是否可能携带路径;若有,统一过 redact 或只取 first_line。
4. 为 redact/basename 行为补或更新单测:非家目录路径必须被折叠为 basename。

## 验收

- `grep -n "p.display()" src-tauri/src/z_log.rs` 无直接落盘调用。
- crash 报告字段与 logging.md 字段表一致。
- `cargo test -p z-viewer`(z_log 相关)全绿。

## Comments

- 2026-10-24 已实施：prune 两条打点改英文小写 + `util::basename_of`；crash 报告新增 `crash_last_viewed()`（last_viewed 只落 basename）+ 单测 `test_crash_last_viewed_basename_only`；logging.md 的 crash 字段行与 prune 描述已同步。cargo test 全绿。
- 2026-10-24 复核补做（修改建议 3）：核查结论——payload 是唯一可能内嵌用户路径的字段（panic 文案为自由文本），已改经 `first_line_of` 首行截断（`crash_payload()`）+ 单测 `test_crash_payload_first_line`；backtrace 只含构建机源码路径（非用户文件），且 release `panic = "abort"` 下 panic hook 不产 crash 文件（dev-only 产物），整段已经过 redact（家目录折叠），维持全量保留诊断价值；`location` 为相对源码路径，同属构建机信息。logging.md crash 字段行与已知限制已同步。
