# Musage 迭代路线图：从 AI Token Plan Monitor 到通用 Quota Monitor

> Status: draft (2026-06-10)
> Owner: Thedeergod666
> Reviewers: TBD

---

## 1. 背景 (Background)

Musage 当前定位：**AI token plan 实时用量监控的桌面悬浮窗**。
已支持 3 个 provider：MiniMax（公开 quota API）、DeepSeek（公开钱包 API）、Xiaomi MiMo（dashboard admin API，cookie auth）。

在调研 ccswitch 的 `coding_plan.rs` 过程中发现：

- ccswitch 支持 **6 个** Token Plan provider（Kimi / Zhipu×2 / MiniMax×2 / ZenMux），都是公开 quota API
- MiniMax 和 DeepSeek 能"只填 API key 就跑"，是因为它们**恰好**暴露了公开 quota endpoint
- Xiaomi MiMo 是反例——只暴露 dashboard admin API（cookie 鉴权），无公开 endpoint

进一步调研发现 **quota / usage 监控是更普遍的需求**：

| 服务类型 | 用户用得上吗 | 公开 quota API 吗 |
|---|---|---|
| LLM token plan（MiniMax / Zhipu / Kimi） | ✅ 刚需 | ✅ 多数是 |
| 搜索 API（Tavily / Exa / Brave） | ✅ 刚需 | ✅ Tavily 就有 `/usage` |
| 通信 API（Twilio / SendGrid） | ✅ 实用 | ✅ `/Balance` 类 |
| 支付 / 账单（Stripe） | ⚠️ 公司用 | ✅ `/balance` |
| GitHub | ⚠️ 个人 | ✅ `/rate_limit` |
| Vercel / Cloudflare | ⚠️ 自部署用 | ✅ `/v1/usage` |

→ 结论：**Musage 的核心抽象（"token plan 用量卡片"）可以泛化成"任何服务的用量卡片"**。同样的悬浮窗 UI，同样的 schema，同样的鉴权机制——只是 endpoint 和 extractor 不同。

---

## 2. 目标 (Goals)

**G1 — 复用**：Musage 现有架构（悬浮窗 + 后台轮询 + 设置面板 + token / cookie 存储）全部保留，只把"provider 枚举"换成"通用 quota source 注册"。

**G2 — 新增 Tavily**：作为第一个**非 AI** provider，验证通用抽象工作。1 小时内能跑起来。

**G3 — 加回掉队的**：Zhipu GLM（CN+EN）/ Kimi / ZenMux，照搬 ccswitch `coding_plan.rs` 的实现。

**G4 — 用户可扩展**：UI 里能加"自定义 provider"，填 URL + 鉴权 + JSONPath extractor，无需重新编译。

## 3. 非目标 (Non-Goals)

❌ **不做自动登录 / webview cookie capture**：Xiaomi cookie 痛点真实但工程量大，单独里程碑。

❌ **不做 JS 沙箱（boa_engine / rquickjs）**：引入 10MB 启动体积。Phase 4+ 再说，先用 JSONPath。

❌ **不做用量历史 / 趋势图 / 成本预测**：Musage 是"看一眼"，不是"BI 工具"。超出监控范畴。

❌ **不做支付集成 / 自动充值**：只监控，不操作。

❌ **不做配额预警通知**：先做准确展示，未来加 system notification。

---

## 4. 现状 (Current State)

### 后端结构（src-tauri/src/）

```
commands.rs         # tauri command handler
config.rs           # 配置 + keys.json
lib.rs              # tauri Builder + dump CLI
poller.rs           # tokio interval 后台轮询
tray.rs             # 系统托盘 + 动态图标
providers/
  mod.rs            # Provider enum + QuotaRow + QuotaSnapshot + ProviderSnapshot + ErrorKind
  minimax.rs        # 实现
  deepseek.rs       # 实现
  xiaomi.rs         # 实现 (Cookie auth)
```

### 核心类型（现有）

```rust
enum Provider { Minimax, Deepseek, Xiaomimimo }

struct QuotaRow {
  label: String,
  utilization: Option<f64>,    // 0-100 已用百分比
  remaining: Option<f64>,     // 原始数字（暂未用）
  total: Option<f64>,
  resets_at: Option<i64>,
  unit: Option<String>,
  extra: Option<serde_json::Value>,
}

struct ProviderSnapshot {
  provider: Provider,
  success: bool,
  rows: Vec<QuotaRow>,
  error: Option<String>,
  error_kind: Option<ErrorKind>,
  fetched_at: Option<i64>,
  raw: Option<serde_json::Value>,
  is_healthy: bool,
}

struct QuotaSnapshot {
  providers: Vec<ProviderSnapshot>,
  fetched_at: Option<i64>,
}
```

