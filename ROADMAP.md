# Musage 当前路线图

> Status: **2026-06-24 全量审查完成** — v0.2.0 (2026-06-22) 已发布,P1/P2-A/P2-B 实际完成度逐项核对见下表。
> 详尽历史见 git log + 旧 ROADMAP.md(已删除,内容并入 [docs/codeplan/](docs/codeplan/))。

## Recent (已完成,留作历史)

**Phase: 扩展 quota source 到 13 个 + CustomSource + i18n P0-P2**(~9-12 天,2026-06-13~2026-06-17)

| PR | 内容 | 时间 | 状态 |
|---|---|---|---|
| PR 1 | Kimi / 智谱 + Xiaomi 一键登录 + 退避 (commits `0ae07d0`~`8c8b749`) | 0.5 天 | ✅ Done |
| PR 2 | 4 个新 Bearer provider + Claude 官方 Cookie + 5 个 logo (commits `aabe823`~`8790596`) | 3 天 | ✅ Done |
| PR 3 | CustomSource + 13 内置 + 设置面板重构 + i18n P0-P2 (commits `68b5dff`~`72e86cc`) | 4-5 天 | ✅ Done |
| **v0.2.0 cleanup** | 删 Provider enum + 砍 Novita/Qwen STUB + 统一 IPC + 修 31+10 broken test + CI 收紧(commits `b253720`~`f800c8e`) | 1 天 | ✅ Done |
| **PR 1b Extra Instance** | 内置 provider 副本 + 统一 extra_instances 持久化 + 复制按钮预选 (commits `c6ce2be`/`1985250`) | 0.5 天 | ✅ Done |
| **v0.2.1 batch**(2026-06-24) | 7 个 commit:`docs(sync)` / `inline custom_sources wrapper` / `浮窗 unique_id` / `list_picker display_name` / `tray tooltip #N` / `批量粘贴 + New API callout` / `系统通知 + import/export` | 1 天 | ✅ Done |

详细 plan / 关键发现见 [docs/codeplan/2026-06-15-extend-providers.md](docs/codeplan/2026-06-15-extend-providers.md)。

## Active (2026-06-24 审查后实际状态)

**Phase: 文档同步 + tech debt 收尾**(~1.5 人天,2026-06-24 一个批次 7 commit)

2026-06-24 用 7 维度 Explore 子 agent 核查代码现状,对照旧 P1/P2-A/P2-B 表重新对照实际完成度:

| 原计划 | 项 | 状态 | 备注 |
|---|---|---|---|
| **P1** | 浮窗位置跨屏感知 | ⏳ | 自承认在 Next,不算 P1 实做 |
| **P1** | 首启空态 | ✅ | commit `5b976e2` / `bbaa56f` |
| **P1** | 错误态一键恢复按钮 | ✅ | `src/main.ts:585-608` 按 error_kind 分发 4 按钮 + 倒计时 |
| **P1** | 关键 i18n 收尾 | ⚠️ ~95% | 残留 `<5%`(`types.ts` 6 行 / `credentials.ts:307/387` / `updater.ts:90`) |
| **P2-A** | 批量粘贴 key 自动匹配 | ✅ (commit 6) | `credentials.ts` 加 batch textarea + 前缀识别 |
| **P2-A** | New API preset 显眼化 | ✅ (commit 6) | `extra-instance-form.ts` 加 callout |
| **P2-A** | 错误恢复完整版 | ⏳ | 仅 P1-3 按钮本体,无额外恢复路径(自动重试 / 复制错误 / 忽略等) |
| **P2-B** | Xiaomi/Claude 8h 失效系统通知 | ✅ (commit 7) | `tauri-plugin-notification` + 60s 去重 + 仅 xiaomi/claude |
| **P2-B** | 一键重登(4 跳 → 1 跳) | ⚠️ 半完成 | Xiaomi ✅(commit `c561c2e`),Claude ❌(仍多跳,需研究 cookie 抓取) |
| **P2-B** | import/export(无 keys) | ✅ (commit 7) | `advanced.ts` 加 Import/Export section,纯 web 实现 0 新 dep |

