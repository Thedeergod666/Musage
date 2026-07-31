# Musage 2026-07-30 全量审计 — 分级汇总报告
> 单份 P0-P3 汇总，对应 8 份分报告 `01~08-*.md`（每份是分域详查）
> 基线 `9cdbb1c` (v0.2.5 候选) + 2026-07-30 当天未 commit 工作
> 修复落地：分支 `codex/audit-fixes-2026-07-29` 上 76 个 atomic commits

---

## 总览统计

- 总 audit commits: **76**
- 覆盖域: **8** (01 providers-a / 02 providers-b / 03 login / 04 config-ipc / 05 poller-lifecycle / 06 platform-tray / 07 frontend / 08 misc) + 上轮 2026-07-29 audit 20 commits
- 总代码变更: 58 文件 / +3901 / -721 行
- 新增单测: **8** (D-013 ×3 / D3-005 ×3 / D3-007 ×1 / D8-004 ×1)
- 守门: `cargo test --lib` 361 passed (1 ignored) / `pnpm tsc --noEmit` 0 errors / `pnpm test` 29/29 passed

| 优先级 | commits | 说明 |
|---|---|---|
| **P0** 紧急 (critical panic / 永久数据损坏 / 永久泄漏) | **2** | D-012 (provider OOM) / D8-001 (4 用户可见 raw key) |
| **P1** 高 (回归风险 / 必触发) | **25** | 包括 release.yml / 配置锁 / 退避 / 浮窗几何丢 |
| **P2** 中 (defense-in-depth / 一致性 / tech debt) | **10** | capability overgrant / prototype lock / 计数 cap |
| **P3** 低 (文档 / 命名 / 边缘 case) | **11** | doc / dumpMissingKeys / magic number |
| **上轮 2026-07-29 audit** | **48** | H1~T1 上一轮已修,本轮作为基线 |

---

## 🔴 P0 — 紧急 (立即修, 阻塞 v0.2.5) (2 bugs)

### 02 providers-b (D-旧 ID) — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D-012` | `8976d36` | fix(stepfun): apply json_body_limited in fetch_plan_status (audit D-012) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |

### 08 misc — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D8-001` | `92a600b` | fix(i18n): add 4 missing production t() keys (audit D8-001) | [08-misc.md](08-misc.md) |

## 🟠 P1 — 高 (下次发布前修, 必触发) (25 bugs)

### 02 providers-b (D-旧 ID) — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D-001` | `54a319e` | fix(providers): enforce response body limits globally (audit D-001) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-013` | `c812a4d` | fix(providers): reject n<=0 in claude/kimi extract_reset_ms (audit D-013) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |

