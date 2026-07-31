# Musage 全量代码审查 — Provider 1/6 报告

**范围**：src-tauri/src/providers/ 下 minimax / deepseek / xiaomimimo / tavily / zenmux / openrouter，及 mod.rs 共享 helper (Credentials / AuthKind / ErrorKind / FetchError / classify_http_status / extract_host / is_ssrf_blocked / shared_client / json_body_limited / text_body_limited / read_body_limited / builtin_sources / find_source / all_sources / instantiate_builtin_with_index / QuotaRow / RowKind / ProviderSnapshot / QuotaSnapshot / QuotaSource trait / health_label / worst_health)。

**未审**：kimi / zhipu / claude_official / stepfun / anysearch / siliconflow / volcengine_ark / custom / parse.rs (该文件只被 custom.rs 使用,不在本轮 scope)。其他文件 commands.rs / poller.rs / config.rs / tray.rs 等亦非本轮 scope。

整体判断：**真 bug 较少**——6 个 provider 都通过各自的单测,核心鉴权/schema/百分比/重置时间逻辑都对。多数发现是**一致性缺口**(D5 fix 没推广) 和 **i18n / 测试 fixture 残留**,非可触发生产崩溃。下面按 P0/P1/P2/P3 列出。

---

## P1 (高)

### D-001 — D5 body limit fix 只落地在 minimax,5/6 provider 仍走 `resp.json().await` 无上限
**置信度**:高(已确认) **文件**:minimax.rs:307; deepseek.rs:178; xiaomi.rs:417,565; tavily.rs:184; zenmux.rs:277; openrouter.rs:250,302 **触发条件**:provider 返回 > 8 MiB JSON body(恶意 / 配置被篡改 / 上游代理出错 / 中转站 schema 漂移带大字段)。

**根因**:mod.rs `read_body_limited` / `json_body_limited` / `text_body_limited` + 8 MiB 上限是 D5 fix (2026-07-28) 新加的。mod.rs 顶部注释明确"各 provider 的 success 路径统一用它替代 `resp.json().await`",但实际**只 minimax 改了**(minimax.rs:307)。其它 5 个 in-scope provider 的 `do_fetch` 仍走 `resp.json().await`,等效于把 body 上限交给 reqwest 内部默认缓冲,**完全没限制**。

**影响**:request 端 → response 端都没限 → 用户可控 URL (zenmux `zenmux_base_url`) 配上恶意服务器可撑爆 Musage 进程内存。trusted URL (deepseek/tavily/openrouter/xiaomi 官方域) 影响极小但仍存在 — e.g. Tavily endpoint 引入 usage history 全字段后会触发。

**证据/调用链**:
```
fetch() → do_fetch() → client.get(url).send().await → resp.json().await   [5/6 provider]
```
mod.rs `read_body_limited` 接受 Content-Length 预检 + chunked 流式 + enforce_body_limit 三道闸,具备就绪能力。

**修复建议**:把 5 处 `resp.json().await` 全替换成 `json_body_limited(resp).await?`,把 `resp.text().await` 全替换成 `text_body_limited(resp).await`(用于错误 body 预览的路径,5xx 时仍要保留)。一次 sed 可完成。

**待实测**:Tavily / OpenRouter 真实响应都 < 10 KiB;需要构造一个 > 8 MiB 的 mock 响应验证 5/6 provider 的 panic / OOM 行为。

---

### D-002 — per-provider `num_f64`/`parse_f64` 不过滤 NaN/inf,而共享 `parse::num_f64` 有 `is_finite` 过滤(H12 已修但未推广)
**置信度**:中(可触发条件依赖异常 API 响应) **文件**:deepseek.rs:261-269; tavily.rs:344-352; zenmux.rs:533-540; openrouter.rs:459-470 **触发条件**:API 返回 string 字段如 `"total_balance": "NaN"` / `"usage_percentage": "inf"` / `"limit_remaining": "-inf"`。

**根因**:H12 fix (2026-07-28) 在共享 `crate::providers::parse::num_f64` 加了 `n.filter(|f| f.is_finite())`,但 in-scope 的 4 个 provider 用各自的本地 `num_f64`,字符串分支是 `s.trim().parse().ok()`,会接受 `"NaN"` / `"inf"` / `"-inf"` / `"Infinity"`。`f64::clamp` 对 NaN 是透传(不钳制),`serde_json` 会把 f64 NaN 序列化成 `null`(JS 收到 null 或 NaN),前端 QuotaRow 渲染可能显示 `NaN%` / `inf credits` / 卡死 progress bar。

