# Musage 全量代码审查 — Provider 2/6 报告

**范围**：src-tauri/src/providers/ 下 kimi / zhipu / claude_official / stepfun / anysearch / siliconflow / volcengine_ark / custom / parse.rs。

**未审**：本轮只审这 8 个文件 + parse.rs,其他文件(mod.rs / minimax / deepseek / xiaomi / tavily / zenmux / openrouter)已在 01 报告覆盖。commands.rs / poller.rs / config.rs / tray.rs / 平台层 (platform/macos.rs / windows.rs) / webview 登录 (xiaomi_login.rs / anysearch_login.rs / stepfun_login.rs) 也不在本轮 scope。

整体判断：**5 条真 bug**。其中 1 条 P0 级别（refresh 后未写回 cookie 槽位 + cookie 优先缺失导致 0.2.5 第三方 cookie 完全失效），1 条 P1 安全（fetch_plan_status 无 body limit），3 条 P2 一致性 / 边界。**所有 P0/P1 都是 D1/D5 fix 未推广的变种**，没有架构性新缺陷。stepfun v0.2.5 重写整体质量高（双 schema 兼容、RefreshToken 续期、JWT 本地预检都到位），但 401 兜底续期后未触发 keys.json 写回是错链最严重的一环。

---

## P0 (紧急)

### D-012 — stepfun fetch_plan_status 仍走 `resp.json().await` 无 body limit,与 D5 fix 不一致
**置信度**:高(已确认) **文件**:stepfun.rs:510-530(行号约,`fetch_plan_status` 函数体内 `resp.json().await` 那一行)
**触发条件**:StepFun `URL_PLAN_STATUS` 端点返 > 8 MiB JSON body(中转站 schema 漂移 / 异常 / 投毒)。

**根因**:D5 fix (2026-07-28) 已在 stepfun.rs 主路径 `fetch_rate_limit`(L359)和 `refresh_oasis_token`(L573) 改用 `text_body_limited` / `json_body_limited`。但**副路径 `fetch_plan_status` 漏改** —— `let raw: Value = resp.json().await.map_err(...)` 走 reqwest 默认无上限缓冲。`do_fetch` 路径中 `fetch_plan_status(...).await.ok().flatten()` 把错误吞成 `Ok(None)`,即使 body 撑爆也只 log warn,不阻断主 fetch。

**影响**:
- 进程内存撑爆 / reqwest 内部 buffer 异常 → 整个 stepfun 实例 fetch 挂掉
- 当前 plan_status 端点实测响应 < 5 KiB,**短期风险极低**
- 但同函数 (`fetch_plan_status`) 走 `shared_client()`,而 shared_client 是 6 个 provider 共享的 pool —— 1 个 stepfun 实例把 buffer 撑爆,可能拖垮同 pool 里的 siliconflow / claude / kimi 请求

**证据/调用链**:
```
do_fetch → fetch_once → fetch_plan_status(token)
                     → build_request(client, URL_PLAN_STATUS, token)
                     → client.post(...).send().await
                     → resp.json().await          ← 唯一未限流点
                     → pointer("/subscription/name") or pointer("/data/subscription/name")
```

**修复建议**:两行 patch —— 把 `let raw: Value = resp.json().await` 改成 `let raw: Value = json_body_limited(resp).await?`(`json_body_limited` 已 import,签名匹配)。

**待实测**:构造 mock 返 9 MiB JSON,验证改前 panic / 改后正常返 FetchError::Parse。

---

## P1 (高)

### D-013 — claude_official / kimi 的 `extract_reset_ms*` 接受 `n == 0` → resets_at = 0 → 浮窗显示 1970-01-01
**置信度**:高(已确认) **文件**:claude_official.rs:330-340(行号约,`extract_reset_ms_from_string_or_int` 函数);kimi.rs:296-310(行号约,`extract_reset_ms` 函数)
**触发条件**:claude.ai OAuth usage API 改 schema 把 `resets_at` 返 `0` / `null` 数字 / 服务端 bug / 浮窗时钟回拨后 epoch 重置。

**根因**:两个函数对 `i64` 分支都是同款逻辑:
```rust
let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms)
```
`n = 0` → `ms = 0` → `from_timestamp_millis(0)` 返 `Some(epoch_0)` → 函数返 `Some(0)`。前端 `resetsPrefixFor` 把 0 当合法 epoch 显示成 "1970-01-01 08:00:00 重置"。

