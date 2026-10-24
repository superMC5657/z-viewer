# Code Review 报告 — 2026-10-24

范围:整个项目(fixed point = 空树 `4b825dc`,diff 覆盖全部 127 个提交)。双轴审查:Standards(标准轴)+ Spec(规格轴),由两个独立子代理并行完成,关键发现已人工抽查核实。

标准来源:`docs/logging.md`、`AGENTS.md`、`docs/agents/domain.md`、`README.md` + Fowler 坏味道基线。
规格来源:`docs/需求报告与技术方案.md`、`docs/开发阶段规划.md`、`docs/adr/0001-jxl-libjxl-decode-engine.md`。

## Standards 轴

### 硬违规(违反 docs/logging.md)

1. **日志泄漏全路径(隐私)** — `src-tauri/src/z_log.rs:379,395` prune 打点为中文消息(违反「消息一律英文小写前缀模块词」),`p.display()` 记录全路径(违反「路径只许 basename」红线);redact 只折叠家目录,非家目录路径整条落盘。全仓库仅此两处。
2. **crash 报告含 last_viewed 全路径(隐私)** — `z_log.rs:474-476` crash 报告含文档未载的 `last_viewed` 全路径字段,logging.md 的 crash 字段表没有它。用户崩溃时正在看什么可随诊断包带出。
3. **标准文件自身失真** — logging.md 引用的 `src/window-state.ts` 已不存在(并入 presence.ts);「相关文件(行号索引)」行号全部漂移(如 license.rs:238→实际 254)。

### 基线坏味道(judgement call)

4. **Duplicated Code** — `src/presenter.ts:39` ANIM_EXTS 与 `src-tauri/src/decode/format.rs:21` 两份人肉同步。仓库为 RAW 扩展名专建真源 `get_raw_extensions`(`commands/mod.rs:400`)并写明漏同步后果,动画表无此保护:后端新增动画格式,前端帧条静默失效。
5. **Mysterious Name** — `commands/mod.rs:176,294,420` `require_pro(license.is_pro(), false, || true)` 恒等于 `license.is_pro()`,闭包形式遮蔽语义。
6. **Duplicated Code + Data Clump** — `src/viewer.ts:157-165` 与 `203-219` 重复 seq/prevOwned/revokePrev/restorePrev 簿记,4 个参数穿入 showEnvelopeInternal(:235),可捆成一个加载上下文类型。
7. **Duplicated Code** — `src/presenter.ts:433-522` ensureAnimation 约 90 行内三处重复「关位图+关解码器+return」清理块(467/482/495)。
8. **调试残留** — `src/main.ts:314` `(window).__lastAdvance` 幻灯片每拍写全局供 CDP 读取,随生产发布。
9. **次要杂项** — LoadResult.mode / NavResult.boundary 字符串魔法值(Primitive Obsession;decode_cache.rs:150、cache_policy.rs:22、presenter.ts:30 比对点);license.rs:398/417 吊销错误码 matches! 重复两份;commands/mod.rs:394 get_settings 绕过 SettingsState::read 直抓锁;license.rs:731 status() 注释称「单快照」实为两次加锁。

整体评价:日志纪律(basename_of/first_line_of、license 静态文案、零 println/dbg)、navigator/browse/cache_policy 深模块划分与测试覆盖执行严格,上述问题均属局部修正。

## Spec 轴

### (a) 规格要求但缺失/半做

1. **核心特色被付费门控** — 「跨同级文件夹无缝浏览」(需求 2.2「核心特色」)在免费版被整体禁用:`browse/scan.rs` `open_gated(cross_folder=false)` 不枚举兄弟文件夹,`jump_folder` 返回 `pro-required`。需求文档从未提出免费/付费;开发阶段规划记载了门控,但把规格核心功能变成付费墙缺乏正式决策记录。
2. **Esc「退出软件」未实现** — presence.ts 中 Esc 只关弹窗/退沉浸(快捷键表 2.5)。
3. **幻灯片间隔「自定义」缺失** — 仅 1s/2s/5s/10s 固定下拉(规格 2.4 为「2s/5s/10s/自定义」),且多出规格未列的 1s。
4. **AVIF 未注册文件关联** — 规格格式表含 AVIF,但 `src-tauri/nsis/installer-hooks.nsh` 注册清单漏掉 avif。

### (b) 规格外新增(scope creep)

1. **整套商业授权体系** — license.rs 1430 行(Ed25519 JWT、设备指纹、在线续验、激活/注销对话框、购买跳转),三份规格均无此需求(ADR 仅侧面提及)。
2. **埋点上报(合规风险)** — analytics.rs 退出时向远端 POST 设备 ID、授权状态、浏览次数、格式/文件夹分布(带 Bearer token),无用户告知与开关。
3. 其他:自动更新(updater)、763 行统一日志 z_log、鼠标侧键翻页、Home/End 跳转、拖拽打开、双击缩放切换。

### (c) 实现与规格相悖/可疑

1. **切图动画** — 规格 §3「图片切换淡入淡出」、规划文档「150ms crossfade 已就绪」;代码已改为 0ms 瞬切(viewer.ts / ui.css:99),文档未同步。
2. **缓存策略缩水** — 规格 §8.2/§4「前 3 张+后 3 张、容量如 8 张」;默认 level1 前1后1、level2 前1后3(settings)。
3. **降采样** — 规格「8000×6000 阈值/屏幕 2 倍、解码前探测尺寸」;实现为固定长边 3840 且解码后缩放(decode/mod.rs MAX_DIM)。

### 全部满足

格式通道(asset 8 类/TIFF/RAW 22 扩展/JXL per ADR)、natord 排序、边界 Toast 1.5s、毛玻璃 UI 与闲置 2s 隐藏、全部变换与快捷键、RAW 预览快显+demosaic+EXIF 回正、动画原生播放+按需拆帧、文件关联/右键菜单/启动参数;「明确不做」清单均未越界。

## 汇总

- Standards 轴 9 项发现(3 硬违规 + 6 judgement call);最重:z_log.rs 日志与 crash 报告泄漏用户文件全路径。
- Spec 轴 10 项发现(4 缺失 + 3 scope creep + 3 相悖);最重:需求文档的「核心特色」跨文件夹浏览在免费版被付费门控,而需求文档本身并无免费/付费之分——需 ADR 正式化商业决策并同步文档。

tickets 见 `issues/01`–`13`,执行计划见各 ticket 的 Severity 与 report 会话记录。