### 前端结构

- `src/main.ts`：悬浮窗，渲染 `snap.providers` 数组为卡片
- `src/settings.ts`：3 个 tab，按 provider 分
- `src/settings.html`：3 个 `.provider-panel`

### 痛点

1. 每加一个 provider 要改 5+ 个文件（enum + impl + settings + frontend + commands）
2. `Provider` enum 是编译期固定的，运行时加不了
3. `QuotaRow.utilization` 是 0-100%，但 Tavily 这种**显示具体数字**（150/1000 credits）的也想要
4. `provider: "minimax" | "deepseek" | "xiaomimimo"` 是 TypeScript 字面量联合，新增时也得改

---

## 5. 目标架构 (Target Architecture)

### 5.1 核心抽象：把 `Provider` enum 替换成注册表

```rust
// providers/mod.rs（重构后）

/// 单个 quota 来源的"身份 + 鉴权 + endpoint"配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSourceConfig {
  pub id: String,                  // "minimax" / "tavily" / 用户自定义 "my-openai"
  pub display_name: String,         // "MiniMax" / "Tavily"
  pub icon: Option<String>,         // 相对路径 "minimax-logo.png"
  pub auth: AuthSpec,               // 见下
  pub enabled: bool,                // 关掉就不轮询
  pub fetch_interval_secs: Option<u64>,  // 可覆盖全局间隔
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSpec {
  ApiKey {
    header: String,    // "Authorization" 或 "api-key"
    prefix: String,     // "Bearer " 或 "" (Zhipu 不加 Bearer)
  },
  Cookie {
    header: String,    // 固定 "Cookie"
  },
  Bearer { /* 简化路径，等价 ApiKey { header: "Authorization", prefix: "Bearer " } */ },
}

/// 单个 quota 来源的运行时实例（带 fetch 函数）
pub trait QuotaSource: Send + Sync {
  fn id(&self) -> &str;
  fn display_name(&self) -> &str;
  fn icon(&self) -> Option<&str>;
  fn auth_kind(&self) -> AuthKind;   // 给前端 UI 用，决定显示"API Key 输入框"还是"Cookie 输入框"
  fn config(&self) -> &QuotaSourceConfig;

  /// 拉数据。返回 `QuotaSourceSnapshot`（不带 provider enum）。
  fn fetch<'a>(
    &'a self,
    credentials: &'a Credentials,
  ) -> Pin<Box<dyn Future<Output = Result<QuotaSourceSnapshot, String>> + Send + 'a>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
  pub api_key: Option<String>,
  pub cookie: Option<String>,
}

/// 一次 fetch 的结果
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSourceSnapshot {
  pub id: String,                  // "minimax" / "tavily"
  pub display_name: String,         // 冗余存一份，前端不需要二次映射
  pub icon: Option<String>,
  pub success: bool,
  pub rows: Vec<QuotaRow>,
  pub plan_name: Option<String>,    // "Bootstrap" / "Standard 月度套餐"
  pub error: Option<String>,
  pub error_kind: Option<ErrorKind>,
  pub fetched_at: Option<i64>,
  pub raw: Option<serde_json::Value>,
  pub is_healthy: bool,
}

/// 一行展示数据（扩展）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaRow {
  pub label: String,                // "5h" / "周" / "search" / "extract"
  pub utilization: Option<f64>,     // 0-100 已用百分比（可选）
  pub used: Option<f64>,            // 原始已用数字（如 150 credits）
  pub total: Option<f64>,           // 原始总数（如 1000 credits）
  pub unit: Option<String>,         // "%" / "credits" / "USD"
  pub resets_at: Option<i64>,
  pub plan_name: Option<String>,    // 部分 row 单独标记 plan（极少用）
  pub extra: Option<serde_json::Value>,
}

/// 顶层快照（替换 QuotaSnapshot）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSnapshot {
  pub sources: Vec<QuotaSourceSnapshot>,
  pub fetched_at: Option<i64>,
}
```

### 5.2 注册表（替代 enum 分发）

```rust
// providers/mod.rs
pub fn builtin_sources() -> Vec<Box<dyn QuotaSource>> {
  vec![
    Box::new(MinimaxSource::default()),
    Box::new(DeepseekSource::default()),
    Box::new(XiaomimimoSource::default()),
    // Phase 2: 加 Tavily
    // Box::new(TavilySource::default()),
    // Phase 3: 加 Zhipu×2 / Kimi / ZenMux
  ]
}
```