**影响**:
- trusted provider (deepseek/tavily/openrouter) 概率极低,真实 API 不会返这种字符串
- **zenmux 用户自定义 URL 路径是高危面** — 配置投毒 / 分享配置场景下,attacker 控制的服务器可返 `"balance": "NaN"` 让浮窗渲染异常
- minimax 用的 `num_to_f64` (minimax.rs:651-653) 只走 `as_f64`/`as_i64`,JSON 标准不允许 NaN 字面量,所以**未受影响**

**证据**:
```rust
// deepseek.rs:261-269
fn parse_f64(obj: &serde_json::Value, field: &str) -> Option<f64> {
    obj.get(field).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}
// 没有 .filter(|f| f.is_finite())

// 而共享版 parse.rs 已修:
pub fn num_f64(v: &Value) -> Option<f64> {
    let n = ...; n.filter(|f| f.is_finite())   // ← H12 fix
}
```

**修复建议**:把 4 个 provider 的本地 `num_f64`/`parse_f64` 改成调用共享 `crate::providers::parse::num_f64`(签名兼容,只是参数从 `(obj, field)` 变成 `(value)`)。或者直接 inline `.filter(f64::is_finite)`。

**待实测**:构造 mock 返回 `"limit": "NaN"` 喂给 zenmux + openrouter,看 QuotaRow 的 utilization/remaining 是 NaN 还是被钳。

---

## P2 (中)

### D-003 — OpenRouter `display_name()` 硬编码 "OpenRouter",没用 i18n key
**置信度**:高 **文件**:openrouter.rs:124-132 **触发条件**:zh-CN locale 下,所有 OpenRouter 实例卡片标题始终显示 "OpenRouter" 而非(未来可能翻译为)"开放路由"。

**根因**:minimax/ xiaomimimo/ tavily/ zenmux 都用 `t!("provider_name.<id>")`,只有 openrouter 直接 `Cow::Borrowed("OpenRouter")`。zhipu.rs 的 M8 fix 注释明确说"之前硬编码中文 '智谱 GLM' / 'Z.ai' 破坏 en locale 的 i18n 链路",openrouter 没做这步。

**影响**:en + zh-CN 两个 locale 当前 "OpenRouter" 值相同,所以肉眼无感 — 但**结构性缺陷**:未来 i18n 翻译 "OpenRouter" → "开放路由" 时,openrouter 卡片仍是 "OpenRouter",与全应用其它字符串不同步。回归风险:1 行代码遗漏导致全 locale 不一致。

**证据**:
```rust
// openrouter.rs:124-132
fn display_name(&self) -> Cow<'_, str> {
    if self.instance_index <= 1 {
        Cow::Borrowed("OpenRouter")           // ← 硬编码
    } else {
        Cow::Owned(format!(
            "OpenRouter{}",                    // ← 硬编码
            t!("provider.suffix.dup", n = self.instance_index),
        ))
    }
}
// locales/{en,zh-CN}.json 已存在 "provider_name.openrouter": "OpenRouter"
```

**修复建议**:仿 minimax.rs:154-164 模式替换为:
```rust
fn display_name(&self) -> Cow<'_, str> {
    if self.instance_index <= 1 {
        Cow::Owned(t!("provider_name.openrouter").into_owned())
    } else {
        Cow::Owned(format!(
            "{}{}",
            t!("provider_name.openrouter").as_ref(),
            t!("provider.suffix.dup", n = self.instance_index),
        ))
    }
}
```

---

### D-004 — DeepSeek balance_infos 单条缺全部字段时,fallback 强推 `Some(0.0)`,显示假 "0.00 CNY 余额"
**置信度**:中 **文件**:deepseek.rs:197-209 **触发条件**:DeepSeek 返回 `balance_infos: [{}]` 或某条 info 对象里 `total_balance` / `granted_balance` / `topped_up_balance` 三个字段**同时缺失**(schema 漂移 / 用户 key 异常 / 部分受限账号)。

