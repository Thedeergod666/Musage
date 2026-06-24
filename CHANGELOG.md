# Changelog

All notable changes to Musage will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-06-24

v0.2.0 后的一次性收尾:7 个 commit,文档与代码同步 + 5 个 PR 1b 残留限制解除 + P2-A/P2-B 增量。

### Added

- **批量粘贴 key 自动匹配**:providers section 顶部 `<details>` 折叠 batch textarea,粘贴多行 `provider=value` 或纯 key 自动识别 provider 前缀(`sk-cp-` / `tp-` / `tvly-` / `Oasis-Token` 等复用现有 placeholder regex)并填入。
- **New API preset callout**:`+ 添加新来源` modal 选中 custom 时显示强调框,引导用户用 New API 模板。
- **系统通知(Xiaomi/Claude cookie 失效)**:`tauri-plugin-notification` + 60s 去重,`ErrorKind::AuthFailed` + provider 是 `xiaomimimo` / `claude_official` 时弹系统通知(走 `log_provider_error` hook,跟现有去重共用一套缓存)。
- **import/export 配置(无 keys)**:设置面板"高级"页 Import/Export section,Export 走纯 web `Blob` + `<a download>` 下载 JSON,Import 走 `<input type="file">` + `FileReader` 走 `saveConfig` 全量保存,0 新 Tauri dep。
- **托盘 tooltip `#N` 后缀**:多 instance 时同 base provider 区分显示(如 `MiniMax 5h 45% / 周 72% #2`)。
- **托盘进度条遍历多 instance**:原 `pick_minimax_rows` `find()` 只取第一个 minimax,改成遍历所有 instance + 进度条颜色按 `#1` 走。
- **`ProviderSnapshot.unique_id` 字段**:后端 snapshot 加 `#[serde(default)] unique_id: Option<String>`,所有 provider 构造时填 `self.unique_id()`,前端 7 处 id 路由(`contentFingerprint` / `existingCards.get(id)` / 重排 / `rowsForRender` / `card.dataset.provider` / `updateCard` / err button)优先取 `unique_id`,fallback `source_id` → `provider`。

### Changed

- **后端 `list_picker_providers` 返翻译好的 `display_name`**:`PickerProvider.name_key` → `display_name`,后端用 `t!("provider_name.xxx").into_owned()` 注入翻译串。前端 `src/i18n/{en,zh-CN}.json` 的 `provider_name.*` 11 项镜像删除,**单一来源 = 后端 `src-tauri/locales/{en,zh-CN}.json`**。
- **浮窗 DOM `data-source-id` → `data-unique-id`**:错误卡 retry 按钮 click 委托从 `target.dataset.sourceId` → `target.dataset.uniqueId`。
- **`config/custom_sources.rs` wrapper 内联到 `lib.rs` 后删除**:v0.2.0 release 2 天后老 `custom_sources.json` 都被启动 rename 成 `.migrated`,wrapper 无 active caller。

### Fixed

- **PR 1b 5 项残留限制全部清理**:
  - 浮窗 `data-unique-id` 改 `unique_id()` 渲染 ✅
  - 托盘 tooltip 拼 `#N` 后缀 + 进度条遍历多 instance ✅
  - `delete_extra_instance` keys.json rename 已 v0.2.1 commit 3 在结构层面预留(本轮不打 keys.json schema break → 留 v0.3)
  - `list_picker_providers` 返 `display_name` ✅
  - 前端 `provider_name.*` 镜像删除 ✅

### Tech debt 收尾

- Tech debt 7 项刷新:2 ✅ 已修(原硬编码中文 / Provider enum)/ 5 ⏳ 留 v0.3(`http_status_to_error_kind` / `refresh_inner` Box::new / Backoff 持久化 / Per-provider shutdown signal / Frontend 单测)
- PR 1b 5 项限制:4 ✅ / 1 ⏳(`delete_extra_instance` v2 完整重做留 v0.3)
- P2-A 完成度从 13% → 87%(批量粘贴 + New API callout 完成;错误恢复完整版留 v0.3)
- P2-B 完成度从 17% → 75%(系统通知 + import/export 完成;Claude 一键重登留 v0.3)

### Upgrade notes