`poller` 和 `commands::refresh_now` 改成：
```rust
for src in builtin_sources() {
  if !src.config().enabled { continue; }
  let creds = load_credentials(src.id())?;
  let snap = src.fetch(&crefs).await;
  push_to_quota_snapshot(snap);
}
```

### 5.3 鉴权统一抽象

当前 3 个 provider 鉴权方式：

| Provider | Header | Prefix |
|---|---|---|
| MiniMax | `Authorization` | `Bearer ` |
| DeepSeek | `Authorization` | `Bearer ` |
| Xiaomi | `Cookie` | (整段) |

→ 抽象成 `AuthSpec` enum。reqwest 调用变成：
```rust
match src.auth_kind() {
  AuthKind::ApiKey => req.header(spec.header, format!("{}{}", spec.prefix, creds.api_key)),
  AuthKind::Cookie => req.header(spec.header, creds.cookie),
}
```

### 5.4 credentials 存储

当前 `keys.json`：
```json
{
  "minimax": "sk-cp-...",
  "deepseek": "sk-...",
  "xiaomimimo": "tp-...",
  "xiaomimimo:cookie": "api-platform_..."
}
```

新 `credentials.json`（命名更准）：
```json
{
  "minimax":    { "api_key": "sk-cp-..." },
  "deepseek":   { "api_key": "sk-..." },
  "xiaomimimo": { "api_key": "tp-...", "cookie": "api-platform_..." },
  "tavily":     { "api_key": "tvly-..." }
}
```

迁移策略：
- 读时优先看新格式，找不到再 fallback 到旧 `keys.json`（仅 `api_key` 字段）
- 写时只写新格式
- `MIGRATION.html` 注释："如果从 0.1.x 升级，第一次保存配置后会自动迁移到新格式"

### 5.5 前端适配

#### `src/main.ts`（悬浮窗）

变化点：
- `QuotaSnapshot.providers` → `QuotaSnapshot.sources`
- 每个 source 多了 `plan_name`、`icon`、`id`
- `QuotaRow` 多 `used` / `total` 字段（之前是 `remaining`）

```typescript
interface QuotaSource {
  id: string;
  display_name: string;
  icon: string | null;
  success: boolean;
  rows: QuotaRow[];
  plan_name: string | null;
  error: string | null;
  error_kind: ErrorKind | null;
  fetched_at: number | null;
}

interface QuotaRow {
  label: string;
  utilization: number | null;    // 0-100
  used: number | null;
  total: number | null;
  unit: string | null;           // "%" / "credits" / "USD"
  resets_at: number | null;
  plan_name: string | null;
}

interface QuotaSnapshot {
  sources: QuotaSource[];
  fetched_at: number | null;
}
```

渲染逻辑新增：
- 有 `used` + `total` + `unit === 'credits'`：显示 `"150/1000 credits"`
- 有 `utilization`：显示进度条（沿用现有逻辑）
- 有 `plan_name`：卡片头部副标题显示

#### `src/settings.ts`（设置面板）

**两套视图**：
1. **内置 provider 列表**（MiniMax / DeepSeek / Xiaomi / Tavily 等）→ 每个一个 tab，照旧
2. **自定义 provider 列表**（未来）→ 折叠区，"Add custom provider" 按钮

---

## 6. 迁移策略 (Migration)

**原则**：向后兼容，至少 2 个版本。

| 数据 | v0.2（当前） | v0.3（Phase 1） | v0.4（Phase 2+） |
|---|---|---|---|
| `keys.json` | flat string map | 同左，标记 deprecated | 移除，旧文件读一次迁移走 |
| `config.json` | `providers: {minimax: {enabled, region}}` | 加 `schema_overrides`（已有） | 加 `custom_sources: Vec<QuotaSourceConfig>` |
| 存储 schema | `Provider` enum | 渐进替换 | 完全用 QuotaSource |

**前端兼容**：老 snapshot 的 `providers[]` 字段保留一段时间，发 deprecated warning。

---

## 7. 迭代阶段 (Phases)

### Phase 0：方案敲定 ✅

- [x] 用户审批 ROADMAP.md
- [x] 决定 Phase 1 范围（最小入侵）

### Phase 1：架构打底 + Tavily（2-3 天）

**目标**：通用抽象跑通，第一个非 AI provider（Tavily）上线。

**任务**：

- [ ] **后端类型重构**：
  - [ ] `providers/mod.rs`：引入 `QuotaSourceConfig` / `AuthSpec` / `Credentials` / `QuotaSourceSnapshot` / 扩展 `QuotaRow`
  - [ ] 保留旧 `Provider` enum 作为薄包装（临时，向前兼容）
  - [ ] 新 trait `QuotaSource`