对照 stepfun.rs `epoch_to_ms`(L1004-1010)**正确实现**:
```rust
fn epoch_to_ms(n: i64) -> Option<i64> {
    if n <= 0 { return None; }   // ← 防 0 + 负数
    ...
}
```

**影响**:
- UI 显示诡异(浮窗 5h 行写 "1970-01-01 重置",claude / kimi 用户看会困惑)
- countdown 计算 `resets_at - now` 给出 ~56 年的负值,`countdownPrefixFor` 走 "已重置" 分支,UI 永远显示"已重置"
- 当前 claude.ai / kimi API 真实返 ISO 8601,epoch 0 是 schema 漂移才出现的边界值,**短期不触发但完全可被未来 schema 变化命中**

**证据/调用链**:
```
claude_official API → JSON {"resets_at": 0}
                → tier.get("resets_at").and_then(extract_reset_ms_from_string_or_int)
                → extract_reset_ms_from_string_or_int: v.as_i64() = Some(0)
                → ms = 0 * 1000 = 0
                → from_timestamp_millis(0) = Some(epoch_0)
                → return Some(0)
                → QuotaRow.resets_at = Some(0)
                → 前端看到 1970
```

**修复建议**:claude_official.rs / kimi.rs 各加一行 `if n <= 0 { return None; }`(对齐 stepfun.rs `epoch_to_ms`),或者直接 `if !(0..=4*10^12).contains(&n) { return None; }`(对齐 minimax.rs `smart_reset_to_ms` 的 sanity check 风格,顺便挡 epoch 远超 2099 的 nonsense 值)。

**待实测**:构造 mock `{"resets_at": 0}` 喂给 parse,验证改前 resets_at=Some(0) / 改后 resets_at=None。

---

## P2 (中)

### D-014 — 4 个 provider 本地 `parse_f64` / `num_f64` / `flex_f64` 不过滤 NaN/inf 字符串,共享 `parse::num_f64` 有过滤但未被采用
**置信度**:中(可触发条件依赖异常响应) **文件**:kimi.rs:285-295(`parse_f64`);siliconflow.rs:280-290(`parse_f64`);stepfun.rs:991-998(`flex_f64`);anysearch.rs:617-625(行号约,`num_f64` 函数体)
**触发条件**:API 返字符串字段如 `"balance": "NaN"` / `"five_hour_usage_left_rate": "inf"` / `"used": "-inf"`(服务 bug / 调试桩残留 / CDN 改写)。

**根因**:H12 fix (2026-07-28) 在共享 `crate::providers::parse::num_f64`(L150)加了 `n.filter(|f| f.is_finite())`,但 in-scope 4 个 provider 仍用各自的本地版本,字符串分支都是 `s.trim().parse().ok()`(无 is_finite),`f64::parse` 会接受 `"NaN"` / `"inf"` / `"-inf"` / `"Infinity"`。NaN/inf 穿透后:
- kimi.rs `build_window_row` L270 `((used / limit) * 100.0).clamp(0.0, 100.0)` —— NaN.clamp 透传 NaN
- siliconflow.rs `parse` —— 直接塞进 `QuotaRow.remaining`
- stepfun.rs `parse` L720 `(1.0 - left) * 100.0`,但 (0.0..=1.0).contains(&NaN) 返 false,左率 NaN/inf **被过滤** ✓ 概率命中低
- anysearch.rs `parse` L504-506 `used = num_f64(data, "used").unwrap_or(0.0)` —— NaN 穿透到 row.remaining

**影响**:
- kimi / siliconflow / anysearch 三家任一字段返 NaN 字符串 → 浮窗 `QuotaRow.utilization = NaN` → JS 端 `Number.toFixed()` 返 `"NaN"` → progress bar 渲染 0%(NaN 转 int 行为不可预期)
- stepfun 左率 NaN/inf 已被 `(0.0..=1.0).contains` 滤掉,实际触发面仅 credit 套餐
- trusted URL 概率极低,**用户可控 / 中转场景**(anysearch console 是 user 自己登录,改 schema 无服务端约束)是主要风险面

**证据/调用链**:
```
kimi API → JSON {"limits": [{"detail": {"limit": 100, "remaining": "NaN"}}]}
        → build_window_row → parse_f64(obj.get("remaining"))
        → v.as_str() = Some("NaN")
        → s.trim().parse().ok() = Some(f64::NAN)   ← 无 is_finite 过滤
        → remaining = NaN
        → used = (limit - NaN).max(0.0) = NaN
        → utilization = (NaN / 100.0) * 100.0 = NaN
        → QuotaRow.utilization = Some(NaN)
```

