# 11. ADR:商业化分层与门控正式化(含需求文档回写)

Status: resolved
Source: code review 2026-10-24, Spec 轴 #1(核心特色被门控)+ scope creep #1(license 体系)
Severity: P2

## 问题

- 需求文档(`docs/需求报告与技术方案.md` 2.2)把「跨同级文件夹无缝浏览」列为核心特色,且通篇无免费/付费之分;实现上免费版整体禁用(`browse/scan.rs` `open_gated(cross_folder=false)`、`jump_folder` 返回 `pro-required`,已抽查确认)。
- license.rs 1430 行(Ed25519 JWT、设备指纹、在线续验)无任何规格依据;只有 `docs/开发阶段规划.md` 一笔带过。

这是**产品决策**,不是代码缺陷:代码随阶段规划走,规格从未更新。

## 需要人决策的点

1. 免费版/专业版的功能边界清单(跨文件夹浏览归哪边?RAW?JXL?)。
2. 授权体系形态(离线 license?在线续验频率?设备数限制?)是否确认为长期方案。
3. 冲突裁决:修订需求文档以匹配实现,还是调整门控以匹配规格。

## 建议

1. 写 `docs/adr/0002-commercial-tier-gating.md`,记录上述三项决定与理由。
2. 按决定回写 `docs/需求报告与技术方案.md`(2.2 核心特色处标注免费/专业归属)与 README。
3. 完成后 ticket 12、13 引用该 ADR。

## 验收

- ADR 合并;需求文档、README 与实际行为三者一致;后续 review 不再报此矛盾。

## Comments

- 2026-10-24 已裁决并实施：付费边界**维持现状**（跨文件夹浏览 + 预取缓存 = 专业版）。产出 `docs/adr/0002-commercial-tier-gating.md`；需求文档 1/2.2/2.8 同步标注分层归属；README 经核对已与现状一致，无需改动。