- [ ] **现有 3 个 provider 迁移到新 trait**：
  - [ ] `minimax.rs` → `MinimaxSource: QuotaSource`
  - [ ] `deepseek.rs` → `DeepseekSource: QuotaSource`
  - [ ] `xiaomi.rs` → `XiaomimimoSource: QuotaSource`（cookie auth 走 `AuthKind::Cookie`）
- [ ] **注册表**：
  - [ ] `providers/mod.rs::builtin_sources()`
- [ ] **poller.rs** 改成遍历注册表
- [ ] **commands.rs::refresh_inner** 改成遍历注册表
- [ ] **Tavily 接入**：
  - [ ] `providers/tavily.rs`：`GET https://api.tavily.com/usage`，Bearer
  - [ ] 解析 `key.{usage,limit,search_usage,extract_usage,crawl_usage,map_usage,research_usage}`
  - [ ] rows：1 个 "free tier" 主行（used/total） + 5 个 endpoint 行
  - [ ] `plan_name = account.current_plan`
- [ ] **前端适配**：
  - [ ] `main.ts`：类型更新 + 渲染支持新字段（plan_name 显示）
  - [ ] `settings.html`：加 Tavily tab（一个 input + 一个 region 下拉无）
  - [ ] `settings.ts`：Tavily loadKeyStatus / saveKey / ProviderId 类型扩展
- [ ] **图标**：
  - [ ] 拿一个 Tavily logo SVG → 放 `src/assets/tavily-logo.png`
- [ ] **验证**：
  - [ ] `cargo check` 通过
  - [ ] `pnpm tauri dev` 跑起来
  - [ ] 浮窗显示 MiniMax + DeepSeek + Tavily 3 张卡片，Tavily 显示 "150/1000 credits"
  - [ ] 切到 Tavily 设置 tab，能填 key + 保存

**不涉及**：用户自定义 provider、JS extractor、Zhipu/Kimi/ZenMux

**回滚方案**：保留旧 `Provider` enum 一段时间，新代码用 enum 映射到新 struct。

### Phase 2：补完 ccswitch 同款 provider（1 天）

**目标**：把 MiniMax / DeepSeek / Zhipu / Kimi / ZenMux 这条线补齐，对齐 ccswitch `coding_plan.rs`。

**任务**：

- [ ] `providers/zhipu_cn.rs` + `zhipu_en.rs`：照抄 ccswitch，鉴权用 `AuthKind::ApiKey { prefix: "" }`（不写 Bearer）
- [ ] `providers/kimi.rs`：`GET https://api.kimi.com/coding/v1/usages`
- [ ] `providers/zenmux.rs`：base_url 用户自定义（合并到 settings）
- [ ] 4 个 provider 加 tab + 图标

### Phase 3：Xiaomi cookie 体验改进（半天）

**目标**：把整段 Cookie 拆成 `access_token` + `user_id` 两个 input，跟 ccswitch 一致。

**任务**：

- [ ] `AuthKind::Cookie` 扩展支持 token 模式：`format!("api-platform_serviceToken={}; userId={}; ...", token, user_id)`
- [ ] settings UI：拆字段
- [ ] keys.json schema 兼容（拆字段，但保留整段 Cookie 的 fallback）

### Phase 4：用户自定义 provider（2 天）

**目标**：UI 里加"自定义 quota source"，无需重编译。

**任务**：

- [ ] **JSONPath extractor**：
  - [ ] 加 `jsonpath_lib` 依赖（轻量）
  - [ ] extractor 配置 schema：
    ```rust
    struct ExtractorConfig {
      url: String,
      used_path: Option<String>,     // "$.key.usage"
      total_path: Option<String>,
      unit: Option<String>,
      resets_at_path: Option<String>,
      plan_name_path: Option<String>,
      rows: Vec<SubRow>,              // 多个子 row
    }
    ```
  - [ ] 通用 fetcher 拿 config + credentials → 跑 extractor → 产出 `QuotaSourceSnapshot`
- [ ] **设置面板**：
  - [ ] "Custom Sources" 折叠区
  - [ ] "Add" 按钮：弹 dialog 让用户填 url + auth + extractor JSON
  - [ ] 列表：每个 custom source 一行 + 删除按钮
- [ ] **存储**：`config.json.custom_sources: Vec<CustomSourceConfig>`
- [ ] **poller**：合并 builtin + custom sources

### Phase 5：Xiaomi webview 自动登录（3-5 天，**单独里程碑**）

