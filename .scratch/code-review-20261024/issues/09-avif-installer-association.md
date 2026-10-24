# 09. 安装器补 AVIF 文件关联

Status: resolved
Source: code review 2026-10-24, Spec 轴缺失 #4
Severity: P1

## 问题

规格格式表(需求 2.7/8.7)含 AVIF,但 `src-tauri/nsis/installer-hooks.nsh` 的注册清单漏掉 avif——装完后 AVIF 文件右键「打开方式」找不到本应用。

## 修改建议

1. 对照 `docs/需求报告与技术方案.md` 格式表核对安装器注册清单,补 avif;顺带核查 avifs、jxl、jls 等是否同样漏注册(以应用实际可解码集合为准,与 decode 通道清单比对)。
2. 卸载钩子同步清理新增的关联项。
3. 有条件时本地跑一次安装/卸载,验证「打开方式」出现与清理。

## 验收

- 安装后 AVIF(及补上的其他格式)右键打开方式出现本应用;卸载后注册表无残留。
- 清单与 decode 实际支持格式一致(在 nsh 或 CI 注释中注明与哪个模块对齐)。

## Comments

- 2026-10-24 已实施：installer-hooks.nsh 补 avif（注册宏 / Capabilities FileAssociations / 卸载清理三处）；头部真源指针修正为 decode/format.rs（ASSET_EXTS + RAW_EXTS + TIFF_EXTS + JXL_EXTS）。装机人工验证（右键打开方式出现/卸载清理）留待下次打包时确认。
