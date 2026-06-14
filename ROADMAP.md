# Musage 当前路线图

> Status: 2026-06-15 起。
> 详尽历史见 git log + 旧 ROADMAP.md（已删除，内容并入 [docs/codeplan/](docs/codeplan/)）。

## Active

**Phase: 扩展 quota source 到 15 个 + 自定义 New API source + 设置面板重构**（~9-12 天）

完整 plan: [docs/codeplan/2026-06-15-extend-providers.md](docs/codeplan/2026-06-15-extend-providers.md)

| PR | 内容 | 时间 | 状态 |
|---|---|---|---|
| PR 1 | Phase 0 冒烟 + AGENTS.md 同步 (kimi/zhipu) | 0.5 天 | ⏳ 待开 |
| PR 2 | 4 个新 Bearer provider (stepfun / siliconflow / novita / qwen) + Claude 官方 Cookie | 3 天 | ⏳ 待开 |
| PR 3 | CustomSource + IPC + 设置面板重构（合并）| 4-5 天 | ⏳ 待开 |

**已砍**（移入 [FUTURE.md](FUTURE.md)）：Doubao (IAM 签名 ROI 太低)、JS extractor (启动 +10MB)。

## Next

- **Custom New API 中转站 实战验证**：吃 dmx / byteplus / lemondata 三个真实账号，反向校准 extract 模板
- **AGENTS.md 全面 sync**：当前文件结构树、provider 列表、build 备注都已过期，需要一次完整刷新
- **CHANGELOG 维护节奏**：每次 PR 都该有 entry，参考 Keep a Changelog

## Later

见 [FUTURE.md](FUTURE.md)

## Tech debt

- `Provider` enum 已被 `QuotaSource` trait 替代，`dump` CLI 还在走 enum 路径，需要迁移到 trait
- 前端 `src/settings.html` 还在按 provider id hardcode，需要动态化（被 PR 3 解决）
- `src-tauri/src/api.rs` 描述里的"核心：拉取 + 宽容解析"已不存在（已并入 providers/），doc 描述需同步
- 多个 `renderXxx` 函数（renderRegionSelect / renderBaseUrlInput / ...）在 settings/ 里硬编码，新增 provider 时重复劳动

## 文档分层（给新会话的 agent）

| 文档 | 角色 | 何时读 |
|---|---|---|
| [README.md](README.md) | 给 GitHub 访客 / 潜在用户 | 浏览 repo 时 |
| [AGENTS.md](AGENTS.md) | **新会话的 AI agent** 的 handoff doc | **进入新会话时必读** |
| [ROADMAP.md](ROADMAP.md) | 当前在飞 / 下一个 milestone | 选下一阶段工作时 |
| [FUTURE.md](FUTURE.md) | 砍掉的 / 想法 / 暂缓 | 偶尔回看 |
| [RELEASING.md](RELEASING.md) | 维护者发版 cheat sheet | 准备发版时 |
| [CHANGELOG.md](CHANGELOG.md) | 给用户看的版本变更 | 升级前 / 写 release notes 时 |
| [docs/codeplan/](docs/codeplan/) | 历史 plan + review notes | 接手相关 phase 时 |
| [docs/research/](docs/research/) | 调研报告 | 调研新 provider / API 时 |