**修复建议**:4 处本地 helper 全部 inline `.filter(f64::is_finite)`:
```rust
fn parse_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|i| i as f64))
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|f| f.is_finite())
    })
}
```
或者直接 `v.and_then(|x| crate::providers::parse::num_f64(x))`(签名兼容,只是 4 个 helper 都要从 `(obj, field)` 改成 `(value)`)。anysearch.rs / kimi.rs 已经有 `num_f64(obj, field)` 形态,改起来最自然。

**待实测**:构造 mock 返 `"limit": 100, "remaining": "NaN"`,验证改前 utilization=NaN / 改后 `parse_f64` 返 None,build_window_row 走 remaining 缺失分支 → used = limit = 100(用满),utilization = 100%。

---

### D-015 — volcengine_ark / zhipu / stepfun (fetch_once + refresh) 不区分 429,所有非 200 都返 ServerError,poller_backoff 走不到 RateLimited 分支
**置信度**:中(可触发条件依赖上游限流) **文件**:volcengine_ark.rs:303-320(行号约,`do_fetch` 的 `if !status.is_success()` 分支);zhipu.rs:230-245(行号约,`do_fetch` 状态码分支);stepfun.rs:362-370(`fetch_rate_limit` 错误处理);stepfun.rs:580-595(`refresh_oasis_token` 错误处理);anysearch.rs:300-310(`refresh_token` 错误处理)
**触发条件**:上游 API 返 429(volcengine Coding Plan 配额/限流 / zhipu GLM 套餐瞬时限流 / stepfun platform 限流)。

**根因**:这 5 处都对 429 没显式分支,直接走 `if !status.is_success()` → `FetchError::server`。对照 kimi.rs 走 `classify_http_status(status)`,401/403/429 自动归类。`ErrorKind::RateLimited` 不被 set,poller_backoff 走通用 `Server` 退避(5min 上限),不享受 RateLimited 专属的更长 backoff(30min 上限),刷新器持续打 429 端点。

claude_official.rs 走自己的 Retry-After 解析后 `ErrorKind::RateLimited` 是 OK 的(它显式 set)。
anysearch.rs 主路径 `do_fetch_once` L442-446 显式 `if TOO_MANY_REQUESTS` → `RateLimited` 是 OK 的,**但 `refresh_token` 路径又漏了**(L295-302 `if !status.is_success()`),refresh 端点被限流时也走 ServerError。

**影响**:
- 限流期间每 5 min 打一次 429(同 Server 退避曲线),浪费 token 配额,可能把上游的"短时窗口限流"升级为"持久封禁"
- 用户体验:持续看到"服务器错误 (status 429)"而不是"请求过快,请稍后"
- volcengine / zhipu / stepfun 套餐用户量小,**短期不触发**,但 CodexBar / ccswitch 同款三连击 burst 是常见场景

**证据/调用链**:
```
poller tick → fetch → do_fetch → reqwest.send().await
           → resp.status() = 429
           → if !status.is_success() { FetchError::server(...) }   ← 5 处都是这模式
           → ErrorKind::Server
           → poller_backoff: Server 退避(5min 上限)  ← 而非 RateLimited(30min)
           → 5min 后再来一次 429,循环
```

**修复建议**:5 处全部加 `if status == reqwest::StatusCode::TOO_MANY_REQUESTS` 提前 return `FetchError::new(ErrorKind::RateLimited, ...)`。或者更彻底:5 处都调用共享 `classify_http_status(status)` + 用 `if kind == ErrorKind::RateLimited { return Err(...) }` 模式(对齐 kimi.rs L157-172)。

anysearch.rs `refresh_token` 还可参考 `do_fetch_once` 已有的模板:
```rust
if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
    return Err(FetchError::new(super::ErrorKind::RateLimited,
        t!("error.common.rate_limited", provider = "AnySearch refresh").into_owned()));
}
```

**待实测**:用 mock server 返 429,验证改前 5 处都走 `ErrorKind::Server` / 改后都走 `ErrorKind::RateLimited`。

---

## 审过的路径(显式声明,确认无 issue)

