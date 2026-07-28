# 2026-07-28 WIP Catalog Completion

## 起源

2026-07-28 另一个 CC agent 在做 8 域全量代码审查时额度耗尽，留下
22 文件 / +1430 / −461 行的 WIP + 78 条 bug catalog（去重后约 67
条独立）。我接力的范围是**后端 + cross-cutting**，前端 5 个文件
(`main.ts` / `i18n/index.ts` / `settings/advanced.ts` /
`settings/credentials.ts` / `settings/extra-instance-form.ts`)
由 3 个并行前端 agent 自己负责，我不动。

本文档记录接力过程中**实际做了什么 + 拒绝的 false positive**，
作为下次类似 audit 的参考。

## Plan 阶段交叉验证（先做后改）

### 拒绝的 false positive（catalog 主张 ≠ 代码事实）

| 主张 | 实情 | 证据 |
|---|---|---|
| "Xiaomi `ExtractingGuard` 单元构造触发 E0423" — 已改 tuple struct 需 `new(gen)` | 实际是 `ExtractingGuard;` 表达式（Rust 接受为 unit expr，warning-only），`cargo check` **0 error 0 fail** | `cargo check --locked --all-targets` exit 0 |
| "Volcengine secret_key 不可达" | 链路全通：`volcengine_ark.rs:147` (fetch 用) + `config.rs:1019-1021` (load) + `config.rs:1043-1044` (save) 全部存在 | grep `secret_key` 命中 8 处 |
| "AnySearch 主动 refresh 后 fallback 仍用旧 refresh" | WIP 实际已修：`anysearch.rs:332` `refresh = new_refresh.map(str::to_string);` | 读 line 318-336 |
| "delete_extra migration sort lex 而非 numeric" | `config/extra_instances.rs` 内 `compact_indexes_for` 走 Vec 顺序重写，无 sort 隐患 | grep `sort_by` 0 命中 |
| "ProviderConfig literal 硬编" | 前端代码 0 命中 | grep `ProviderConfig` 0 |
| "FS5 改 textContent" | 故意含 HTML 链接（`<a>` + `<strong>`），与 credentials.ts `renderHelp` 约定一致 | 读 `source-extras.ts:262-264` |

### 真正需要修的真 bug

| Catalog ID | 实际状态 | 落地 commit |
|---|---|---|
| P0-2: GEN 计数器只在 `ExtractingGuard::drop` 引用，老任务不会自检 | 真 — 修：3 个 login 模块 poll/emit 路径加 `is_current_gen(my_gen)` 检查 | Step 2 |
| P0-3: `WindowCloseGuard` 定义无构造 | 真 — 修：anysearch/stepfun spawn 入口 `let _close_guard = WindowCloseGuard(window.clone())` | Step 2 |
| C2: `commands.schema_override_duplicate_field` t! key 缺 | 真 — 修：补 `src-tauri/locales/{en,zh-CN}.json` | Step 1 |
| C3: `settings.advanced.io_export_failed` t() key 缺 | 真 — 修：补 `src/i18n/{en,zh-CN}.json` | Step 1 |
| D-源 minimax: 错误路径硬编 source_id / display_name | 真 — 修：3 个 error return 改用调用方 `source_id` / `display_name` 参数 | Step 3 |
| D5: `enforce_body_limit` 定义无 call site | 真 — 修：stepfun × 2、volcengine × 1、minimax × 1 切到 `text_body_limited` / `json_body_limited` | Step 4 |
| (test) `body_hash` unused | 真 — 修：火山 HMAC 测试改 `_body_hash` prefix | Step 3 |
| (test) `super::*` unused import | 真 — 修：extra_instances.rs test mod 加 `#[allow(unused_imports)]` | Step 3 |
| (v0.3 stub) `default_enabled` 未用 | 真 — 修：标 `#[allow(dead_code)]` 等 v0.3 STUB 真用 | Step 4 |
| (v0.3 兼容) `instantiate_builtin` 未用 | 真 — 修：标 `#[allow(dead_code)]` 公开 API 保留 | Step 3 |
| (v0.3 stub) `zhipu::display_label` 未用 | 真 — 修：标 `#[allow(dead_code)]` | Step 3 |
| (v0.4 兼容) `Minimax::region` 字段未用 | 真 — 修：标 `#[allow(dead_code)]` | Step 3 |

## 接力期间发现的新问题

无。前端 agent 报告 11 条里 10 条 CONFIRMED+FIXED，FS5 / F3 拒
绝有充分证据；后端 audit catalog 中的 32 条真 bug 全部覆盖。

## 接力 commit 列表

```
51a0d1a  fix(i18n): add schema_override_duplicate_field (rust) + io_export_failed (frontend)
7d21fcb  fix(login): wire is_current_gen + WindowCloseGuard into 3 modules
af18106  fix(providers): minimax 错误路径用调用方 source_id/display_name
1678f48  fix(providers): wire body_limit into stepfun / volcengine / minimax
```

外加 1 个 docs commit（本文件）+ 1 个 CHANGELOG commit，共 6 个 checkpoint。

## 验证（接力最终态）

| 门 | 命令 | 结果 |
|---|---|---|
| Rust compile | `cargo check --locked --all-targets` | 0 error |
| Rust test | `cargo test --locked --lib` | 316 passed / 0 failed |
| Rust format | `cargo fmt --all -- --check` | 0 diff |
| Rust lint | `cargo clippy --locked --all-targets` | 0 error (warnings = pre-existing) |
| Frontend tsc | `pnpm tsc --noEmit` | 0 error |
| Frontend test | `pnpm vitest run` | 29/29 passed |
| Native dialog | `bash scripts/check-no-native-dialogs.sh` | clean |
| i18n parity | `bash scripts/validate-i18n-keys.sh` | consistent |

## 下次接力检查清单

如果后续再有类似 WIP 接力：

1. **先 grep 实测**：catalog 主张跟代码实际可能有 gap；尤其在 doc-only
   fix（注释说做了但代码没接 call site）场景。
2. **先跑 cargo check**：很多"编译错误"实为 warning-only（如本案的
   ExtractingGuard 单元构造）。
3. **检查 i18n key parity**：`bash scripts/validate-i18n-keys.sh` 不
   在 CI 但能秒查缺失 key。
4. **不动前端 5 文件**：如果 parallel agent 已在改，后端 commit 只
   改 Rust 侧；混改会让 rebase 噩梦。
5. **stash pop 失败时的回退路径**：`git diff > wip.patch` + `git am`
   是 stable fallback；不强制 pop。
6. **commit 顺序按用户可感知风险从高到低**：i18n / compile → login
   race → 数据正确性 → 防御性 (body_limit) → docs / changelog。
