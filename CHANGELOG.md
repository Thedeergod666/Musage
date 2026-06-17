# Changelog

All notable changes to Musage will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Kimi (Moonshot Coding)** 套餐源 (commit `0ae07d0`)
- **智谱 GLM** 套餐源,支持国内/国际 endpoint 切换 (commit `0ae07d0` + `4aedaef`)
- **Xiaomi MiMo 一键登录**:应用内 WebView 自动提取 Cookie (commit `c561c2e`)
- **Xiaomi 多鉴权 fallback**:Bearer→Cookie 自动降级,"丢个 API key 就跑" (commit `d232b31`)
- **Per-provider 指数退避**:429/5xx 翻倍间隔,30 分钟上限 (commit `77bd65d`)
- **Xiaomi 浮窗显示模式 3 档选择器**:完整 / 只套餐 / 只总额度 (commit `2c6d2d7`)
- **首字母 fallback logo**:新 provider 不写 SVG 也能跑 (commit `6d3d822`)
- **顺序区按 enabled/disabled 分区**:浮窗卡片顺序更清晰 (commit `8c8b749`)
- **5 个新 built-in 套餐源**:StepFun / SiliconFlow / Novita / Qwen / Claude 官方 (commit `aabe823`)+ 官方 logo 资源 (commit `a073186`)
- **CustomSource (New API 中转站)**:用户可加任意 New API 系中转,3 选 1 extract 模板,JSONPath 解析,UUID 持久化,覆盖 10+ 国内中转站 (commit `68b5dff` + `b234e45` + `bb5151c`)
- **设置面板重构**:6 组分组 (token_plan/balance/official/xiaomi/custom/misc) + 顶部搜索 + 原生 `<dialog>` modal + ↑/↓ 跨段排序 (commits `bb5151c` / `78e10bb` / `174b5de`)
- **浮窗卡片顺序交互**:↑/↓ 跨段 + 分隔线可拖 + 全隐藏时保留分隔线 (commits `78e10bb` / `271d9c2` / `0b661e6` / `2a5faca`)
- **i18n 架构 P0**:后端 rust-i18n + 前端自写 helper + `set_app_locale` IPC + `musage://locale-changed` 事件,设置面板加语言切换 UI (commits `14f9890` / `2f3a06d`)
- **i18n 字符串 P1-P2**:`t!()` / `t()` 覆盖 tray/tooltip/CLI/13 provider 错误/settings.html 主面板/5 个 section render/PROVIDER_META 三处合一 + 6 个分组标题 + credentials/groups/order/test/modal/custom-source-form/source-extras/providers/advanced/floating/logs/updater/region-wizard 全部硬编码中文 (commits `36eaa2b`–`c693de1` + `34b4181` + `73ad719` + `5d372c8`–`72e86cc`)
- **i18n 收尾**:`src/i18n/{en,zh-CN}.json` 补齐 settings 面板所有缺失的 i18n keys (commit `4bc9f56`)
- **错误分类重构**:从子串匹配改 ErrorKind enum 透传,i18n 与错误分类解耦 (commit `5401cb7`)
- **区域向导**:`set_region` command + UI,设置面板 region 选项独立管理 (commit `e9a2139`)
- **4 档语义色 + 钱包告警阈值可调** + 全部重置按钮 (commits `0a6a455` / `107e11b`)
- **托盘 percent 模式字号 14→16→18→20**(布局极限,commits `63490ef` / `70b4736` / `7b6289d`)
- **浮窗位置记忆**(80% 完成):`WindowEvent::Moved`/`Resized` → debounced persister + 启动恢复

