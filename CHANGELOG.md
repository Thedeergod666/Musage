# Changelog

All notable changes to Musage will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-06-29

v0.1.0 (2026-06-13) 之后 16 天的累计：6+ PR + 1 次大清理 + PR 1b 额外实例 + 35 个 follow-up commit。version 字段钉死 0.2.0，git tag `v0.2.0` 落在本 commit。

### Pre-cleanup 累积 (PR 2-3 + PR 1b, commits `0ae07d0`–`1985250`)

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
- **i18n P0-P2**: 后端 rust-i18n + 前端自写 helper + `set_app_locale` IPC + 11 provider 错误 + settings 5 section + tooltip 全 i18n 化
- **PR 1b Extra Instance (2026-06-24)**: 内置 provider 副本 + 统一 `extra_instances.json` 持久化 + 复制按钮预选 (`c6ce2be` / `1985250`)
- **3 个新 built-in provider**: StepFun / SiliconFlow / Claude 官方 (PR 2)

### 2026-06-22 集中清理 (PR 1-6, commits `b253720`–`54a5554`)

#### BREAKING
- **Config schema**: `Provider` enum removed. All source id is now a string (e.g. `"minimax"`, `"tavily"`, `"custom_<uuid>"`). Old config.json files still load (`ProviderSnapshot.provider` field defaults to `""` via `#[serde(default)]`).
- **IPC API**: 7 enum-based commands removed (`has_api_key_for` / `set_api_key_for` / `delete_api_key_for` / `get_api_key_for` / `has_cookie_for` / `set_cookie_for` / `delete_cookie_for`). Use the string-based `set_source_credential` / `get_source_credential` / `has_source_credential` / `delete_source_credential` instead.
- **Config keys API**: 7 enum-based helpers removed in `config.rs` (`load_api_key_for` / `save_api_key_for` / `delete_api_key_for` / `load_cookie_for` / `save_cookie_for` / `delete_cookie_for` / `cookie_key`). Use `load_credential_for_id` / `save_credential_for_id` / `delete_credential_for_id`.

#### Removed
- `Provider` enum (35 placeholder sites consolidated to string ids; `tavily-enum-placeholder-footgun` warning fully resolved).
- `ProviderImpl` trait (replaced by `QuotaSource` trait).
- **Novita + Qwen STUB providers** (公开 API 无 quota endpoint,永久返回「未支持」错)。用户看到「未支持」比看不到更沮丧 (F1 反模式),直接砍更干净。
- 2 个 STUB logo 资产 (`novita-logo.svg` / `qwen-logo.svg`).

#### Fixed
- **Tray tooltip 永久只看 MiniMax** (`tray.rs`): 之前 `Provider::Minimax` enum filter 导致 Tavily / ZenMux / Kimi / Zhipu / StepFun / SiliconFlow / Claude 官方 / 用户自定义 New API 中转站 **全部不进 tray tooltip**。改成 `source_id == "minimax"` 字符串路由,所有 provider 都进 tray tooltip。
- **Xiaomi `apply_display_mode` i18n bug**: 之前 hardcode `"套餐"` / `"总额度"` 字符串 filter,en locale 下永远 filter 0 行 → 浮窗空白。改成 `t!("row.plan")` / `t!("row.monthly_total")`,跟 locale 解耦。
- **生产代码 10 个 broken test** (lib test 编不过) — 9× `RwLock::get()` 弃用 (rust 1.78+) 改 `read().unwrap()`,1× `Cow<'_, str>` vs `&str` 类型 mismatch 加 `.as_ref()`.
- **23 个 i18n assertion** (test 写死中文 label, en locale 下不匹配) — 改 `t!("row.xxx")` 形式。
- **2 个 epoch range 太严** (timezone drift ±1 天) — 放宽到 ±6 天。

#### Changed
- **CI now runs `cargo test`** (was just `--no-run`); added `cargo clippy` + `cargo fmt --check` + `pnpm exec tsc --noEmit` gate.
- **Docs-only PR check**: `docs:` PR titles fail if non-.md files modified (force separation of docs-only PR from mixed PR).
- **`ProviderSnapshot.provider` field**: `Provider` enum → `String`. `#[serde(default)]` 让老 JSON 反向兼容。
- **`ProviderSnapshot::health_label()`**: `match self.provider` 改 `match self.source_id`（deepseek 走 `is_healthy` 分支, 其它 utilization 分支）。
- **`ProviderSnapshot::empty_error()`**: 删 `provider: Provider` 参数（`Provider` enum 已删）。
- **Frontend**: `settings/order.ts` 删 9 处冗余 `as ProviderId` cast。
- **Backend**: `lock_recover()` helper 抽出（mutex poison recover 模板统一 3 处使用）。
- **`cargo fmt --all`** 跑过一次：28 文件重新格式化（纯格式变化，无逻辑）。