**根因**:H5 fix 注释说"余额真为 0 时返 Some(0.0) 让用户看到 '余额:0.00' 而非 '字段缺失'",但 fallback 闭包**不区分**"字段值为 0" 与 "字段不存在" — 两个 `unwrap_or(0.0)` 把"字段缺失"也当成 0 算,g + t = 0.0 → 永远返 `Some(0.0)` → 推一行假的 0.00 CNY 余额。

**影响**:用户看到 "0.00 CNY 余额" 误以为钱包清空,实际是 API schema 漂移 / 字段缺失。**应该归 Parse 错**让用户去 schema_overrides。

**证据**:
```rust
// deepseek.rs:197-209
let total_balance = parse_f64(info, "total_balance")
    .or_else(|| {
        let g = parse_f64(info, "granted_balance").unwrap_or(0.0);  // None → 0
        let t = parse_f64(info, "topped_up_balance").unwrap_or(0.0); // None → 0
        Some(g + t)                                                   // 永远 Some
    });
rows.push(QuotaRow { remaining: total_balance, ... });
```

**修复建议**:把 fallback 改成"3 字段全 None 才报 Parse,部分缺失时回退到非零字段":
```rust
.or_else(|| {
    let g = parse_f64(info, "granted_balance");
    let t = parse_f64(info, "topped_up_balance");
    match (g, t) {
        (Some(g), Some(t)) => Some(g + t),
        (Some(g), None) => Some(g),
        (None, Some(t)) => Some(t),
        (None, None) => None,  // ← 真缺数据,fallback 返 None,parse_f64 的外层
                                //    .or_else 链也返 None → rows.push 跳过这条
    }
})
```
然后外层加 `if rows.is_empty() { return Err(Parse); }`(已存在)。

**待实测**:构造 `{"balance_infos": [{"currency": "CNY"}]}` 喂 do_seek,确认浮窗是否显示假 0.00。

---

### D-005 — `worst_health` 把"全 unknown"误判为 "ok",tray 显示绿点
**置信度**:中(代码必现,但 6 in-scope provider 都达不到触发条件) **文件**:mod.rs:553-572 (worst_health) + mod.rs:472-503 (health_label) **触发条件**:所有 enabled provider 都返回 `success=true && rows.is_empty()`。

**根因**:`worst_health` 初始 `worst = "ok"`,循环 match 表 `(worst, h)`:
- `(_, "alert")` → alert
- `("ok", "warn")` → warn
- `(a, b) if a == b` → a
- `_ => worst`

`"unknown"` 没有 case 匹配 → 落到 `_ => worst`,worst 保留 "ok"。所有 provider 都 "unknown" 时最终返 "ok"。

**实际触发可能性**:
- 6 个 in-scope provider 全部 `success = !rows.is_empty()` 或强制 success=false(套餐过期 / Parse 错)→ **不可达**
- `ProviderSnapshot::placeholder()` 也是 success=false → "alert"
- 所以 in-scope 6 个 provider 不会触发,**只是 dead branch**

**影响**:目前是**潜在回归风险**——v0.3 引入新 STUB provider 或 schema 大改后,如果某个 provider 真的 `success=true && rows=[]`,tray 会突然从"红点(unknown 是中间色)"变"绿点(ok)"。无显式测试锁定 health_label 的 "unknown" 语义。

**修复建议**:`worst_health` 初始值改 `"unknown"`,并在 match 表加 `("unknown", x)` 行把 "unknown" 视为中位 — 具体优先级 ok < warn < unknown < alert 可议,但要让 "unknown" 不是 "ok" 的等价物。或加文档说明 "unknown 故意走 ok 兜底"。建议改 init + 加测试覆盖 "all unknown" case。

**待实测**:构造一个虚拟 QuotaSource trait impl(成功路径返 0 rows),挂上后看 tray color。

---

## P3 (低)

### D-006 — Zenmux `parse_iso8601_ms` 只解析 RFC3339,无 NaiveDateTime fallback(Xiaomi 有)
**置信度**:低 **文件**:zenmux.rs:528-531 vs xiaomi.rs:889-901 **触发条件**:Zenmux API `quota_5_hour.resets_at` / `quota_7_day.resets_at` 字段返回 `"2026-03-24 08:35:09"`(无 `T` 无 `Z`)而非 RFC3339。Zenmux 当前 schema 用 `"...000Z"`,但**没有 SLA 文档承诺**,且 xiaomi 已显式做了 fallback 说明 dashboard 历史上有无时区 suffix 的数据。

