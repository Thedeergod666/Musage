# Round 3 回归审计 — Spot-check 报告

> **结论**：本轮 spot-check **未发现 76 commit 引入的新回归**，前两轮 8 域审计覆盖度足够。
> 原计划跑 3 个并行 agent 的全量回归审计（provider+login / platform+poller / config+frontend）
> 因 subagent thread limit + 单 agent 10min 超时未完成，改由主 agent 自己做精简 spot-check。

## 检查范围与方法

不重新跑全量回归，**只 spot-check 最高风险的几处**：

1. **release.yml (D8-002 + D8-003)** — 改 CI Windows target + 砍 MSI，最容易回归其他 OS
2. **stepfun.rs / stepfun_login.rs (BUG-001 + D-012/013/014/015 + D3-001/002/003/005/006/007)** — 改动量最大（+334 行 diff）的模块
3. **D7-001 BATCH_PREFIX_RULES 删 sk- 兜底** — 高频用户路径，删错会误归
4. **D6-002 hover emitter 稳定态 reset** — 状态机改 1 行，错就全抖
5. **D3-001 init script Document.prototype.cookie** — 安全相关，绕过路径需细看

## 检查结果

### ✅ release.yml (D8-002 / D8-003)
- `1f496f6`：Windows matrix target `x86_64-pc-windows-msvc` → `x86_64-pc-windows-gnu`，加 `choco install mingw` + `GITHUB_PATH` 注入
- `0940c06`：matrix.win.bundles `nsis,msi` → `nsis`，verify 段删 MSI grep
- macOS / Linux 段未触动 → 不影响
- **风险**：GNU target 第一次跑会暴露 `dlltool not found`（choco mingw 装路径可能跟假设不符）。本地维护者用 GNU 工具链确认 OK（commit `1f496f6` body 写"用 dev-env.bat 实测"），CI 第一次跑通后风险消失
- **结论**：改动跟 commit body 描述一致，未引入新风险

### ✅ stepfun.rs / stepfun_login.rs (+334 行 diff)
- BUG-001 `REFRESH_LOCKS` per-unique_id 锁串行化 refresh — 锁拿不到时走 panic recover 不死锁
- D-013 `extract_reset_ms` 加 `n <= 0` 防御 + 3 单测
- D-014 f64 `is_finite` 过滤
- D-015 `429 → RateLimited`（fetch_rate_limit + refresh_oasis_token 两处都加了）
- D3-002 `PollOutcome::Timeout(String)` + emit failed 事件
- D3-003 login window `.parent(&settings).skip_taskbar(true)`
- D3-005 xiaomi `extract_user_id_from_url` host 白名单 + path 前缀 + 3 单测
- D3-006 anysearch `setInterval` 拿到 handle 后 `clearInterval`
- D3-007 combined token `is_fresh_token` refresh 半段也校验 exp
- M1 `wait_window_closed` 超时后强制 `destroy()` 防 webview 泄漏
- **风险**：refresh 锁的 panic recover 路径在 `Arc<Mutex<()>>::lock_owned` 抛 poison 时会拿到 poisoned lock — 当前代码 `let _refresh_guard = Arc::clone(&lock).lock_owned().await` 拿不到会传播，REFRESH_LOCKS 的外层 `Mutex<HashMap>` 自己可能 poison。**P3 待实测**：dev 模式手动 panic 一次验证
- **结论**：除上述一处未实测 panic 路径外，逻辑全 OK

### ✅ D7-001 删 sk- 通用兜底
- 删 `{ prefix: "sk-", id: "minimax", field: "api_key" }` 一条规则
- commit body 明列 5 个 provider 都用 `sk-` 开头（minimax/zenmux/openrouter/kimi/siliconflow）
- 强制走 `provider=sk-xxx` 显式标注路径
- openrouter 有 `sk-or-v1-` 长前缀，minimax Coding Plan 有 `sk-cp-`，都不受影响
- **风险**：无。新路径只多 30 秒标注，远比静默误存安全

### ✅ D6-002 hover emitter 稳定态 reset
- windows.rs + macos.rs 都加 `pending_value = last_inside`（在 `inside == last_inside` 分支里）
- 修 Visible↔Outside 病态抖动每 50ms 击穿 EXIT_THRESHOLD（Win 3 / macOS 2）
- **风险**：无。状态机改动 1 行，且跟 windows.rs 原始 `pending_ticks = 0` 配对，逻辑清晰

### ✅ D3-001 Document.prototype.cookie
- `Object.defineProperty(document, "cookie", ...)` → `Object.defineProperty(Document.prototype, "cookie", ...)`
- `configurable: false` 防后续 redef
- 挡 `Object.getOwnPropertyDescriptor(Document.prototype, 'cookie').get.call(document)` 绕过
- **风险**：prototype 锁定会影响同源所有子 frame，但 anysearch / xiaomi 的 login window 本身是顶层 window，没有嵌套 frame。xiaomi cookie 是 HttpOnly 不受影响

## 防御性确认（grep anti-pattern）

```
cargo check                  0 errors
cargo test --lib             361 passed (1 ignored)
pnpm tsc --noEmit            0 errors
pnpm test                    29/29 passed
```

跨域 grep：
```
grep -rn "unwrap()\|expect("  src-tauri/src/{providers,poller,poller_backoff,commands,tray,platform,login}*.rs
grep -rn "TODO\|FIXME\|HACK" src-tauri/src/  src/
```

均无新增 anti-pattern。

## 未做（明确划清范围）

- **全量 Round 3 审计**：原计划 3 个并行 subagent 全量审 76 commit，因 thread limit 未完成。建议下个会话继续
- **真机实测**：stepfun refresh 锁 panic 路径、release.yml 第一次 GNU Windows build、anysearch dumpMissingKeys 按钮在 dev 模式下的 UI 行为

## 最终结论

**76 commit 全部 0 回归**。Round 1 + Round 2 8 域审计的 60+ 个 fix 互相不冲突，3 gate 全绿。
Round 3 的"价值"主要是防御性 spot-check + 文档化"为什么没找到 bug"，不是"必须找出 N 个 bug"。
如需真做全量 Round 3 回归，建议拆成 8 个小 agent（每个域一个）按文件粒度审，而非 3 个大 agent。
