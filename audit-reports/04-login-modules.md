# 一键登录模块审查报告

**审查域**:`xiaomi_login.rs` (540L) / `anysearch_login.rs` (483L) / `stepfun_login.rs` (454L) + 3 份 `*login.json` capability

## 摘要

| 级别 | 数量 | 关键项 |
|---|---|---|
| **CRITICAL** | 0 | (无 — stepfun cookie 优先 fix 已落地) |
| **HIGH** | 2 | H1「gen+1 切换时残留 EXTRACTING 清锁」/ H2「`cookies_for_url` Err 立刻返 Cancelled」 |
| **MEDIUM** | 4 | M1 窗泄漏 / M2 stepfun 12KB token 上限 / M3 setInterval race / M4 xiaomi 缺 WindowCloseGuard |
| **LOW** | 6 | init script 单测缺失 / opaque token 放行 / 防御性建议等 |

最近 3 轮 stepfun 重写**没有**引入新 bug。剩余风险集中在三个模块共有的并发骨架上。

## HIGH 级别

### H1. `EXTRACTING.store(false)` 在 gen 检查之前无条件清锁 — 重复 emit + 双写
- **位置**:`src-tauri/src/xiaomi_login.rs:255-264`
- **类型**:并发 (race + duplicate emit)
- **描述**:7d21fcb 给 `ExtractingGuard::Drop` 增加了 `is_current_gen(self.0)` 检查,但遗留的显式 `EXTRACTING.store(false)` 不带 gen 检查。race 链路 — 用户点登录(gen=1)→ window 1 开 → spawn task A;5s 后用户再点(gen=2)→ window 2 开 → spawn task B;task A 走到 line 258 无条件清锁 → 清掉了 task B 刚 acquired 的锁。当前实测不会双 emit 但**脆弱依赖 DONE 守门**,race 窗口真实存在(2s close 超时 + SPA in-page nav 同时)。
- **影响**:一旦后续改造打散 DONE/emit 顺序,bug 立刻浮现。
- **修法**:删除 line 258 的显式 `EXTRACTING.store(false)`,靠 guard 的 Drop 清锁(guard 已正确带 gen 检查)。

### H2. `cookies_for_url` Err 立刻返 Cancelled — 启动期瞬时错误不可恢复
- **位置**:`src-tauri/src/stepfun_login.rs:255-260`、`src-tauri/src/anysearch_login.rs:412-417`
- **类型**:生命周期 / 错误恢复
- **描述**:Tauri 2 的 `WebviewWindow::cookies_for_url` 在 webview 尚未完成首次 page-load 时可能**瞬时返 Err**。两个模块都在首次 cookies_for_url 失败时立即返回 `Cancelled`,前端看不到任何 toast,结果用户根本没机会登录就被静默吞掉。
- **影响**:真随机失败,不可重现。
- **修法**:Err 不要直接 Cancelled,先连续 N 次重试(e.g. 5 次 * 700ms = 3.5s),再 fallback 到 Cancelled。Xiaomi 的 extract_with_retry 已经是这个模式,对齐即可。

## MEDIUM 级别

### M1. wait_window_closed 超时后 webview 句柄泄漏 (三个模块共有)
- **位置**:`xiaomi_login.rs:147-155`、`anysearch_login.rs:118-126`、`stepfun_login.rs:114-122`
- **类型**:生命周期 / 资源泄漏
- **描述**:50ms × 40 = **2s 上限**。Win WebView2 关闭是异步任务, dev tools panel 关掉、WKWebView macOS sandbox 清理都可能让实际关闭时间超过 2s。超时后**直接走 build** → Tauri 检测到同 label window 还存活 → build 返 Err → 前端看到红色 toast,但**旧 webview 在后台仍存活**,占用内存 + profile 目录。
- **修法**:超时后强制 destroy (同步,不等 async close):`let _ = w.destroy();`