### 03 login — 3 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D3-002` | `4a2f0f0` | fix(login): emit failed event on 14min polling timeout (audit D3-002) | [03-login.md](03-login.md) |
| `D3-003` | `1e0f8d0` | fix(login): parent login windows to settings + skip_taskbar (audit D3-003) | [03-login.md](03-login.md) |
| `D3-004` | `91b693a` | fix(capabilities): drop overgrant create-webview-window from login caps (audit D | [03-login.md](03-login.md) |

### 04 config-ipc — 8 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D4-001` | `6ee542e` | fix(config): rename corrupted config.json instead of copy (audit D4-001) | [04-config-ipc.md](04-config-ipc.md) |
| `D4-003` | `42b1427` | fix(commands): fold add_extra_instance key save into write lock (audit D4-003) | [04-config-ipc.md](04-config-ipc.md) |
| `D4-004` | `3868456` | fix(commands): delete_extra_instance save failure rolls back keys.json (audit D4 | [04-config-ipc.md](04-config-ipc.md) |
| `D4-005` | `2afe431` | fix(config): stop silently swallowing keys.json backup failures (audit D4-005) | [04-config-ipc.md](04-config-ipc.md) |
| `D4-006` | `2619a09` | fix(commands): skip delete_credential_for_id when target ref reused by compact ( | [04-config-ipc.md](04-config-ipc.md) |
| `D4-007` | `08d3b8d` | fix(config): truncate keys.json.bak backups at startup (audit D4-007) | [04-config-ipc.md](04-config-ipc.md) |
| `D4-008` | `e4a603d` | fix(commands): validate floating window coords/dims in save_config (audit D4-008 | [04-config-ipc.md](04-config-ipc.md) |
| `D4-009` | `fd85511` | fix(commands): cap provider_order / schema_overrides counts in save_config (audi | [04-config-ipc.md](04-config-ipc.md) |

### 05 poller-lifecycle — 8 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D5-007` | `c659519` | fix(poller): batch refresh_inner backoff.write acquisitions (audit D5-007) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-033` | `65b0847` | fix(poller): purge stale interval schedules (audit D5-033) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-038` | `2904808` | fix(commands): extract publish_snapshot helper (audit D5-038) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-066` | `44b9efa` | fix(backoff): avoid retaining idle source entries (audit D5-066) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-074` | `2629903` | fix(poller): jitter delay before per-provider spawn (audit D5-074) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-084` | `52af16a` | fix(poller): keep manual refresh failures out of backoff (audit D5-084) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-101` | `528622c` | fix(geom_persister): flush latest geom on SHUTDOWN (audit D5-101) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |
| `D5-102` | `e2af7d2` | fix(platform): graceful shutdown for OS hover emitter threads (audit D5-102) | [05-poller-lifecycle.md](05-poller-lifecycle.md) |

### 06 platform-tray — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D6-001` | `ff3bf63` | fix(platform/macos): fullscreen watcher checks SHUTDOWN (audit D6-001) | [06-platform-tray.md](06-platform-tray.md) |

### 07 frontend — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D7-001` | `6048702` | fix(settings): drop generic sk- prefix rule from batch paste (audit D7-001) | [07-frontend.md](07-frontend.md) |
| `D7-002` | `e250ff1` | fix(settings): make initI18n idempotent (audit D7-002) | [07-frontend.md](07-frontend.md) |

### 08 misc — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D8-002` | `0940c06` | fix(release): drop MSI from Windows bundle (audit D8-002) | [08-misc.md](08-misc.md) |

## 🟡 P2 — 中 (v0.3 周期修, defense-in-depth / tech debt) (10 bugs)

### 02 providers-b (D-旧 ID) — 5 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D-002` | `86ba489` | fix(providers): reject non-finite numeric strings (audit D-002) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-004` | `cc61229` | fix(deepseek): reject balance entries without amounts (audit D-004) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-005` | `4fc16ac` | fix(tray): preserve unknown aggregate health (audit D-005) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-014` | `a0e02d9` | fix(providers): filter NaN/inf in 4 local f64 helpers (audit D-014) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-015` | `3c398dd` | fix(providers): classify 429 as RateLimited in 3 fetch sites (audit D-015) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |

### 03 login — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D3-001` | `6d447b2` | fix(login): lock Document.prototype.cookie instead of document.cookie (audit D3- | [03-login.md](03-login.md) |
| `D3-005` | `4317ed5` | fix(xiaomi): restrict extract_user_id_from_url to trusted hosts (audit D3-005) | [03-login.md](03-login.md) |

### 06 platform-tray — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D6-002` | `d2c70fe` | fix(platform): reset pending_value on stable hover tick (audit D6-002) | [06-platform-tray.md](06-platform-tray.md) |
| `D6-003` | `eadc4ef` | fix(platform/macos): use Retained::retain for NSWindow in hit test (audit D6-003 | [06-platform-tray.md](06-platform-tray.md) |

### 08 misc — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D8-003` | `1f496f6` | fix(release): switch CI Windows target to GNU + install MinGW (audit D8-003) | [08-misc.md](08-misc.md) |

## 🟢 P3 — 低 (v0.3 tech debt, 文档/边缘) (11 bugs)

### 02 providers-b (D-旧 ID) — 5 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D-006` | `a32963a` | fix(zenmux): parse_iso8601_ms accept naive datetime (audit D-006) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-007` | `f369846` | fix(tavily): test use t!() for i18n-sensitive strings (audit D-007) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-009` | `43be409` | fix(openrouter): prune stale entries from LAST_SUCCESSFUL (audit D-009) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-010` | `aa33f83` | fix(tavily): accept RFC3339 / NaiveDateTime for billing_period.end (audit D-010) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |
| `D-011` | `80c69fe` | fix(minimax): smart_reset_to_ms clamp negative duration (audit D-011) | [01-providers-a.md + 02-providers-b.md](01-providers-a.md + 02-providers-b.md) |

### 03 login — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D3-006` | `c377685` | fix(anysearch): stop token-write interval after first success (audit D3-006) | [03-login.md](03-login.md) |
| `D3-007` | `f1ec0dc` | fix(stepfun): validate refresh half in is_fresh_token (audit D3-007) | [03-login.md](03-login.md) |

### 06 platform-tray — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D6-004` | `aafe3d4` | fix(platform/macos): update set_window_level doc to match H15 (audit D6-004) | [06-platform-tray.md](06-platform-tray.md) |

### 07 frontend — 1 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D7-008` | `d64dc4e` | fix(settings): prefix testConn summary with unique_id (audit D7-008) | [07-frontend.md](07-frontend.md) |

### 08 misc — 2 commits

| Bug ID | Commit | Subject | 源报告 |
|---|---|---|---|
| `D8-006` | `ca3967a` | fix(commands): extract POLLER_DRAIN_TIMEOUT const (audit D8-006) | [08-misc.md](08-misc.md) |
| `D8-007` | `601c0f2` | fix(settings): expose dumpMissingKeys in dev about section (audit D8-007) | [08-misc.md](08-misc.md) |

---

## 上轮 2026-07-29 audit 已修基线 (20 commits, 本轮未触碰)

作为本轮 8 域审计的修复基线，按 H1~T5 命名（critical/high/medium/low/trace 五档）。

| Commit | Subject |
|---|---|
| `9cdbb1c` | fix(xiaomi-login): on_page_load 加 WindowCloseGuard 对齐 stepfun/anysearch (audit M |
| `cc842b9` | fix(lib): tray::setup 先于 poller::start 避免首轮 snapshot 丢弃 (audit M1) |
| `721d016` | fix(platform/macos): NSWindow raw ptr 包 Retained 防 dispatch 间 UAF (audit H2) |
| `331c59b` | fix(commands): BackoffState::record 加 RefreshSource caller 区分 (audit H5) |
| `ec294f1` | fix(poller): graceful shutdown via SHUTDOWN Notify + quit_app 两步退出 (audit H1) |
| `d528ffe` | fix(stepfun-login): save_token 加 12 KB combined token 上限 (audit M2) |
| `406c021` | fix(login): wait_window_closed 超时 2s 后强制 destroy 防 webview 泄漏 (audit M1) |
| `93af7b6` | fix(tray): Windows tooltip 按 127 UTF-16 单元截断 + scalar 边界 (audit M4) |
| `0cc75b6` | fix(tray): sanitize_percent NaN/Infinity/越界归一,percent 文本不再显示 -25%/NaN% (audit M5 |
| `683baa1` | fix(config): truncate_old_backups 启动清理老 .bak 留最新 5 份 (audit L4) |
| `7e1a14d` | fix(commands): save_config refresh_interval_secs 上限 86400 (audit L2) |
| `15a0fda` | fix(commands): save_config providers map 加 256 条 DoS cap (audit H4) |
| `5d16723` | fix(config): cleanup_orphan_tmp_files 只清 OUR_TMP_SUFFIXES 防误删用户文件 (audit H1) |
| `02ed5f4` | fix(login): cookies_for_url Err 改 continue 重试而非 Cancelled (audit H1) |
| `19b7bb1` | fix(tray): macOS 浅色菜单栏改黑字 (audit H2) |
| `4d4eb2d` | fix(tray): Percent 100% 自适应缩小防裁切 (audit H1) |
| `f8ba007` | fix(poller): per-provider ±10% jitter 防 thundering herd (audit H2) |
| `798115b` | fix(xiaomi/login): 删显式 EXTRACTING 清锁,靠 ExtractingGuard Drop (audit H1) |
| `529fb6c` | fix(win/platform): apply_z_order 加全局锁防 hover/main thread 竞态 (audit T1) |
| `b179b27` | fix(logstore): redact 敏感字段防 API key/cookie 落盘 (audit H3) |

---

## 单测增量

| Bug ID | 测试名 | 文件 |
|---|---|---|
| D-013 | `claude_official::tests::extract_reset_ms_*` | `providers/claude_official.rs` |
| D-013 | `kimi::tests::extract_reset_ms_*` | `providers/kimi.rs` |
| D-013 | (另一) | (见 commit `c812a4d`) |
| D3-005 | `xiaomi_login::tests::extract_user_id_*` (3 case) | `xiaomi_login.rs` |
| D3-007 | `stepfun_login::tests::combined_token_requires_both_halves_fresh` | `stepfun_login.rs` |
| D8-004 | `config::tests::best_effort_*` | `config.rs` |

---

## 风险 & 已知未做

**未触发生产环境 panic 的"几乎 P0"路径**:
- 02 报告 D-012 内存撑爆（fetch_plan_status 5xx 不带 body 截断）。触发面：StepFun 5xx 时；用户量大后单 provider 1MB 响应也能吃满。已加 200 字符截断
- 04 报告 D4-001 损坏 config.json 留原文件 + 同名 .bak，导致后续 save 永远失败。已改 `std::fs::rename` 原子移走

**未修 P3 (tech debt) 推 v0.3**:
- `refresh_inner` 每次 `Box::new` 12 个 source 优化（按 Arc 缓存）
- Backoff 状态持久化到 disk
- Per-provider poller task shutdown signal（App 退出时不泄漏）
- `delete_extra_instance` v2（重命名 keys.json + spec）
- Frontend 单元测试覆盖另外 ~15 个核心函数（contentFingerprint / render / updateCard / autoResizeWindow）
- `http_status_to_error_kind` 全面推广（已落地为 [`classify_http_status`](src-tauri/src/providers/mod.rs)，kimi 优先；其余 provider 保留各自的具体 msg 短路）

**实测未覆盖** (按审计报告"待实测"段):
- D3-007 is_fresh_token 修后真实 StepFun 账号未实测（旧测试是 30min 过期边缘，30 天后才会再次触发）
- D-013 / D-014 / D-015 部分 provider 无真实账号（如 siliconflow 拿不到账号）
- D8-003 release.yml 切 GNU 工具链后未在 Windows runner 实际跑过 tag build