### Fixed
- 浮窗高度上限 800→2400 + 屏幕工作区兜底 (commit `4a707ea`)
- 重置时间 > 1 天时显示日期 + (N天) 而不是 321h30m (commit `7b92406`)
- Kimi 月牙 / 智谱 Z mark 手写路径错位 (commit `30e5257`)
- Xiaomi 月度 → 总额度,套餐和总额度相等时合并为一行 (commit `55be30b`)
- CI #12 的 rust brotli 冲突 + 3 个 tsc 严格模式错 (commit `d73aa80`)
- CI 绕开 setup-node@v4.4.0 cache 机制 (commit `879de07`)
- **状态写回竞态**:整块覆写改为按 provider 合并,避免 data rollback (commit `b567bef`)
- **浮窗 i18n 失效 + 设置面板中英文混杂** (commit `de44915`)
- **i18n 排查发现的 3 个 bug** + 英文 locale 残留中文("提醒"、"智谱"等) (commits `aa48614` / `8e2a19b`)
- **7 个设置子模块 + 多个 i18n 关键 bug**:app.ts / updater.ts 测试按钮 help + fallback error 字符串 (commits `3494c88` / `87c3acb`)
- **3 个 UI 修复**:sticky tab 缝隙泄漏 + 3 个高频组 tab interface + 3 列并排 modal radio (commits `db5b744` / `0378b9f` / `dd318e7`)
- **Kimi/智谱 logo 换真图标** + 智谱浮窗 fallback 首字母 (commit `dfbab89`)
- err-label 移到 dot 左侧,dot 始终固定在卡片右上角 (commit `6165049`)
- 托盘刷新后 Xiaomi 浮窗显示模式被重置回 TotalOnly (commit `f660500`)
- macOS 上 Xiaomi cookie 提取 3 个 bug 一起修 (commit `b7b33ea`)
- ZenMux ErrorKind import 误删 — `unused warning` 误判 (commit `a4c6d91`)
- **CI 跨平台 dist 一致性**:HTML 100% 一致 + Vite `[hash]` 拿掉 + 双重 UTF-8 修复 (commits `707e342` / `8a45787` / `2a7d794`)
- **CI release 修 verify**:`checkout` 显式 + pnpm 钉到 9.15.4 对齐 `packageManager` + 注释掉 `APPLE_*` 走未签名构建 + 死代码清理 (commits `8790596` / `5ef0fe7` / `25b6aa6` / `fa785e7`)
- 多个 CI 诊断 / normalize 脚本抽 `.cjs` (commits `1ab3d67` / `a1c1255` / `519e9e3` / `107ded6` / `fa701bc`)

### Changed
- 浮窗默认显示模式从 All 改为 TotalOnly (commit `c3577f1`)
- 托盘菜单"显示/隐藏悬浮窗"合并为单个"切换悬浮窗" (commit `3d08b02`)
- 托盘菜单"强制置顶浮窗(Win 逃生口)"文案 → "置顶一下" (commit `4c14913`)
- 文档分层重构:ROADMAP 改写 + FUTURE / CHANGELOG 新建 + UPDATER → RELEASING (commit `70e83d3`)
- 协议兼容:3 处 `ProviderConfig` init 补 `xiaomi_display_mode` 字段 (commit `b36e0e6`)
- **13 内置 + N 动态** provider 架构:`QuotaSource` trait + `builtin_sources()` 工厂 + `CustomSource` 注册到 `all_sources()` (PR 3 整体)

### Tech debt (待 PR-2026-06 cleanup 修)
- `cargo check` 20 warnings(dead code: `set_state` 11× / `region` field 1× / `client` 不一致)
- 13 provider 错误分类不统一(401/403/429 各自映射不同),缺 `http_status_to_error_kind` helper
- `Provider::Minimax` 占位散落 7+ 处([memory/tavily-enum-placeholder-footgun] 警示踩坑已发生,CustomSource 必踩)
- `refresh_inner` 每次 `Box::new` 13 个 source([memory/source-instance-rebuild-footgun] 已知未修)
- Backoff 状态不持久化到 disk,重启后 30min 退避归零
- Per-provider poller task 无 shutdown signal,App 退出时可能泄漏
- `refresh_single_inner` miss 时返硬编码中文(regression of `8e2a19b` 的修法)
- Frontend 0 单元测试(contentFingerprint / render / updateCard / autoResizeWindow 4 个核心全靠手测)
- 详情见下次 review 报告 → `/private/tmp/claude-501/.../whbtppp70.output`

## [0.1.0] - 2026-06-13

首发版本。

### Added
- 桌面悬浮窗 + 系统托盘
- 6 个 quota source:
  - **MiniMax** (Bearer, Token Plan 5h/周)
  - **DeepSeek** (Bearer, 余额)
  - **Xiaomi MiMo** (Cookie, 套餐+总额度)
  - **Tavily** (Bearer, Credits)
  - **ZenMux** (Bearer, PAYG/Subscription)
  - **OpenRouter** (Bearer, 余额)
- Tauri 2 + Windows NSIS 安装包(2.5 MB)
- 本地 `keys.json` 存储(0600 权限)
- 后台 tokio 轮询
- CLI `musage dump` 子命令(独立验证 schema)
- 托盘动态图标(颜色随用量变:绿/橙/红)
- 文档:README / AGENTS / ROADMAP

### Known limitations
- 8 个 provider 平铺在设置面板,UI 待重构
- 浮窗卡片无位置记忆
- Windows 端 PinBottom 模式 hover-raise 是 best-effort(见 AGENTS.md)

[Unreleased]: https://github.com/Thedeergod666/Musage/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Thedeergod666/Musage/releases/tag/v0.1.0