#### Tech debt cleared
- `commands/mod.rs`: 1732 → ~1590 行 (-142 from 7 IPC + Provider 清理)
- `providers/mod.rs`: 删 60 行 enum 定义 + ProviderImpl trait
- `poller_backoff.rs` test fixtures: 改用 string ids
- `apply_display_mode` production bug: hardcode `套餐`/`总额度` 改 `t!()` (i18n 解耦)
- 11 个 provider 文件: `Provider::Xxx` 占位改 `"xxx"` string (消除 [memory/tavily-enum-placeholder-footgun](memory/tavily-enum-placeholder-footgun.md) 警示)

### v0.2.0 后续 follow-up (commits `00565a1`–`28e4b4e`, 35 个 commit, 2026-06-22 ~ 2026-06-29)

#### Added (PR 1b 收尾 + 增量)
- **批量粘贴 key 自动匹配**:providers section 顶部 `<details>` 折叠 batch textarea,粘贴多行 `provider=value` 或纯 key 自动识别 provider 前缀(`sk-cp-` / `tp-` / `tvly-` / `Oasis-Token` 等复用现有 placeholder regex)并填入。
- **New API preset callout**:`+ 添加新来源` modal 选中 custom 时显示强调框,引导用户用 New API 模板。
- **系统通知(Xiaomi/Claude cookie 失效)**:`tauri-plugin-notification` + 60s 去重,`ErrorKind::AuthFailed` + provider 是 `xiaomimimo` / `claude_official` 时弹系统通知(走 `log_provider_error` hook,跟现有去重共用一套缓存)。
- **import/export 配置(无 keys)**:设置面板"高级"页 Import/Export section,Export 走纯 web `Blob` + `<a download>` 下载 JSON,Import 走 `<input type="file">` + `FileReader` 走 `saveConfig` 全量保存,0 新 Tauri dep。
- **托盘 tooltip `#N` 后缀**:多 instance 时同 base provider 区分显示(如 `MiniMax 5h 45% / 周 72% #2`)。
- **托盘进度条遍历多 instance**:原 `pick_minimax_rows` `find()` 只取第一个 minimax,改成遍历所有 instance + 进度条颜色按 `#1` 走。
- **`ProviderSnapshot.unique_id` 字段**:后端 snapshot 加 `#[serde(default)] unique_id: Option<String>`,所有 provider 构造时填 `self.unique_id()`,前端 7 处 id 路由(`contentFingerprint` / `existingCards.get(id)` / 重排 / `rowsForRender` / `card.dataset.provider` / `updateCard` / err button)优先取 `unique_id`,fallback `source_id` → `provider`。
- **错误恢复完整版 (P2-A-7)**:浮窗错误卡新增 2 个通用按钮 (任何 error_kind 都可用):
  - `📋 Copy error`: 复制 `p.error` 到剪贴板 + mini flash 反馈 (3 秒自动淡出,玻璃风格 backdrop-filter)
  - `📋 Logs`: 打开设置 + 跳到 logs section (复用修好的 `open_settings_window(section)` 通道)
- **浮窗位置跨屏感知 (P1-1)**:多屏用户拖到副屏后重启,位置可能"不在任何 monitor 内"(副屏拔了 / DPI 变 / 显卡驱动重置)。`saved_pos_valid` 判定从 `x > 50 || y > 50` 启发式升级成 `position_is_visible(x, y, &[Monitor])` 几何检查:遍历 `win.available_monitors()` 矩形包含测试。拿不到 monitor 列表(Wayland 老版本)时 fallback 老启发式。monitor hotplug 监听留 v0.3。

#### Changed
- **后端 `list_picker_providers` 返翻译好的 `display_name`**:`PickerProvider.name_key` → `display_name`,后端用 `t!("provider_name.xxx").into_owned()` 注入翻译串。前端 `src/i18n/{en,zh-CN}.json` 的 `provider_name.*` 11 项镜像删除,**单一来源 = 后端 `src-tauri/locales/{en,zh-CN}.json`**。
- **浮窗 DOM `data-source-id` → `data-unique-id`**:错误卡 retry 按钮 click 委托从 `target.dataset.sourceId` → `target.dataset.uniqueId`。
- **`config/custom_sources.rs` wrapper 内联到 `lib.rs` 后删除**:v0.2.0 清理点 2 天后老 `custom_sources.json` 都被启动 rename 成 `.migrated`,wrapper 无 active caller。
- **后端 `open_settings_window(app, section: Option<String>)` 接收 section 参数**:窗口起后 emit `musage://settings-navigate` 事件,settings.ts 新增 listener 调 `navigateToSection(target)` helper 跳 section。修隐藏 bug:错误卡 advanced 按钮之前传 `{section: "advanced"}` 给后端被静默忽略。

