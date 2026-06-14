# Changelog

All notable changes to Musage will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Kimi (Moonshot Coding)** 套餐源（commit `0ae07d0`）
- **智谱 GLM** 套餐源，支持国内/国际 endpoint 切换（commit `0ae07d0` + `4aedaef`）
- **Xiaomi MiMo 一键登录**：应用内 WebView 自动提取 Cookie（commit `c561c2e`）
- **Xiaomi 多鉴权 fallback**：Bearer→Cookie 自动降级，"丢个 API key 就跑"（commit `d232b31`）
- **Per-provider 指数退避**：429/5xx 翻倍间隔，30 分钟上限（commit `77bd65d`）
- **Xiaomi 浮窗显示模式 3 档选择器**：完整 / 只套餐 / 只总额度（commit `2c6d2d7`）
- **首字母 fallback logo**：新 provider 不写 SVG 也能跑（commit `6d3d822`）
- **顺序区按 enabled/disabled 分区**：浮窗卡片顺序更清晰（commit `8c8b749`）

### Fixed
- 浮窗高度上限 800 装不下 8+ provider，提到 2400 + 屏幕工作区兜底（commit `4a707ea`）
- 重置时间 > 1 天时显示日期 + (N天) 而不是 321h30m（commit `7b92406`）
- Kimi 月牙 / 智谱 Z mark 手写路径错位（commit `30e5257`）
- Xiaomi 月度 → 总额度，套餐和总额度相等时合并为一行（commit `55be30b`）
- CI #12 的 rust brotli 冲突 + 3 个 tsc 严格模式错（commit `d73aa80`）
- CI 绕开 setup-node@v4.4.0 cache 机制（commit `879de07`）

### Changed
- 浮窗默认显示模式从 All 改为 TotalOnly（commit `c3577f1`）
- 托盘菜单"显示/隐藏悬浮窗"合并为单个"切换悬浮窗"（commit `3d08b02`）
- 托盘菜单"强制置顶浮窗（Win 逃生口）"文案 → "置顶一下"（commit `4c14913`）

## [0.1.0] - 2026-06-13

首发版本。

### Added
- 桌面悬浮窗 + 系统托盘
- 6 个 quota source：
  - **MiniMax** (Bearer, Token Plan 5h/周)
  - **DeepSeek** (Bearer, 余额)
  - **Xiaomi MiMo** (Cookie, 套餐+总额度)
  - **Tavily** (Bearer, Credits)
  - **ZenMux** (Bearer, PAYG/Subscription)
  - **OpenRouter** (Bearer, 余额)
- Tauri 2 + Windows NSIS 安装包（2.5 MB）
- 本地 `keys.json` 存储（0600 权限）
- 后台 tokio 轮询
- CLI `musage dump` 子命令（独立验证 schema）
- 托盘动态图标（颜色随用量变：绿/橙/红）
- 文档：README / AGENTS / ROADMAP

### Known limitations
- 8 个 provider 平铺在设置面板，UI 待重构
- 浮窗卡片无位置记忆
- Windows 端 PinBottom 模式 hover-raise 是 best-effort（见 AGENTS.md）

[Unreleased]: https://github.com/Thedeergod666/Musage/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Thedeergod666/Musage/releases/tag/v0.1.0