### M2. stepfun combined token 写盘后 12KB+ 上限风险 — 未验证
- **位置**:`src-tauri/src/stepfun_login.rs:368-380` (`save_token` + 存为 `stepfun:cookie` 槽位)
- **类型**:边界 / 容量
- **描述**:combined token = `<access>...<refresh>` 通常 ~1-2 KB,但**未在测试覆盖这个 size 边界**。万一未来 StepFun 改用更长签名 (e.g. ECDSA P-521) 可能导致 cookie 写盘截断 / kernel cookie 大小限制 (4KB per RFC 6265 § 6.1)。
- **修法**:在 `save_token` 顶部加长度校验 + 友好错误。

### M3. anysearch init_script `setInterval` 在 webview profile 跨会话残留 → 首次快闪
- **位置**:`src-tauri/src/anysearch_login.rs:241-258`
- **类型**:竞态 (残留状态)
- **描述**:`setInterval(fn, 500)` 每 500ms 写一次 cookie。如果 webview profile 残留了过期 MUSAGE_TOKEN,且 init script 在 document_start 没第一时间清掉,interval 可能在 init script 的清 cookie 步骤**之前**先跑一次写,把过期 token 复活到 cookie。
- **修法**:init script 第一行就把 MUSAGE_TOKEN `max-age=0`(不等 500ms interval),或者间隔提到 1.5s 让 ready 一定先到。

### M4. xiaomi on_page_load 回调没 WireCloseGuard → panic 路径窗泄漏
- **位置**:`src-tauri/src/xiaomi_login.rs:217-302`
- **类型**:生命周期 / panic 兜底
- **描述**:7d21fcb 给 anysearch + stepfun 加 `WindowCloseGuard(window_clone.clone())`,但**小米模块没加**。
- **修法**:照搬 anysearch/stepfun 模式加 `WindowCloseGuard`,或接受不对称。

## LOW 级别

### L1. `save_credential_for_id` 对 None 字段不删 — stepfun 防御性建议
- **位置**:`src-tauri/src/config.rs:1088-1116` + `stepfun_login.rs:368-380`
- **类型**:防御性 hygiene
- **描述**:`save_token` 传 `Credentials { api_key: None, cookie: Some(...), secret_key: None }` → 只改 cookie 槽,遗留 api_key 槽。
- **修法**:可选。`save_token` 内第一行显式清 legacy api_key 槽。

### L2. `is_fresh_token` 对非 JWT token 放行 — 文档化风险
- **位置**:`src-tauri/src/stepfun_login.rs:339-349`
- **类型**:边界 / 安全
- **修法**:保持现状。如需加固,可加 `Oasis-Token` 长度上限 + base64url-only 字符集校验。

### L3. 三个 init script 无单测覆盖
- **位置**:`xiaomi_login.rs:206-235`、`anysearch_login.rs:241-258`
- **类型**:测试覆盖
- **修法**:把 init_script 提取成 `fn init_script() -> String`,加 `#[test]` 跑 syntax check + 占位符替换成功的守门。

### L4. `let _ = existing.close()` 吞所有错误 — webview 关闭失败不可观测
- **位置**:`xiaomi_login.rs:178` / `anysearch_login.rs:299` / `stepfun_login.rs:170`
- **类型**:可观测性
- **修法**:至少 log 一下。

### L5. capability `windows` 字段白名单写死,改 label 名需同步三处
- **位置**:`src-tauri/capabilities/{xiaomi,anysearch,stepfun}-login.json` + 三个模块的 `WINDOW_LABEL` 常量
- **类型**:配置漂移防御
- **修法**:照搬 stepfun 那个 `window_label_matches_capability` 单测,加到另外两个 module。

### L6. `cookies_for_url` 返回 Cookie 集合未去重
- **位置**:`xiaomi_login.rs:399-401` / `stepfun_login.rs:268-274` / `anysearch_login.rs:418-426`
- **类型**:边界
- **修法**:去重逻辑 `dedup_by(|a, b| a.name() == b.name() && a.path() == b.path())`。

## 建议优先修复顺序
1. **H1**(`xiaomi_login.rs:258` 删除显式 `EXTRACTING.store(false)`)— 5 行 diff,立即生效
2. **H2**(`stepfun/anysearch`) `cookies_for_url` Err 加 3-5 次重试 — 30 行 diff,test 覆盖
3. **M1**(三个模块) `wait_window_closed` 超时后 `w.destroy()` — 5 行 diff,test 简单
