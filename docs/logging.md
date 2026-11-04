# 统一日志（logging）

> 复用 `tauri-plugin-log` v2，不自动上报。日志只进 LogDir 文件，**禁止写入业务 SQLite**。
> 实现：`src-tauri/src/z_log.rs`（插件/脱敏/prune/panic）、`src/lib/z-log.ts`（前端转发）。

## 日志目录（各 OS，`identifier = com.zviewer.app`）

目录由 `app.path().app_log_dir()` 解析（启动时 `main.rs` 先 `create_dir_all` + `prune`）：

| OS | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\com.zviewer.app\logs` |
| macOS | `~/Library/Logs/com.zviewer.app`（注意：无末尾 `logs`） |
| Linux | `~/.local/share/com.zviewer.app/logs`（`XDG_DATA_HOME` 存在时以它为基） |

panic hook 拿不到 `AppHandle`，用 `resolve_log_dir_fallback()` 按同样规则兜底
（全失败时退到系统临时目录 `com.zviewer.app-logs`）。

## 目录里的文件

| 文件 | 来源 | 说明 |
| --- | --- | --- |
| `<productName>.log`（如 `ZViewer.log`，+轮转兄弟 `ZViewer_*.log`） | Rust 侧 + 前端经 `plugin:log|log` | 统一日志（文件名从配置文件 `tauri.conf.json` 的 `productName` 动态获取；统一因果时间轴） |
| `crash-<ts>.txt` | panic hook | 版本/identifier/时间/线程/位置/last_viewed（仅 basename）/payload（首行截断）/Backtrace（仅 dev 构建，见已知限制），写盘前已脱敏 |

单文件 5MB 轮转（`max_file_size`），策略 `KeepSome(5)`（插件层只留 5 个轮转兄弟）；
`prune`：删 14 天前文件，总量超 25MB 时按 mtime 从旧到新删到预算内，
只处理 `*.log` 与 `crash-*.txt`，失败只记 `log` 不抛 panic。
prune 内两条打点均为 `debug`（`removed expired` / `over budget removed`，只记 basename，release 不落盘）。
水位由 `z_log.rs` 常量锁定（`MAX_FILE_SIZE`/`ROTATION_KEEP`/`PRUNE_KEEP_DAYS`/`PRUNE_MAX_BYTES`），
`test_converged_waterline` 回归锁定；`main.rs` 启动修剪复用同一常量。

## dev / release 矩阵

| | dev（`debug_assertions` / `vite dev`） | release |
| --- | --- | --- |
| Rust 门槛 | `Debug`（hyper/reqwest/wry/tao 压到 `Warn`） | `Info`（同上压到 `Warn`，`debug` 不落盘） |
| 门槛覆盖 | `ZVIEWER_LOG` 优先、`RUST_LOG` 兜底（`resolve_level`，双 cfg 分支均接入；非法值穿透到默认） | 同左 |
| Stdout | 有 | 无（`clear_targets` 后只留 LogDir） |
| Webview 回显 | 有，但**只回显 Rust 日志**（前端日志不二次广播，无回声循环） | 无 |
| 前端回显 | `attachLogger` + `echoGuard` 把 Rust 日志打印到 devtools，回显输出不再二次转发 | 不 attach |
| 时区/格式 | 本地时区，`[HH:mm:ss.SSS][级别][target] <脱敏后消息>` | 同左 |

## 脱敏（`z_log::redact`，写盘前 format 内执行，无正则零依赖，绝不 panic）

1. 家目录 → `~`（`USERPROFILE` / `HOME`）。
2. 含 `@` 的连续 token → `[redacted-email]`。
3. `bearer <值>`（大小写不敏感，后跟空白才算）→ `bearer [redacted]`；
   `*token[=:] <值>`（含 `api_token`/`access_token`/`analyticsToken` 等前后缀，
   支持 JSON `"xxxToken": "值"` 形态）→ `[redacted]`。
4. 连续 `[A-Z0-9-]` 且长度 ≥16（含 `-` 或数字）→ `[redacted-key]`。

## 前端用法

```ts
// src/main.ts：启动第一行，失败不阻塞启动
import { initZLog } from "./lib/z-log";
void initZLog().catch(() => undefined);
```

- `initZLog()`（幂等）：劫持 `console[log/info/warn/error]` 转发为 `[FE] …`
  （`orig` 先输出，原生 console 行为不变；转发失败静默丢弃）；
  注册 `window.onerror` + `unhandledrejection`；仅 `import.meta.env.DEV` 时回显后端日志。
- 后端直接用 `log::*`，无私有宏封装，调用点即日志点（文件行号准确）。
- 摘要辅助（`util.rs`，纯函数无日志）：`basename_of`（`Path::file_name`，
  无文件名时回退 `unknown`）、`first_line_of`（首行 + 200 字符截断，`chars()` 字符边界安全）。

## 级别语义

- `info`：生命周期（打开成功、激活/注销成功）
- `warn`：可恢复（打开失败、解码失败、激活失败、离线降级）
- `error`：失败（浏览模型打不开：`open_path` 直接返回前端错误）
- `debug`：细节（导航边界、预取失败、续验成功/推迟、埋点上报失败）

## 消息风格与红线

- 一律英文小写前缀模块词（`browse open` / `decode failed` / `license revoked` …），只记摘要。
- 路径只许 basename（`Path::file_name`）；原因取首行或截断封顶（`first_line_of` 200 字符）。
- 禁止：完整 args/全路径/激活码-email-JWT-deviceId 三件套值/服务端原文/全文 dump；
  `redact` 只是兜底，“不该记的值”不许进消息。
- 禁止 `println!/eprint!/dbg!`；前端高频热路径（鼠标移动、缩放、拖拽、rAF 逐帧渲染）不打点，关键生命周期与状态转换使用 `console.*` 经 `z-log` 统一汇入统一日志文件（`<productName>.log`）。

## 应用生命周期与浏览（`main.rs` + `commands/mod.rs`）

- `info` zviewer startup: version {ver} (os: {os}, arch: {arch})（应用启动与运行环境）
- `info` zviewer exit: session duration {dur}s, images viewed: {v}, unique: {u}, folders: {f}（正常退出时会话统计小结）
- `info` browse open ok {basename} (folder: {folder_name}, total: {total})（打开图片成功，包含所属文件夹与总图片数）
- `info` browse open reject {not-image|no-parent|unknown-image|unknown-folder|empty-dir} {basename}（打开拒绝原因码，只记 basename）
- `debug` browse scan done folders {n} total {m}（后台兄弟扫描完成计数，只记数量；全空防御分支另带 `(all-empty)` 后缀）
- `warn` browse open failed {basename}（启动路径打不开：非图片/空目录）
- `error` browse open failed {basename}（`open_path` 模型建立失败，直接返回前端错误）
- `info` browse jump folder {target} ok {folder_name}（文件夹级跳转成功）
- `info` browse nav crossed folder to {folder_name}（翻页无缝跨越进入新文件夹）
- `debug` browse nav {global_index+1}/{global_total} {file_name}（单图切换：高频交互，仅 dev 可见，release 不落盘防刷屏）
- `debug` browse boundary {first-image|last-image|first-folder|last-folder}（导航触底边界，debug 级别）
- `warn` browse jump failed: unknown target（未知跳转目标）

## 前端交互与功能状态（`src/main.ts` + `src/viewer.ts` + `src/presence.ts` + `src/services/updater.ts`）

所有常规翻页统一由 Rust 后端处理，前端仅在**关键视图模式变更**与**功能周期**时输出 INFO（低频、关键）：
- `info` [ui] immersive mode: on / off（沉浸模式切换）
- `info` [ui] pin window: on / off（窗口置顶切换）
- `info` [view] fit mode: fit / actual（适应窗口 / 1:1 像素实际大小模式切换）
- `info` [view] rotate: +90° / -90° (current: ...)（图片旋转）
- `info` [view] flip: horizontal / vertical（图片翻转）
- `info` [updater] checking for updates... / check complete: already up to date / update available / downloading ... / download complete ..., installing... / update installed（更新检查→下载→安装生命周期）
- `error` [updater] download failed / install failed / relaunch failed（下载与安装失败分流，原文截断由各调用点控制）
- `info` [slideshow] started (interval: {ms}ms) / paused / interval changed / finished / exited（幻灯片生命周期）
- `error` window.onerror / unhandledrejection（全局未捕获异常）
- `error` [render] failed: {err}（DOM 原生图片加载失败）
- `debug` [app] frontend initialized（开发环境启动心跳）

## 解码（`decode/mod.rs` 分发 + `commands/mod.rs` 命令层）

- `warn` decode failed {basename} [raw|tiff|jxl|anim]: {原因首行}（通道侧：`decode/mod.rs` 分发点按通道记一次，通道后缀与命令层去重）
- `warn` decode fallback {basename} [jxl]: {原因首行}（libjxl 主引擎失败、jxl-oxide 回退成功；回退原因经 `decode_jxl` 返回值上浮，分发点记录）
- `warn` decode failed {basename}: {原因首行}（`load_image` 实时路径：RAW/TIFF/JXL/动画全通道收敛于此；
  预取后台调用走 `cache prefetch failed` 的 debug，见下）
- `debug` decode done {basename} in {ms}ms（通道耗时打点：raw/tiff/jxl/anim 通道成功后记一次，
  asset 通道零解码不记；release 不可见，翻页延迟分析用，见 `flip_latency_bench`）
- `debug` decode stages {basename} read=.. engine=.. [orient=.. resize=.. rgba=..] total={ms}ms
  （分阶段计时明细：`decode::stage` 会话在 `decode/mod.rs::load_image_inner` emit，
  与 `decode done` 同请求成对；阶段语义见 `decode/stage.rs` 模块注释——分段严格可加，
  JXL/RAW 引擎自带文件 IO 归 engine 段不可拆分；release 不可见，真实用户分段回收用）
- `debug` decode pack {basename} in {ms}ms（信封打包耗时：`commands::load_image` 命令层
  pack_envelope；跨线程不属 decode 段行，与 stages 行按 basename+时序关联出传输占比）
- 解码器内部（`raw.rs`/`tiff.rs`/`jxl.rs`/`animation.rs`）零 log：底层错误（io/ImageError/rawler `Display`）
  均不含路径，只在分发点 + 命令层各记一次（后缀区分）。

## 缓存（`commands/mod.rs` + `navigator.rs` + `cache/*.rs`）

- `info` cache level set {level}（缓存等级切换：0=关闭、1=开启、2=高，配置变迁留痕）
- `debug` cache prefetch failed (ctx) {basename}: {原因首行}（队列 B 上下文预取；
  低价值后台优化，release 不可见）
- `debug` cache prefetch failed (first) {basename}: {原因首行}（队列 A 首图预取；同上）
- `debug` cache hit {basename} / cache miss {basename}（`DecodeCache::get_entry`，热路径只 debug）
- `debug` cache evict dropped {n} (cap {cap})（`LruQueue::put`/`set_capacity` 淘汰计数，只记数量不记路径）
- LRU 本体其他路径零 log：锁失败/未命中是常态，只在上述三处记 debug。

## 授权（`license.rs`，只记事件不记值）

`activate/deactivate/verify_online` 的 `Err(String)` 载荷含服务端自由文本/reqwest 错误
（含 URL），一律不进消息；有疑问只记静态文案。激活码 / email / JWT / deviceId 一律不进日志。

- `info` license activated（激活成功）
- `warn` license activate failed（激活失败，静态文案）
- `warn` license activate token verify failed（本地验签不过；与自有静态文案精确相等才分流，
  服务端自由文本不参与分流）
- `debug` license verify ok（在线续验成功，release 不可见）
- `warn` license verify offline, using grace period（网络失败走离线宽限期）
- `warn` license revoked, credentials removed (http-status) / (body-error) / (valid-flag)（吊销删凭证，3 处按分支后缀区分：HTTP 非 2xx 吊销码/body 错误码/valid=false）
- `debug` license verify deferred (http-status) / (body)（其他服务端错误/响应解析异常，保留凭证等重验）
- `info` license status changed {pro|free}（启动后台续验发现吊销/失效并广播事件时，只记目标状态；`main.rs`）
- `info` license deactivated（注销成功）/ `warn` license deactivate failed（注销失败，静态文案）
- `debug` license load invalid fallback to free（启动 `load()` 文件存在但解析/验签失败回退免费版；
  静态文案，不记路径不记内容；`LicenseManager::load`；文件不存在正常回退不记）

## 埋点（`analytics.rs`）

- `debug` analytics report failed (client) / (request) / (send)（client 构造失败/请求构造失败（非法 URL）/发送失败；
  按分支后缀区分，静态文案，不记负载内容；未配置 apiBase/analyticsPath 时静默不上报，不记）
- `record_view` 高频计数点零 log；上报负载只含聚合统计（见 `analytics.rs` 隐私边界）。

## 已知限制

- `Cargo.toml [profile.release] panic = "abort"`：release 下 panic 直接 abort，
  panic hook（`crash-*.txt`）仅 dev/unwind 构建有效。如需 release 崩溃报告，
  需另议 panic 策略（不在本文范围内，不动 release profile）。

## 相关文件（符号索引）

行号天生易腐，本节按符号定位（grep 符号名即达）；新增打点时在此补符号而非行号。

- `src-tauri/src/z_log.rs`（插件构建、门槛覆盖 `resolve_level`、`prune`（basename 打点）、`redact`、panic hook `write_crash_report`；
  回归：`test_converged_waterline` 锁水位、`test_format_path_masks` 锁脱敏、`test_crash_last_viewed_basename_only`/`test_crash_payload_first_line` 锁 crash 字段、`test_resolve_level_dual_branch` 锁双分支）
- `src-tauri/src/main.rs`（`startup_image_path` 启动参数跳过 debug；`setup` 内 startup/open 打点与授权状态变迁；启动修剪复用水位常量）
- `src-tauri/src/util.rs`（日志摘要辅助单一实现：`basename_of`/`first_line_of`/`now`）
- `src-tauri/src/commands/mod.rs`（`navigate` 边界/跨越/单图切换、`open_path` 打开成败、
  `jump_folder` 未知跳转目标、`load_image` 解码失败 + 信封打包 `decode pack` debug）
- `src-tauri/src/decode/mod.rs`（通道分发 warn：`[raw]`/`[tiff]`/`[jxl]`/`[anim]` 后缀 + `[jxl]` 引擎回退，
  `decode done` 通道耗时 + `decode stages` 分段明细 debug（emit 经 `decode::stage` 会话），
  摘要辅助转发自 `util`，与命令层后缀去重）
- `src-tauri/src/decode/stage.rs`（分阶段计时会话单一实现：`begin`/`timed`/`emit`、
  阶段名常量 `name::*`、同名累计 `accumulate`；测试观察面 `force`/`snapshot` 随构建剔除）
- `src-tauri/src/browse/scan.rs`（打开拒绝原因码 info + 后台扫描计数 debug）
- `src-tauri/src/cache/*.rs`（`DecodeCache::get_entry` hit/miss debug + `LruQueue` 淘汰计数 debug）
- `src-tauri/src/navigator.rs`（预取派生与代次协议：`cache prefetch failed (ctx)`/`(first)` debug、
  `navigate`/`did_open`/`frontend_context`；翻页延迟基准 `flip_latency_bench`）
- `src-tauri/src/license.rs`（授权事件打点：`LicenseManager::load` 启动加载回退、
  `verify_online` 离线降级/吊销三态/推迟/续验成功、`activate_license` 激活与验签失败、
  `deactivate_license` 注销）
- `src-tauri/src/analytics.rs`（`report_exit`：退出小结 info + 上报失败 debug（client/request/send 后缀））
- `src-tauri/src/browse/*.rs`、`src-tauri/src/decode/*.rs`、`src-tauri/src/cache/*.rs`
  （零 log：错误上浮到命令层记一次，热路径不记）
- `src/lib/z-log.ts`、`src/main.ts`（前端接入；业务不加打点）
- `src-tauri/capabilities/default.json`（`log:default`）