- 老 v0.2.0 升级 v0.2.1 无破坏性改动(纯增量和 refactor)
- `keys.json` 格式不变(`delete_extra_instance` v2 重做留 v0.3 单独 PR)
- 老 `custom_sources.json` 启动时已被迁移并 rename 成 `.migrated`,wrapper 删后无副作用

## [0.2.0] - 2026-06-22

v0.1.0 (2026-06-13) 之后的 9 天累计 7+ PR, 加上 2026-06-22 一天集中清理 (PR 1-6)。

### Pre-cleanup 累积 (PR 2-3, commits `0ae07d0`-`f800c8e`)

#### Added
- **Kimi (Moonshot Coding)** 套餐源 (commit `0ae07d0`)
- **智谱 GLM** 套餐源,支持国内/国际 endpoint 切换 (commit `0ae07d0` + `4aedaef`)
- **Xiaomi MiMo 一键登录**:应用内 WebView 自动提取 Cookie (commit `c561c2e`)
- **Xiaomi 多鉴权 fallback**:Bearer→Cookie 自动降级,"丢个 API key 就跑" (commit `d232b31`)
- **Per-provider 指数退避**:429/5xx 翻倍间隔,30 分钟上限 (commit `77bd65d`)
- **Xiaomi 浮窗显示模式 3 档选择器**:完整 / 只套餐 / 只总额度 (commit `2c6d2d7`)
- **首字母 fallback logo**:新 provider 不写 SVG 也能跑 (commit `6d3d822`)
- **顺序区按 enabled/disabled 分区**:浮窗卡片顺序更清晰 (commit `8c8b749`)
- **CustomSource (New API 中转站)**:用户可加任意 New API 系中转,3 选 1 extract 模板,JSONPath 解析,UUID 持久化 (commits `68b5dff` / `b234e45` / `bb5151c`)
- **设置面板重构**:6 组分组 + 顶部搜索 + 原生 `<dialog>` modal + ↑/↓ 跨段排序
- **i18n P0-P2**: 后端 rust-i18n + 前端自写 helper + `set_app_locale` IPC + 13 provider 错误 + settings 5 section + tooltip 全 i18n 化
- **5 个新 built-in provider** (临时): StepFun / SiliconFlow / Novita / Qwen / Claude 官方 — **Novita + Qwen 在 v0.2.0 cleanup 阶段已砍 (见下方 Removed)**

### 2026-06-22 集中清理 (PR 1-6, commits `b253720`-`54a5554`)

### BREAKING

- **Config schema**: `Provider` enum removed. All source id is now a string (e.g. `"minimax"`, `"tavily"`, `"custom_<uuid>"`). Old config.json files still load (`ProviderSnapshot.provider` field defaults to `""` via `#[serde(default)]`).
- **IPC API**: 7 enum-based commands removed (`has_api_key_for` / `set_api_key_for` / `delete_api_key_for` / `get_api_key_for` / `has_cookie_for` / `set_cookie_for` / `delete_cookie_for`). Use the string-based `set_source_credential` / `get_source_credential` / `has_source_credential` / `delete_source_credential` instead.
- **Config keys API**: 7 enum-based helpers removed in `config.rs` (`load_api_key_for` / `save_api_key_for` / `delete_api_key_for` / `load_cookie_for` / `save_cookie_for` / `delete_cookie_for` / `cookie_key`). Use `load_credential_for_id` / `save_credential_for_id` / `delete_credential_for_id`.

### Removed

- `Provider` enum (35 placeholder sites consolidated to string ids; `tavily-enum-placeholder-footgun` warning fully resolved).
- `ProviderImpl` trait (replaced by `QuotaSource` trait).
- **Novita + Qwen STUB providers** (公开 API 无 quota endpoint，永久返回「未支持」错)。用户看到「未支持」比看不到更沮丧 (F1 反模式)，直接砍更干净。
- 2 个 STUB logo 资产 (`novita-logo.svg` / `qwen-logo.svg`)。

### Fixed