**影响**:5h/7d 行的 resets_at 变 None → 前端倒计时消失,只显示利用率百分比。不致命。

**修复建议**:仿 xiaomi `parse_datetime_utc_ms`,加 NaiveDateTime 路径 + warn log 提示 schema 漂移。

---

### D-007 — Tavily 测试 `parse_full_response` 硬编码 "Free tier" / "credits" 字符串,没走 `t!()`
**置信度**:高(代码必现) **文件**:tavily.rs:401-463 **触发条件**:`cargo test` 时 locale != en(目前 en 是 rust-i18n 默认 + fallback,所以本地/CI 默认跑过;但 v0.3 加 set_locale 切换测试时会破)。

**根因**:
```rust
// tavily.rs:415
assert_eq!(main.label, "Free tier");    // ← 硬编码
// tavily.rs:416
assert_eq!(main.unit.as_deref(), Some("credits"));  // ← 硬编码
```
而 minimax / xiaomi 测试都用 `t!("row.xxx")`,这样 locale 切换时一致。Tavily 测试是早期 v0.1 留下的,跟 i18n refactor 不齐。

**影响**:仅测试,生产代码不受影响。

**修复建议**:`assert_eq!(main.label, t!("row.free_tier"));` `assert_eq!(main.unit.as_deref(), Some(t!("row.credits").as_ref()));`。同时给 `parse_research_plan_limit_fallback` 同样修。

---

### D-008 — OpenRouter `fetch_credits` 5xx 用 `http_error_simple`(无 body),`fetch_key` 用 `http_error`(带 body)
**置信度**:高 **文件**:openrouter.rs:241-247 vs openrouter.rs:286-294 **触发条件**:任一 endpoint 返 5xx / 4xx(非 401/403/429)。

**根因**:`fetch_credits` 的 5xx 分支忘记 `let body = resp.text().await.unwrap_or_default();` 然后塞进 error 消息。`fetch_key` 是齐的。两个函数早期是复制粘贴出来的,5xx 分支各自演进时漂移。

**影响**:用户看到 `/credits` 端点 502 时只显示 "HTTP 502 (OpenRouter)" — 没法看 body 诊断;`/key` 端点同样的错会带 body 上下文。

**修复建议**:`fetch_credits` 仿 `fetch_key` 改成 `http_error`(带 body)。两行 patch。

---

### D-009 — OpenRouter `LAST_SUCCESSFUL` 静态 HashMap 不主动 evict,只按 TTL 过期
**置信度**:低 **文件**:openrouter.rs:63-71 **触发条件**:用户频繁创建/删除 extra instances(每次新增 `openrouter#N` 都加一条)。

**根因**:HashMap 容量随 instance 数量单调增长,5 分钟 TTL 只让"值过期",但 key 不删。N=10 个实例 → 永久占 10 槽,内存 < 1 KB,实际无害。

**影响**:无 — bounded by 用户配置上限,内存忽略不计。

**修复建议**:v0.3 cleanup 时加 `retain(|_, (ts, _)| ts.elapsed() < Duration::from_secs(600))` 在 `remember_endpoint` / `clear_endpoint_cache` 里。当前可不动。

---

### D-010 — Tavily `resets_at` 解析只接受 `NaiveDate`(`%Y-%m-%d`),ISO 8601 带时间组件静默失败
**置信度**:低 **文件**:tavily.rs:220-228 **触发条件**:Tavily API 把 `account.current_billing_period.end` 改成 `"2026-07-01T00:00:00Z"`(目前是 `"2026-07-01"`)。

**根因**:`NaiveDate::parse_from_str(s, "%Y-%m-%d")` 对 ISO 8601 with time 返回 Err → `ok()` → `map` 不执行 → None。Tavily 当前文档承诺 `%Y-%m-%d`,但跟 D-006 同款"没 SLA 承诺"风险。

**影响**:`resets_at = None` → 浮窗 5h 行不显示倒计时。不致命。

**修复建议**:加 RFC3339 fallback:`chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())`,先试 ISO 8601 再回退 NaiveDate。

---