## Next (等下个 phase 推进)

**v0.2.1 已做但部分遗留 + 新需求**:
- 错误恢复完整版(P2-A-7 增量:复制错误 / 打开日志 tab / 忽略本次)
- Claude cookie 一键重登(P2-B-9 增量)
- 浮窗位置跨屏感知完整版(monitor hotplug)

**FUTURE / 产品方向**:
- 多 provider 限速(FUTURE.md "2-3 天")
- 配额预警通知(系统通知,80% 阈值 — 现在有 notification 插件了,门槛大幅降低)
- Export raw JSON(给"今日消耗"用户替代方案)
- 多语言 ja-JP(FUTURE.md "1 周",UI 已 i18n 化,加 1 个 JSON)
- Frontend 单元测试 4 核心函数(contentFingerprint / render / updateCard / autoResizeWindow)

## Later

见 [FUTURE.md](FUTURE.md)。

## Tech debt (2026-06-24 全量审查刷新)

### Critical(已修 ✅)
- ✅ `cargo check` 20 warnings — v0.2 cleanup PR (`f800c8e`/`6e90518`/`54a5554`) 砍 `set_state` / `region` field / Provider enum 等 dead code;剩 `#[allow(dead_code)]` 2 处是 v2 预留
- ✅ 31/193 cargo test 失败 → v0.2 后 **188/188 全绿**(commit `de5185f` 修 10 broken test + 23 i18n assertion + 1 production i18n bug)

### High(2026-06-24 审查后实际状态)
- ⏳ 13 provider 错误分类不统一(401/403/429 各自映射不同),缺 `http_status_to_error_kind` helper — **留 v0.3**
- ✅ ~~`Provider::Minimax` 占位散落 7+ 处~~ — v0.2 删 enum 解决
- ⏳ `refresh_inner` 每次 `Box::new` 13 个 source(`[memory/source-instance-rebuild-footgun]` 已知未修) — **留 v0.3**
- ⏳ Backoff 状态不持久化到 disk,重启后 30min 退避归零 — **留 v0.3**
- ⏳ Per-provider poller task 无 shutdown signal,App 退出时可能泄漏 — **留 v0.3**
- ✅ `refresh_single_inner` miss 时返硬编码中文 — `commands/mod.rs:1267` 已 `t!("error.common.unknown_source_id")`
- ⏳ Frontend 0 单元测试(contentFingerprint / render / updateCard / autoResizeWindow) — **留 v0.3**

### Medium(v0.2 顺手修的留 v0.3)
- ✅ ~~`src-tauri/src/commands/mod.rs:984-988` 硬编码中文 "未知的 source id"~~ — 已修(见 High 第 5 条)
- ⏳ `src/settings/source-extras.ts` 7 个 `renderXxx` 函数未完全数据驱动
- ⏳ `error.provider.minimax_403` 孤例 i18n key(枚举已删后此 key 留做历史兼容)
- ⏳ `Uuid::new_v4().simple()` UUID 路径散落,提到顶层 `format_id()`

### Low(已修 ✅ 或永不做)
- ✅ ~~"Provider enum 已被 QuotaSource trait 替代,dump CLI 还在走 enum 路径"~~ — **v0.2 已删 enum**
- ✅ ~~"src/settings.html 还在按 provider id hardcode"~~ — **已过期**:PR 3 完成后已动态化
- ✅ ~~"Novita / Qwen STUB 永久返 ServerError"~~ — **v0.2 已删**
- ✅ ~~"AGENTS.md 写 PR 1b 已知限制(PR 3 解决)"~~ — **v0.2.1 commit 3/4/5 全做**
- ✅ ~~"前端 i18n `provider_name.*` 11 项与后端镜像"~~ — **v0.2.1 commit 4 后端返 display_name**
- ✅ ~~"`config/custom_sources.rs` 缩成 `load_or_migrate` wrapper"~~ — **v0.2.1 commit 2 内联到 lib.rs**

## 文档分层(给新会话的 agent)

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
