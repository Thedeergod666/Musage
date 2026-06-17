# Musage 当前路线图

> Status: 2026-06-17 起,继承上一阶段所有 PR 已合并的状态。
> 详尽历史见 git log + 旧 ROADMAP.md(已删除,内容并入 [docs/codeplan/](docs/codeplan/))。

## Recent (已完成,留作历史)

**Phase: 扩展 quota source 到 13 个 + CustomSource + i18n P0-P2**(~9-12 天,2026-06-13~2026-06-17)

| PR | 内容 | 时间 | 状态 |
|---|---|---|---|
| PR 1 | Kimi / 智谱 + Xiaomi 一键登录 + 退避 (commits `0ae07d0`~`8c8b749`) | 0.5 天 | ✅ Done |
| PR 2 | 4 个新 Bearer provider + Claude 官方 Cookie + 5 个 logo (commits `aabe823`~`8790596`) | 3 天 | ✅ Done |
| PR 3 | CustomSource + 13 内置 + 设置面板重构 + i18n P0-P2 (commits `68b5dff`~`72e86cc`) | 4-5 天 | ✅ Done |

详细 plan / 关键发现见 [docs/codeplan/2026-06-15-extend-providers.md](docs/codeplan/2026-06-15-extend-providers.md)。

## Active (2026-06-17 起 — Review 后 actionable 计划)

**Phase: 修 critical 残留 + F1 摩擦优化 + tech debt 集中清理**(~14 人天 = 4 PR)

下一阶段分 4 个独立可合 PR,按优先级排序:

| 优先级 | PR | 内容 | 估时 | 摩擦等级 |
|---|---|---|---|---|
| **P0** | 立即修 | 文档同步(本 commit)+ Novita/Qwen STUB default disabled + 31/193 cargo test 修绿 | 1.5 天 | F1 |
| **P1** | 近期做 | 浮窗位置跨屏感知 + 首启空态 + 错误态一键恢复按钮 + 关键 i18n 收尾 | 2.25 天 | F1 |
| **P2-A** | 摩擦优化第一波 | 批量粘贴 key 自动匹配 + New API preset 显眼化 + 错误恢复完整版 | 3.5 天 | F1·必修 |
| **P2-B** | Xiaomi/Claude 8h 失效 | 系统通知 + 一键重登(4 跳 → 1 跳)+ import/export(无 keys) | 3.5 天 | F1.5 |
| **P2-C** | Tech debt 集中清理 | 错误分类统一 helper + set_state 改 `&AppConfig` 共享 + UI 层 vitest + 死代码清理 | 4 天 | F2 |

**详细 actionable 报告** → `/private/tmp/claude-501/-Users-wyh-Project-Musage/adb174f0-c3b5-4953-94d0-ded60423c8da/tasks/whbtppp70.output`(~120KB,7 维度 review + 合成)

## Next (等 P2 完成)

- 多 provider 限速(FUTURE.md "2-3 天")
- 配额预警通知(系统通知,80% 阈值)
- Export raw JSON(给"今日消耗"用户替代方案)
- 多语言 ja-JP(FUTURE.md "1 周",UI 已 i18n 化,加 1 个 JSON)
- 跨屏感知完整版(monitor hotplug)

## Later

见 [FUTURE.md](FUTURE.md)。

## Tech debt (上次 review 摸底 2026-06-17)

### Critical(已记,等 P0 PR 修)
- `cargo check` 20 warnings(dead code: `set_state` 11× / `region` field 1× / 其它)
- Novita / Qwen STUB 永远返 `ServerError` → 退避 30 min cap → 浮窗假死(用户配 key 不知道是 STUB)
- 31/193 cargo test 失败(i18n 切换后中文 hard-code 断言破裂)

### High(等 P2-C PR 修)
- 13 provider 错误分类不统一(401/403/429 各自映射不同),缺 `http_status_to_error_kind` helper
- `Provider::Minimax` 占位散落 7+ 处([memory/tavily-enum-placeholder-footgun] 警示踩坑已发生)
- `refresh_inner` 每次 `Box::new` 13 个 source([memory/source-instance-rebuild-footgun] 已知未修)
- Backoff 状态不持久化到 disk,重启后 30min 退避归零
- Per-provider poller task 无 shutdown signal,App 退出时可能泄漏
- `refresh_single_inner` miss 时返硬编码中文(regression of `8e2a19b` 修法)
- Frontend 0 单元测试(contentFingerprint / render / updateCard / autoResizeWindow)

### Medium(等 P2-C 顺手修)
- `src-tauri/src/commands/mod.rs:984-988` 硬编码中文 "未知的 source id"
- `src-tauri/src/api.rs` 描述里的"核心:拉取 + 宽容解析"已不存在(doc 描述需同步)
- `src/settings/source-extras.ts` 7 个 `renderXxx` 函数未完全数据驱动
- `error.provider.minimax_403` 孤例 i18n key
- `Uuid::new_v4().simple()` UUID 路径散落,提到顶层 `format_id()`

### Low(永不做 — 重复产品边界)
- ~~"Provider enum 已被 QuotaSource trait 替代,dump CLI 还在走 enum 路径"~~ — **已过期**:`lib.rs::run_dump_subcommand` 已走 `builtin_sources()` + 字符串 id
- ~~"src/settings.html 还在按 provider id hardcode"~~ — **已过期**:PR 3 完成后已动态化
- ~~"api.rs 描述里的'核心:拉取 + 宽容解析'已不存在"~~ — **已过期**:api.rs 已删除并入 providers/

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