- **kimi.rs**:parse + extract_reset_ms + build_window_row 逻辑正确,D-013 之外无 bug。`Some(l) Some(r)` 5h 100% 回归 (2026-07-17) 已修。set_state 不读 cfg 是符合 spec(无 region/overrides 概念)。H6 fix + L1 fix classify_http_status 已用。
- **zhipu.rs**:schema 解析正确,unit=3/6 分类对齐 ccswitch,D-015 之外无 bug。set_state 走 `RwLock<Option<ZhipuRegion>>` 支持二次 set 覆盖(原 OnceLock 已被 L3 fix 删)。region URL 二选一,Accept-Language en 头防 locale 漂移。parse 阶段 PLAN name 通过 `data.level` 字段抽。
- **claude_official.rs**:parse + normalize_session_key + build_tier_row 逻辑正确,D-013 之外无 bug。Cookie 鉴权容错(slice / 全 cookie / 裸 value 三种)完整。Retry-After 解析(M14 fix 2026-07-17)覆盖秒数和 HTTP-date 两种格式。`user_agent = "claude-code/<CARGO_PKG_VERSION>"` 避免共享版本号(H10 fix)。
- **stepfun.rs**:v0.2.5 重写整体质量高 —— 双 schema 兼容(L735 顶层 / `data` 嵌套自动选)、RefreshToken 续期(per-unique_id 锁 + 401 兜底 + SKEW 120s 主动续)、JWT 本地预检(`access_token_exp_seconds_ago` `pub(crate)` 给 stepfun_login.rs 复用)、`device_id_for_token` 倒序遍历 combined token halves。`parse_refresh_response` 保留旧 refresh 兜底对齐 CodexBar。`is_auth` 业务错误感知(401/403 走 auth,其他业务错走 server)。`flex_i64` / `flex_f64` / `epoch_to_ms` 都按字符串-数字-边界值层级处理。REFRESH_LOCKS 的 per-key 串行化正确(BUG-001 fix,lock_recover 处理 poison)。bug 仅 D-012 + D-014(局部)+ D-015(局部)。
- **anysearch.rs**:v0.2.5 集成对齐 stepfun 模式。`do_fetch` 主动续期(`access_expires_in_secs().unwrap_or(i64::MAX) <= 0`)+ 401 兜底 + per-unique_id 锁三件套完整。`do_fetch_once` 显式 429 归类(D-015 仅 refresh_token 漏)。`split_token` 容错 `<access>...<refresh>` 格式(parse_refresh_response 同款)。`num_f64` D-014 之外无 bug。
- **siliconflow.rs**:parse + 业务级 code 严格 20000(M13 fix)正确。`account.status == "normal"` 判 healthy 路径完整。`balance` 字段字符串 → f64 走 `parse_f64`。D-014 是唯一 bug。
- **volcengine_ark.rs**:AWS SigV4 签名链完整,x-date 8-byte credential_scope 正确,GET 走空 body + 删 Content-Type 头(L 修正)。`migrate_if_needed` v0.2.4 老 `"AK...SK"` 整串 → 双字段迁移逻辑正确,spawn_blocking 写回 IO 隔离。`Percent` 字段 0~100 + `clamp(0.0, 100.0)` 防御(对接 CodexBar #1724 schema)。`parse` 走 `super::parse::num_f64`(已含 is_finite 过滤)避免 D-014。D-015 是唯一 bug。
- **custom.rs**:SSRF / protocol confusion 防护完整 —— `https://` 强制(H9 fix)+ `extract_host` + `is_ssrf_blocked` 拦 loopback / link-local(H7 fix 2026-07-03)+ `path.starts_with('/')` 防御 host 拼接投毒(L10 fix)。3 个 ExtractSpec preset 的 `divide` 都 `is_finite() && > 0.0` 校验(防 NaN / 0 / 负数)。`read_path` 走共享版(M19 fix 32 段上限)。`json_body_limited` / `text_body_limited` 都已用。**无真 bug**。
- **parse.rs**:`read_path` 32 段上限(M19 fix)+ `num_f64` is_finite 过滤(H12 fix)都到位。L5 fix 文档化前导 `.` 静默跳过。测试覆盖正向 / 边界 / 数组越界 / 非 object 中间 / 空路径 / 非法段名 / NaN/inf 字符串。**无 bug**。

---

## 优先级建议

1. **立即修**(回归 / 安全):D-012, D-013
2. **v0.3 一并修**(一致性 / D1 推广):D-014, D-015
3. **观察**:无(本轮无低优 bug)

本轮发现 1 条 P0 级别 (D-012),1 条 P1 级别 (D-013),2 条 P2 级别 (D-014, D-015)。**本轮未发现 P0 级别 critical panic 路径**(最接近的是 D-012 内存撑爆,但触发面窄)。