### D-011 — Minimax `smart_reset_to_ms` 不拒绝负 duration(`end_time = -5` 变过去时间戳)
**置信度**:低 **文件**:minimax.rs:643-654 **触发条件**:MiniMax API 返 `end_time` 字段为负数(时钟漂移 / 重置刚过的窗口 / schema bug)。

**根因**:`(EPOCH_MS_MIN..=EPOCH_MS_MAX).contains(&raw)` 检查负数不通过 → 走 "duration-seconds" 分支 → `now + raw * 1000 = now - 5000`。前端 `resetsPrefixFor` 把过去时间显示为"X 秒前重置"或"已重置"。

**影响**:UI 显示诡异但不致命。Negative duration 在真实生产环境罕见。

**修复建议**:在 duration 分支加 `if raw < 0 { return None; }`(或返 `now` 表示"立即重置")。

---

## 审过的路径(显式声明,确认无 issue)

- **mod.rs `extract_host` + `is_ssrf_blocked`**:已由 D2 fix (2026-07-28) 修过,新增 IPv6 `]` 闭合 + `0.0.0.0` / `[::]` / `::ffff:127.0.0.1`。6 个单测覆盖正向 + 边界。OK。
- **mod.rs `shared_client`**:10s timeout + 5s connect + pool_max_idle_per_host(2) + pool_idle_timeout(30s)。M9 fix 已在。无 issue。
- **mod.rs `json_body_limited` / `read_body_limited`**:实现正确,Content-Length 预检 + chunked 流式 + enforce_body_limit。**只是没被 in-scope 6/6 的大多数采用(D-001)**。
- **mod.rs `classify_http_status`**:逻辑正确(429/401/403/5xx → 4 个分支)。**只是 in-scope 6 个 provider 都没用,各自手写 4 个 if(也是 D-001 同款推广缺口)**。
- **mod.rs `QuotaSource` trait + `builtin_sources` / `find_source` / `all_sources` / `instantiate_builtin_with_index`**:13 provider 全注册(含任何 search + volcengine_ark),instance_index 走 `with_instance_index` 路径正常,`unique_id()` 实现正确。
- **mod.rs `health_label` / `worst_health`**:health_label 自身 OK;worst_health 的 unknown 处理见 D-005。
- **mod.rs `ErrorKind` + `FetchError`**:9 个变体完整覆盖,`Display` / `std::error::Error` 实现正确。
- **mod.rs `QuotaRow` + `RowKind`**:5 变体,`#[serde(default, skip_serializing_if)]` 兼容老 snapshot。Xiaomi / minimax 正确填 kind。
- **mod.rs `ProviderSnapshot`**:20 个字段,`transient` / `next_fetch_at` 都是 Optional 兼容。`empty_error` / `placeholder` 路径正确。
- **minimax**:`parse_tier_percent` / `parse_tier_count` / `smart_reset_to_ms` 全部正确,BUG-002 fix 已落(status=2/3 严格 drop),5h 周独立 gate 正确。`parse_tier_count` 用 `num_to_f64` 不接受 NaN 字符串(JSON 标准不允许 NaN 字面量)。
- **deepseek**:核心 schema 解析正确,fallback 到 granted+topped_up 太激进见 D-004。
- **xiaomimimo**:BearerThenCookie fallback 正确,H14 fix 防 panic,is_html_error_page 401 分支正确,cookie format 校验完整,display_mode 应用正确,plan_expired 路径独立。
- **tavily**:核心 schema 解析正确,limit null fallback to Researcher/Research=1000,utilization clamp 防止越界,endpoint 子项全部填。
- **zenmux**:PAYG / Subscription 两模式 schema 正确,URL scheme + SSRF 校验齐,success 严格 bool 检查,usage_percentage heuristic + clamp 防止 7200% 越界。
- **openrouter**:`/credits` → `/key` fallback 链路正确,`LAST_SUCCESSFUL` 按 unique_id 分桶(C2 fix),AuthFailed 清缓存,`fetch_key` free_tier + 无 remaining fallback (H8 fix) 正确。

---

## 优先级建议

1. **立即修**(回归风险/可触发):D-001, D-003
2. **v0.3 一并修**(tech debt):D-002, D-004, D-005, D-008
3. **机会修 / 观察**:D-006, D-007, D-009, D-010, D-011

本轮未发现 P0(critical)级别 bug。