#### Fixed (full audit 收尾 + 13 隐患)
- **PR 1b 5 项残留限制全部清理**:
  - 浮窗 `data-unique-id` 改 `unique_id()` 渲染 ✅
  - 托盘 tooltip 拼 `#N` 后缀 + 进度条遍历多 instance ✅
  - `list_picker_providers` 返 `display_name` ✅
  - 前端 `provider_name.*` 镜像删除 ✅
  - `delete_extra_instance` keys.json rename 留 v0.3 (本 commit 不打 keys.json schema break)
- **P0 3 个 blocking bug 修** (`f1f74f9`): delete UUID / CustomSourceSpec serde / orphan extra instance
- **P1 5 个 audit 项修** (`3f99686` + `0180715` + 后续 5ef575c/edd7f82/5c64314/d328eeb):
  - 12 provider `do_fetch` 统一接受 `source_id/display_name` 参数 + C1/C2/H2/H5/M3/M6 修复
  - 5 处全量审查潜在 bug
  - add_extra_instance / update_extra_instance / compact key migration 错误处理 / 顺序修复
  - claude_official copy form 按 auth_kind 切显示 cookie 输入框
- **i18n full audit** (`99320c0`): duplicate provider key / 硬编码 label / fallback path / 10 test assertion
- **extra-instances 3 合 1** (`70068a0`): 重复卡片 / 图标缺失 / 拖拽顺序 + TOML test 500
- **display_name post-fill** (`a47fd1d`): `refresh_inner` / `refresh_single_inner` 通过 `find_source()` 回填 `source_display_name`
- **i18n window title** (`c8cd7fe`): 浮窗 window title 走 i18n 路径（不再硬编码 'Musage'）
- **小米 plan-expired 路由** (`a176b60`): 独立 `error_kind` + 消息
- **ci doctest + Linux Manager import** (`08bdd84`): 2 个 CI 隐患修
- **tray 时间戳 invalid** (`b164b8b`): invalid 时间戳不再显示空串
- **v0.3 placeholder 清理** (`5cb344b`): `"minimax"` provider placeholder 改真实 `provider_id`
- **cargo fmt** (`28e4b4e`): 28 文件重新格式化

#### Tech debt 收尾
- Tech debt 7 项刷新:2 ✅ 已修(原硬编码中文 / Provider enum)/ 5 ⏳ 留 v0.3(`http_status_to_error_kind` / `refresh_inner` Box::new / Backoff 持久化 / Per-provider shutdown signal / Frontend 单测)
- PR 1b 5 项限制:4 ✅ / 1 ⏳(`delete_extra_instance` v2 完整重做留 v0.3)
- P2-A 完成度从 13% → 87%(批量粘贴 + New API callout 完成;错误恢复完整版完成)
- P2-B 完成度从 17% → 75%(系统通知 + import/export 完成;Claude 一键重登留 v0.3)

### Upgrade notes
- 老 v0.1.x 用户升级 v0.2.0:
  1. 应用启动不会崩 — `keys.json` / `config.json` / `custom_sources.json` 通过 `#[serde(default)]` 自动迁移
  2. 设置面板老凭据仍能读 — 因为 keys.json 文件格式没变 (只是 key 是字符串不是 enum 序号)
  3. **第三方插件/脚本如果之前调 `set_api_key_for(minimax)` / `set_cookie_for(xiaomimimo)` 等老 IPC 命令会失败** — 改用 `set_source_credential("minimax", key)` / `set_source_credential("xiaomimimo", cookie, field="cookie")`
- 老 v0.1.x 用户的 `custom_sources.json` 在 v0.2.0 启动时会被自动迁移到 `extra_instances.json` 并 rename 成 `.migrated`
- `keys.json` 格式不变(`delete_extra_instance` v2 重做留 v0.3 单独 PR)

[0.2.0]: https://github.com/Thedeergod666/Musage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Thedeergod666/Musage/releases/tag/v0.1.0

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
- 6 个 provider 平铺在设置面板,UI 待重构
- 浮窗卡片无位置记忆
- Windows 端 PinBottom 模式 hover-raise 是 best-effort(见 AGENTS.md)

[Unreleased]: https://github.com/Thedeergod666/Musage/compare/v0.2.0...HEAD