**目标**：用户不再需要粘 Cookie。

**任务**（高层设计，详细放 ROADMAP-v2）：

- [ ] 引入 `tauri-plugin-webview`
- [ ] 设置面板 Xiaomi tab 加 "🚀 自动登录" 按钮
- [ ] 隐藏 webview 打开 `https://platform.xiaomimimo.com/login`
- [ ] 用户完成登录（手机号 + 验证码 / 微信扫码）
- [ ] 监听 webview URL 变化，检测登录完成（看 cookie 里是否有 `api-platform_serviceToken`）
- [ ] 抓 cookie 存 keys.json
- [ ] 关闭 webview

### Phase 6：JS extractor（可选，**未来再说**）

只在 Phase 4 的 JSONPath 不够用时引入：
- 引入 `boa_engine` 或 `rquickjs`
- 让用户写 JS extractor（仿 ccswitch）
- 启动体积 +10MB，trade-off 需评估

---

## 8. 风险 & 开放问题

### 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| QuotaSource 抽象泄露（每个实现还是得写很多定制代码） | 抽象失效 | Phase 1 后 review，加 common helper |
| 前端 `providers → sources` 改名导致老配置失效 | 用户报错 | 字段双发（providers + sources），前端 fallback |
| 浮窗渲染逻辑改坏（QuotaRow 多字段后显示错位） | UI 异常 | 小步前进，每个 phase 截图对比 |
| keys.json 迁移丢 key | 用户重新填 | 保留读路径 2 个版本，写只在新格式 |

### 开放问题（Phase 1 前要决定）

| 问题 | 选项 | 我的建议 |
|---|---|---|
| `Provider` enum 完全删除 vs 保留薄包装 | A. 完全删 / B. 保留做兼容性 | **B. 保留 2 个版本** |
| credentials 文件名 | A. `keys.json` / B. `credentials.json` / C. 合并进 `config.json` | **C. 合并** —— 跟 schema_overrides、custom_sources 放一起，文件数减一 |
| `QuotaRow.utilization` 是否变成 `Option<f64>` 范围不限（0-100 还是任意） | A. 限 0-100 / B. 不限 | **A. 限** —— 渲染逻辑简单 |
| 鉴权失败时是否区分 "key 错" / "key 过期" / "rate limited" | A. 加更细 enum / B. 沿用 ErrorKind | **A. 加 `AuthErrorKind`** 子 enum |
| 是否要支持 OAuth flow | A. 现在不做 / B. Phase 4 一起做 | **A. 现在不做** —— 复杂度爆炸 |
| 浮窗卡片的 `display_name` vs `id` —— 哪个给用户看 | A. id / B. display_name | **B. display_name** |
| 自定义 provider 的图标 | A. 强制用户填 SVG / B. 默认无图标 / C. 让用户填 emoji | **C. emoji** —— 最低门槛 |

---

## 9. 成功指标 (Success Metrics)

- **Phase 1 完成**：浮窗同时显示 MiniMax / DeepSeek / Xiaomi / Tavily 4 张卡，Tavily 显示 credits 数字
- **代码量**：Phase 1 后加新 provider 应 < 100 行（含 impl + 注册 + UI tab）
- **向后兼容**：老 keys.json + 老 config.json 在 Phase 1 + Phase 2 都还能读
- **性能**：4 个 provider 并发轮询 < 1s（不串行）
- **崩溃率**：0 panic（鉴权失败 / 网络断开 / schema 变了都不应让浮窗崩）

---

## 10. 时间线

| Phase | 范围 | 估时 | 累计 |
|---|---|---|---|
| 0 | 方案敲定 | 0 | Day 0 |
| 1 | 架构 + Tavily | 2-3 天 | Day 2-3 |
| 2 | Zhipu / Kimi / ZenMux | 1 天 | Day 4 |
| 3 | Xiaomi cookie 拆字段 | 0.5 天 | Day 4.5 |
| 4 | 自定义 provider | 2 天 | Day 6.5 |
| 5 | Xiaomi webview 自动登录 | 3-5 天 | Day 9.5-11.5 |
| 6 | JS extractor | TBD | — |

**MVP（Phase 1-3）**：5 天内，Musage 支持 **8 个** quota source（4 AI + 4 非 AI），Xiaomi UX 改善。

---

## 11. 待用户决策（Phase 1 开始前）

1. ✅/❌ Phase 1 范围 OK 吗？
2. credentials 存 `config.json` vs `keys.json`？我建议合并。
3. Phase 1 是否就先合并不动 keys.json，新加 `credentials.json`？
4. 任何 Phase 内的取舍你想改的吗？