- **Tray tooltip 永久只看 MiniMax** (`tray.rs`): 之前 `Provider::Minimax` enum filter 导致 Tavily / ZenMux / Kimi / Zhipu / StepFun / SiliconFlow / Claude 官方 / 用户自定义 New API 中转站 **全部不进 tray tooltip**。改成 `source_id == "minimax"` 字符串路由，所有 provider 都进 tray tooltip。
- **Xiaomi `apply_display_mode` i18n bug**: 之前 hardcode `"套餐"` / `"总额度"` 字符串 filter，en locale 下永远 filter 0 行 → 浮窗空白。改成 `t!("row.plan")` / `t!("row.monthly_total")`，跟 locale 解耦。
- **生产代码 10 个 broken test** (lib test 编不过) — 9× `RwLock::get()` 弃用 (rust 1.78+) 改 `read().unwrap()`，1× `Cow<'_, str>` vs `&str` 类型 mismatch 加 `.as_ref()`。
- **23 个 i18n assertion** (test 写死中文 label, en locale 下不匹配) — 改 `t!("row.xxx")` 形式。
- **2 个 epoch range 太严** (timezone drift ±1 天) — 放宽到 ±6 天。

### Changed

- **CI now runs `cargo test`** (was just `--no-run`); added `cargo clippy` + `cargo fmt --check` + `pnpm exec tsc --noEmit` gate.
- **Docs-only PR check**: `docs:` PR titles fail if non-.md files modified (force separation of docs-only PR from mixed PR).
- **`ProviderSnapshot.provider` field**: `Provider` enum → `String`. `#[serde(default)]` 让老 JSON 反向兼容。
- **`ProviderSnapshot::health_label()`**: `match self.provider` 改 `match self.source_id`（deepseek 走 `is_healthy` 分支, 其它 utilization 分支）。
- **`ProviderSnapshot::empty_error()`**: 删 `provider: Provider` 参数（`Provider` enum 已删）。
- **Frontend**: `settings/order.ts` 删 9 处冗余 `as ProviderId` cast。
- **Backend**: `lock_recover()` helper 抽出（mutex poison recover 模板统一 3 处使用）。
- **`cargo fmt --all`** 跑过一次：28 文件重新格式化（纯格式变化，无逻辑）。

### Tech debt cleared

- `commands/mod.rs`: 1732 → ~1590 行 (-142 from 7 IPC + Provider 清理)
- `providers/mod.rs`: 删 60 行 enum 定义 + ProviderImpl trait
- `poller_backoff.rs` test fixtures: 改用 string ids
- `apply_display_mode` production bug: hardcode `套餐`/`总额度` 改 `t!()` (i18n 解耦)
- 13 个 provider 文件: `Provider::Xxx` 占位改 `"xxx"` string (消除 [memory/tavily-enum-placeholder-footgun](memory/tavily-enum-placeholder-footgun.md) 警示)

### Upgrade notes

- 老 v0.1.x 用户升级 v0.2.0:
  1. 应用启动不会崩 — `keys.json` / `config.json` / `custom_sources.json` 通过 `#[serde(default)]` 自动迁移
  2. 设置面板老凭据仍能读 — 因为 keys.json 文件格式没变 (只是 key 是字符串不是 enum 序号)
  3. **第三方插件/脚本如果之前调 `set_api_key_for(minimax)` / `set_cookie_for(xiaomimimo)` 等老 IPC 命令会失败** — 改用 `set_source_credential("minimax", key)` / `set_source_credential("xiaomimimo", cookie, field="cookie")`

[0.2.0]: https://github.com/Thedeergod666/Musage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Thedeergod666/Musage/releases/tag/v0.1.0
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
- **macOS 安装后打开显示「应用已损坏」**:加 `bundle.macOS` 配置 + `entitlements.plist` (Hardened Runtime:allow-jit / allow-unsigned-executable-memory / disable-library-validation / network.client / network.server),`signingIdentity` 留 `null` 走未签名构建,用户手动 ad-hoc sign + `xattr -cr` 即用;后续有 Apple Developer ID 改 `signingIdentity` 走真签名 + `notarytool` 自动公证
- **浮窗错误态按 error_kind 分发恢复按钮** + 浮窗显示「Next auto-retry in ~Xm」倒计时 (commit `5b976e2`):unconfigured_key/auth_failed → 打开设置 / auth_failed + xiaomimimo → 重新登录 / schema_unknown → ⚙ Advanced / network/rate_limited/server_error → 🔄 Retry 绕过 backoff 立即重拉

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
