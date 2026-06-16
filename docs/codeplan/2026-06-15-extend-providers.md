# Code Plan: 扩展 Musage quota source + 设置面板重构

> 日期: 2026-06-15（创建） / 2026-06-16（PR 2 完成更新）
> 状态: **PR 2 ✅ 完成。PR 3 待执行。**
> 原始 plan 来源: 维护者在 chat 中粘贴的 14KB 草稿。

## 进度

| PR / Phase | 内容 | 状态 | Commit |
|---|---|---|---|
| **PR 1** = Phase 0 | AGENTS.md 同步 + API schema 调研 | ✅ 完成（发现：stepfun 需 OAuth login / novita+qwen 无 public quota API） | (AGENTS.md 已含) |
| **PR 2** = Phase 1 + Phase 2 | 5 个新内置 provider | ✅ **完成**（2026-06-16） | `aabe823` (backend) + `174b5de` (frontend) |
| **PR 3** = Phase 3+4+5 | CustomSource + IPC + 设置面板重构 | ⏳ 待执行 | — |
| 后续 | Phase 6（test/docs）+ Phase X（StepFun 3 步 OAuth + Novita/Qwen 真实 fetch） | — | — |

### PR 2 实际交付（2026-06-16）

按 plan §1 目标，Musage 现有 **13 个 quota source**（8 已存 + 5 新增）。

**5 个新 provider**（commit `aabe823`）：
- [siliconflow.rs](../../src-tauri/src/providers/siliconflow.rs) — Bearer, schema 100% 确认（官方 API ref 实测）。9 单测。
- [claude_official.rs](../../src-tauri/src/providers/claude_official.rs) — Cookie + `oauth-2025-04-20` beta header；429 频发不重试；sessionKey 智能归一化。14 单测。
- [stepfun.rs](../../src-tauri/src/providers/stepfun.rs) — Oasis-Token 手动粘贴模式。11 单测。3 步 OAuth auto-login 留 TODO。
- [novita.rs](../../src-tauri/src/providers/novita.rs) — **STUB**：公开 API 无 quota endpoint（实测确认）。3 单测。
- [qwen.rs](../../src-tauri/src/providers/qwen.rs) — **STUB**：CodexBar issue #612 实测无 quota API。3 单测。

**前端适配**（commit `174b5de`）：
- `types.ts` / `logos.ts` / `credentials.ts`：加 5 个新 source id / 占位符 / 帮助文本 / accent 色
- `src/main.ts`：浮窗 `PROVIDER_META` + `ProviderSnapshot.provider` type union 加 5 个
- **`BUILTIN_ORDER` 改造为 `currentKnownIds` 动态派生**（[src/settings/utils.ts](../../src/settings/utils.ts)）—— 移掉 8 个写死 id，改成从 `SourceMeta[]` 派生。**PR 3 加 CustomSource 时零代码改动**自动出现在浮窗顺序里。
- `AGENTS.md` 同步 13 provider 列表 + 文件结构

**验证**：
- `cargo check` 0 错（12 个 pre-existing warning）
- `cargo test providers::siliconflow/claude_official/stepfun/novita/qwen`：**41 passed; 0 failed**
- `pnpm tsc --noEmit`：0 错
- `pnpm vite build`：通过（settings.js 49.58 → 49.67 kB，+90B）

### 关键发现与决策

1. **StepFun API**：plan 假设 Bearer + GET 一个端点，实际需 Oasis-Token Cookie + POST 两个端点（参考 [CodexBar docs/stepfun.md](https://github.com/steipete/CodexBar/blob/main/docs/stepfun.md)）。3 步 OAuth auto-login 工作量 1 周+ → 暂不做，留 TODO。
2. **Novita / Qwen**：plan URL 是猜测，实际**不存在** public quota API。改 STUB 模式：UI 可见，fetch 永久返 "未支持" 错。
3. **claude_official**：需 `Anthropic-Beta: oauth-2025-04-20` header + `User-Agent: claude-code/<ver>`。**429 频发**（[claude-code#31021](https://github.com/anthropics/claude-code/issues/31021)），不重试。
4. **`BUILTIN_ORDER` 硬编码问题**：原本写死 8 个 id，未来 PR 3 加 `custom_<uuid>` 不可维护。改成从 `SourceMeta[]` 动态派生，**自适应**。

---

## 0. 现状摸底

`src-tauri/src/providers/` 实际有 8 个 provider（AGENTS.md 第 29 行 "6 个" 是过期信息）：

| Provider | 文件 | 鉴权 | 模式 | 状态 |
|---|---|---|---|---|
| minimax | minimax.rs | Bearer | Token Plan | ✅ |
| deepseek | deepseek.rs | Bearer | 余额 | ✅ |
| xiaomimimo | xiaomi.rs | Bearer→Cookie fallback | 套餐+总额度 | ✅ |
| tavily | tavily.rs | Bearer | Credits | ✅ |
| zenmux | zenmux.rs | Bearer | PAYG/Subscription | ✅ |
| openrouter | openrouter.rs | Bearer | 余额 | ✅ |
| kimi | kimi.rs | Bearer | Token Plan | ✅（2026-06-14） |
| zhipu | zhipu.rs | Bearer | Token Plan | ✅（2026-06-14） |

kimi 和 zhipu 带完整单元测试、region 切换、fallback 策略，**模式成熟**，可作为新 provider 的参考模板。

---

## 1. 目标

完成后 Musage 总共支持 **15 个 quota source**：

- **8 个内置**（已存在）：minimax / deepseek / xiaomimimo / tavily / zenmux / openrouter / kimi / zhipu
- **4 个新增内置**：stepfun / siliconflow / novita / qwen
- **1 个官方 OAuth**：claude_official（Cookie 鉴权）
- **1 个用户自定义**：custom_<uuid>（New API 通用，用户在设置面板里加/改/删）

设置面板：解决"15 个 provider 平铺挤乱"，重构为 **分组 + 折叠 + 搜索**。

---

## 2. Phase 0: 前置查漏（半天）

**目标**：确保 kimi/zhipu 已完整接通，避免 Phase 1 踩坑。

### 2.1 后端冒烟

```bash
cmd /c "dev-env.bat && cd src-tauri && cargo check"
# 期望：0 错 0 新警告
```

### 2.2 前端冒烟

```bash
cmd /c "dev-env.bat && pnpm tauri:dev"
# 设置面板勾选 kimi/zhipu → 填真 key → 验证 2 行（5h + 周）显示
# 反复切换 zhipu region (cn/en) → 验证 endpoint 刷新
```

### 2.3 config.json 默认值校验

打开 `%APPDATA%\com.musage.app\config.json`，确认：
- `providers.kimi.enabled = true`
- `providers.zhipu.enabled = true`
- `zhipu_region = "cn"`（默认）
- 无遗留旧字段（BTreeMap 兼容即可）

### 2.4 AGENTS.md 同步

把 kimi/zhipu 加进 AGENTS.md 第 29 行的 "Providers" 列表 + §文件结构 的 `providers/` 树。

### 2.5 **Pre-Phase 0 必做** (review 补充)

跑 3 个 curl 验证 URL，避免 Phase 1 变调研坑：

```bash
curl -H "Authorization: Bearer $STEPFUN_KEY" https://api.stepfun.com/v1/user/info
curl -H "Authorization: Bearer $SF_KEY"       https://api.siliconflow.cn/v1/user/info
curl -H "Authorization: Bearer $NOVITA_KEY"   https://api.novita.ai/v1/user/balance
# 看返回字段，标进 §3 表格的 "响应字段" 行（不能再写"待实测"）
```

---

## 3. Phase 1: 4 个新内置 provider 的 Rust 实现（2-3 天）

> Doubao 在这里**故意不写**——见 §4。

### 3.1 通用实现骨架

```rust
//! <Provider> 用量查询
//!
//! Endpoint: GET <URL>
//! Auth: <header scheme>
//!
//! ## Response schema (基于实测)
//! ```json
//! { ... }
//! ```
//!
//! ## 渲染策略
//! - 第一行: ...
//! - 第二行: ...

use std::pin::Pin;
use serde_json::Value;
use super::{shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};

const URL: &str = "https://...";

pub struct XxxSource { /* 状态字段 (region / mode / base_url 等) */ }

impl Default for XxxSource { ... }

impl QuotaSource for XxxSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("xxx") }                    // 改: &'static str → Cow
    fn display_name(&self) -> Cow<'_, str> { Cow::Borrowed("Xxx") }          // 改
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(&'a self, cfg: serde_json::Value) -> ... {
        Box::pin(async move { ... })
    }

    fn fetch<'a>(&'a self, credentials: &'a Credentials) -> ... {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured("未配置 Xxx API key"));
            }
            do_fetch(api_key, /* state */).await
        })
    }
}

async fn do_fetch(api_key: &str, /* ... */) -> Result<ProviderSnapshot, FetchError> {
    // 1) shared_client().get(URL).header(...)
    // 2) status check (401/403/5xx)
    // 3) resp.json()
    // 4) parse(&raw)
}

fn parse(raw: &Value, /* ... */) -> Result<ProviderSnapshot, FetchError> {
    // 1) 校验 success / 业务层 error code
    // 2) 提取核心字段 (用 num_f64 容错数字/字符串)
    // 3) 组装 QuotaRow[] (utilization / remaining / used / total / resets_at / unit)
    // 4) 返回 ProviderSnapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test] fn parse_full_response() { ... }
    #[test] fn parse_missing_field_is_error() { ... }
    #[test] fn extract_reset_ms_handles_iso_string() { ... }
    // 至少 3 个测试
}
```

### 3.2 各 provider 详情

#### 3.2.1 StepFun（阶跃星辰）

| 项 | 值 |
|---|---|
| URL | `https://api.stepfun.com/v1/user/info`（Pre-Phase 0 验证后填实） |
| Auth | `Authorization: Bearer <key>` |
| 响应字段 | 实测后填 |
| 模式 | 第三方余额（类 DeepSeek） |
| QuotaRow | 主行：余额 + currency |
| 工作量 | 0.5 天 |

#### 3.2.2 SiliconFlow（硅基流动）

| 项 | 值 |
|---|---|
| URL | `https://api.siliconflow.cn/v1/user/info` |
| Auth | `Authorization: Bearer <key>` |
| 模式 | 第三方余额 |
| QuotaRow | 主行：余额 + CNY |
| 工作量 | 0.5 天 |

#### 3.2.3 Novita AI

| 项 | 值 |
|---|---|
| URL | `https://api.novita.ai/v1/user/balance` |
| Auth | `Authorization: Bearer <key>` |
| 模式 | 第三方余额 |
| QuotaRow | 主行：余额 + USD |
| 已知坑 | 海外服务，timeout 放宽到 15s |
| 工作量 | 0.5 天 |

#### 3.2.4 Qwen（阿里 DashScope）

| 项 | 值 |
|---|---|
| URL | `https://dashscope.aliyuncs.com/api/v1/account/quota` |
| Auth | `Authorization: Bearer <key>` |
| 模式 | Token Plan（类 MiniMax） |
| QuotaRow | 多行，每行一个 model |
| 工作量 | 1 天（含 schema 调研） |

---

## 4. Doubao (火山方舟) — 暂缓，移入 [FUTURE.md](../../FUTURE.md)

**原因**：IAM 签名是 AWS-style V4 变体，光是 RFC + 单测就要 1 天，**还没算调通**。原始 plan 估 "2 天" 严重偏低。

**不砍的理由**：用户确实可能用。
**暂缓的理由**：ROI 不高，杠杆在 CustomSource（10+ 个中转站一次吃）。

如果将来真要做：
- 需要 `hmac` + `sha2` + 自己拼 canonical request
- 需要沙箱账号（不污染生产 IAM policy）
- 工作量重估：**1-2 周**，不是一个 Phase 的事

---

## 5. Phase 2: Claude Code 官方（1 天）

| 项 | 值 |
|---|---|
| URL | `https://api.anthropic.com/api/oauth/usage` |
| Auth | `Cookie: sessionKey=<key>`（OAuth session，非 Bearer） |
| 模式 | 官方 OAuth + Cookie（**新 AuthKind**） |
| QuotaRow | 2 行：5h + 7d（类 MiniMax） |
| 已知坑 | Cookie 8h 过期，UI 加 "Cookie 已过期" 提示 + "更新" 按钮 |
| 工作量 | 1 天 |

**实现要点**：
1. `AuthKind` 新加 `ApiKeyOrCookie` 变体（已有），Claude 用这个
2. `Credentials` 已经有 `api_key` + `cookie` 两个字段，**不用改**
3. 必须加 `Anthropic-Beta: oauth-2025-04-20` header
4. doc 告诉用户怎么从浏览器 session 提取 Cookie

---

## 6. Phase 3: 用户自定义 New API source（核心，2-3 天）

> **这是杠杆最高的一块**：一次实现吃掉 dmx / byteplus / lemondata / ctok / silicon / crazyrouter / cubence / dds / runapi / ucloud / shengsuanyun 等 10+ 个 New API 系中转站。

### 6.1 核心抽象: CustomSource 替代 .rs 文件

```rust
// src-tauri/src/providers/custom.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomSourceSpec {
    pub id: String,                  // "custom_a1b2c3d4" (前端 UUID)
    pub display_name: String,        // "DMX API"
    pub base_url: String,            // "https://api.dmx.com"
    pub path: String,                // "/api/user/self"
    pub method: String,              // "GET"
    pub extract: ExtractSpec,        // JSON → QuotaRow 模板
    pub plan_name_path: Option<String>,
    pub accent: Option<String>,      // 前端 fallback 颜色
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractSpec {
    pub main: Option<MainExtract>,
    pub detail_rows: Vec<DetailRowExtract>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainExtract {
    pub remaining: String,           // JSON path: "data.quota"
    pub used: Option<String>,
    pub total: Option<String>,
    pub unit: String,                // "USD" / "CNY" / "credits"
    pub divide: Option<f64>,         // New API 经典 quota/500000 = USD
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailRowExtract {
    pub label: String,
    pub path: String,                // JSON path: "data.search_used"
    pub unit: Option<String>,
}
```

### 6.2 CustomSource 的 QuotaSource 实现

```rust
pub struct CustomSource {
    spec: CustomSourceSpec,
    api_key: OnceLock<String>,
}

impl QuotaSource for CustomSource {
    fn id(&self) -> Cow<'_, str> { Cow::Owned(self.spec.id.clone()) }
    fn display_name(&self) -> Cow<'_, str> { Cow::Owned(self.spec.display_name.clone()) }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn fetch<'a>(&'a self, credentials: &'a Credentials) -> ... {
        // reqwest 调 spec.base_url + spec.path
        // 拿响应后按 spec.extract 转成 QuotaRow[]
    }
}
```

### 6.3 QuotaSource trait 改 `Cow<str>`

```rust
pub trait QuotaSource: Send + Sync {
    fn id(&self) -> Cow<'_, str>;          // 改: &'static str → Cow<'_, str>
    fn display_name(&self) -> Cow<'_, str>; // 改
    // 其他不变
}
```

**传染面 review 必做**（review 补充）：
- [ ] 前端 `src/main.ts` 的 `SourceMeta` / `QuotaSource` 接口
- [ ] 前端 `src/settings.ts` 的 `PROVIDERS` 字典
- [ ] 前端 `as const` / 字面量联合类型
- Phase 0 末尾加 `grep -r "QuotaSource" src/` 摸传染面

### 6.4 New API 通用 schema 适配

ccswitch 的 generic.js 实际响应：
```json
{
  "success": true,
  "data": {
    "id": 1, "username": "user",
    "quota": 50000, "used_quota": 5000,
    "group": "default"
  }
}
```

CustomSourceSpec 默认模板（New API 系）：
- `path`: `/api/user/self`
- `extract.main`:
  - `remaining`: `"data.quota"`
  - `used`: `"data.used_quota"`
  - `total`: `"data.quota + data.used_quota"`  (需表达式求值)
  - `unit`: `"USD"`
  - `divide`: `500000.0`
- `plan_name_path`: `"data.group"`

JSON path parser 实现 30 行 mini 版（参考 minimax.rs 的 num_f64 函数）。

### 6.5 删除的生命周期

- 用户点"删除" → 弹原生 confirm + 二次输入 name（防误删短 id）
- IPC `delete_custom_source(id)`：
  1. 从 `custom_sources.json` 移除
  2. 从 `keys.json` 移除对应 api_key
  3. 从 in-memory registry 移除
  4. emit `musage://config-changed` + `musage://snapshot`
- 批量删除：暂不支持（YAGNI）

### 6.6 文件布局

| 数据 | 文件 | 命名 | 格式 |
|---|---|---|---|
| 内置 provider config | `config.json` | `providers.<id>.*` | JSON |
| 内置 key/cookie | `keys.json` | `<id>` | `{api_key, cookie}` |
| **自定义 spec** | **新增 `custom_sources.json`** | top-level array | `[CustomSourceSpec]` |
| 自定义 key | `keys.json` | `custom_<uuid>` | 同上 |

`custom_sources.json` 独立成文件：避免污染 `config.json`、方便手动编辑 + git diff。

---

## 7. Phase 4: 持久化 + IPC 层（1 天）

### 7.1 新加 IPC commands

```rust
#[tauri::command]
pub async fn list_custom_sources(state: State<'_, AppState>) -> Result<Vec<CustomSourceSpec>, String>;

#[tauri::command]
pub async fn add_custom_source(state, app, spec) -> Result<String, String>;

#[tauri::command]
pub async fn update_custom_source(state, app, spec) -> Result<(), String>;

#[tauri::command]
pub async fn delete_custom_source(state, app, id) -> Result<(), String>;

#[tauri::command]
pub async fn test_custom_source(spec, api_key) -> Result<ProviderSnapshot, String>;
```

### 7.2 builtin_sources() 改造

```rust
pub async fn all_sources(state: &AppState) -> Vec<Box<dyn QuotaSource>> {
    let mut sources: Vec<Box<dyn QuotaSource>> = vec![
        Box::new(minimax::MinimaxSource::default()),
        // ... 8 个内置
    ];
    for spec in state.custom_sources.read().await.iter() {
        sources.push(Box::new(custom::CustomSource::new(spec.clone())));
    }
    sources
}
```

⚠️ 原来 `builtin_sources()` 是同步 `pub fn`，改 async 后要审 7+ 调用方（commands.rs / dump CLI）。

---

## 8. Phase 5: 设置面板重构（2-3 天）

> **review 补充**：原 plan 拆 PR 3+4 独立，但实际应该**合并**——没有 UI 的 CustomSource 用户感知不到，重构完 UI 只改 8 个内置净收益小。

### 8.1 信息架构

```
[Providers Section]
├── [顶部 toolbar]
│   ├── 搜索框（按 display_name 模糊过滤）
│   ├── 计数："启用 7 / 共 15"
│   └── [+ 添加自定义来源] 按钮
├── [分组 1: Token Plan 套餐] (4)
├── [分组 2: 余额查询] (5)
├── [分组 3: 官方/特殊] (3)
├── [分组 4: 用户自定义 New API] (动态 0-N)
└── [Xiaomi MiMo] (单独折叠区, cookie 登录流程特殊)
```

每组默认**展开**，可折叠。

### 8.2 重构策略（最小破坏）

- 渐进式：不一步到位 tab 式（破坏肌肉记忆）
- 改成 grouped + collapsible + search
- 复用现有 `.provider-section` / `.field` / `.help` 样式
- group header 用 `<details>` / `<summary>` 原生 HTML

### 8.3 关键代码骨架

```typescript
const GROUP_DEFINITIONS: Record<GroupKey, { title: string; icon: string; predicate: (meta: SourceMeta) => boolean }> = {
  token_plan: {
    title: "Token Plan 套餐",
    icon: "📊",
    predicate: (meta) => ["minimax", "zhipu", "kimi", "qwen"].includes(meta.id),
  },
  balance: {
    title: "余额查询",
    icon: "💰",
    predicate: (meta) => ["deepseek", "siliconflow", "novita", "stepfun", "openrouter"].includes(meta.id),
  },
  official: {
    title: "官方 / 特殊",
    icon: "🏛️",
    predicate: (meta) => ["tavily", "zenmux", "claude_official"].includes(meta.id),
  },
  custom: {
    title: "用户自定义 New API",
    icon: "🧩",
    predicate: (meta) => meta.id.startsWith("custom_"),
  },
  // review 补充: Xiaomi 必须单独一组, 它的 cookie 登录流程特殊
  xiaomi: {
    title: "Xiaomi MiMo",
    icon: "🍚",
    predicate: (meta) => meta.id === "xiaomimimo",
  },
  misc: {
    title: "其他",
    icon: "🔧",
    predicate: () => true, // catch-all
  },
};
```

### 8.4 搜索框

```typescript
const searchInput = el("input", { type: "search", placeholder: "搜索 provider..." });
searchInput.addEventListener("input", () => {
  const q = searchInput.value.trim().toLowerCase();
  rerenderGroups(); // 只 toggle `hidden`, 不重建 DOM
});
```

### 8.5 自定义来源管理

右侧独立 card，列出所有 custom source + ✏️/🗑️ 按钮。"+" 弹 modal 填表单。

### 8.6 数据流

- 启动: `getConfig() + listSources() + list_custom_sources()` → 合并成 `sources: SourceMeta[]`
- 编辑 key: `setSourceCredential(id, value)` (已存在)
- 改 region/mode: `saveConfig(cfg)` + `setState` (已存在)
- **加 custom source: `add_custom_source(spec)` → emit event 增量更新**（review 补充：不要全量重拉 sources）
- 删 custom source: `delete_custom_source(id)` → 同上

---

## 9. Phase 6: 测试 / CI / 文档（1-2 天）

### 9.1 单元测试（每个新 .rs）

最低 3 个 case：
- `parse_full_response` —— 主路径
- `parse_missing_field_is_error` —— 关键字段缺失
- `parse_business_error` —— API 返回 success=false

参考 kimi.rs / zhipu.rs 的 8-12 个 test case 模式。

### 9.2 实测验证

```bash
cmd /c "dev-env.bat && cd src-tauri && cargo run -- dump --provider <id>"
```

dump CLI 输出 JSON 含 raw response + parsed QuotaRow。手动 eyeball。

### 9.3 前端 e2e（手动）

1. dev 模式启动 → 设置面板
2. 勾选每个新 provider → 填真 key → 验证浮窗
3. 加 custom source → 验证浮窗出现新卡片
4. 改/删 custom source → 验证卡片更新/消失
5. 重启 app → 验证 custom source 持久化
6. 出错情况 → 验证 error_kind 对应 UI

### 9.4 AGENTS.md + README 更新

- AGENTS.md: provider 列表 + 文件结构
- README: 加"添加自定义 New API 中转站"小节, 配 2-3 张截图

---

## 10. 风险与回退

| 风险 | 概率 | 回退策略 |
|---|---|---|
| StepFun/Novita schema 跟预想差太多 | 中 | Phase 0 curl 验证后如实填, Phase 1 真 key 失败就 mark TODO, 不影响其它 |
| CustomSource 的 QuotaSource trait 改造（&'static str → Cow）破坏现有 8 个 .rs | 低 | 改动机械 (Cow::Borrowed); cargo check 0 错就过 |
| **前端 trait 改造传染** | **中** | review 补充: Phase 0 末尾 grep 摸传染面, 准备 type adapter shim |
| 设置面板重构改坏现有用户肌肉记忆 | 中 | 保留所有现有 CSS class / DOM id; 分组加但**不删**旧 panel; 纯增量 |
| user-defined source 的 extract JSON path 太灵活, bug 多 | 高 | **第一版只支持 2 个预设**: New API 系 + 余额系, 后续按需扩展 |
| Doubao IAM 签名 | ~~已排除~~ | 移入 FUTURE.md, 暂缓 |

---

## 11. 总工作量估算

| Phase | 工作量 | 依赖 |
|---|---|---|
| Phase 0: 前置查漏 (含 Pre-Phase 0 curl 验证) | 0.5 天 | — |
| Phase 1: 4 个简单 Bearer provider | 2 天 | Phase 0 |
| Phase 2: Claude 官方 (Cookie) | 1 天 | Phase 0 |
| Phase 3: CustomSource + Cow 改造 | 2-3 天 | — |
| Phase 4: 持久化 + IPC | 1 天 | Phase 3 |
| Phase 5: 设置面板重构 (与 Phase 3+4 合并 PR) | 2-3 天 | Phase 1 + Phase 3 |
| Phase 6: 测试 + CI + 文档 | 1-2 天 | 全部 |
| **总计** | **9-12 天** | |

---

## 12. 建议交付节奏 (review 后调整)

| PR | 内容 | 时间 | 备注 |
|---|---|---|---|
| PR 1 | Phase 0 + AGENTS.md 同步 | 0.5 天 | 不加任何功能, 纯冒烟 + doc |
| PR 2 | Phase 1 + Phase 2 (5 个新 provider) | 3 天 | 端到端可 demo |
| **PR 3** | **Phase 3 + 4 + 5 (CustomSource + IPC + 设置面板重构)** | **4-5 天** | **合并, 端到端可 demo** |
| ~~PR 4~~ | ~~设置面板重构~~ | ~~2 天~~ | **并入 PR 3** |
| ~~PR 5~~ | ~~Doubao (可选)~~ | — | **砍掉, 移入 FUTURE.md** |

每个 PR 都该跑通 `cargo check` + `pnpm tauri:build` (CI 三 OS 全绿) + dev 模式手动验证。

---

## 13. Review notes (2026-06-15, 维护者批注)

review 中发现的关键问题，**已在上面 plan 中直接修改**：

1. **Doubao 工作量低估 50-100%** — AWS-style V4 IAM 签名不是 HMAC-SHA256，估 "2 天" 严重偏低。**砍掉，移入 FUTURE.md**。
2. **PR 3+4 应该合并** — CustomSource + 设置面板重构拆两个 PR 净收益小，且无 UI 时用户感知不到功能。
3. **缺 Pre-Phase 0 curl 验证** — 原 plan 多处标 "URL（猜测）"，应该在 Phase 0 用 curl 30 分钟跑通，避免 Phase 1 变调研坑。
4. **QuotaSource trait 改造的传染面漏前端** — `&'static str` → `Cow<'_, str>` 改 8 个 .rs 是机械的，但前端的字面量联合类型 / `as const` 也会被传染。Phase 0 末尾加 grep 摸传染面。
5. **`cargo check` 期望 "0 警告" 太严** — Tauri 项目 deprecation warning 是常态。改 "0 错 0 新警告"。
6. **Xiaomi MiMo 没写进 GROUP_DEFINITIONS** — 它 cookie 登录流程特殊，predicate 应单独一组。
7. **数据流 "加 custom source → 全量重拉 sources"** — 成本不低，**改 emit event 增量更新**。
8. **extract 模板复杂度选择** — 简单版 (只 2 个预设) 胜出。理由：ccswitch 的 JS extractor 失败率高（用户复制粘贴错的脚本），Musage 走极简模板反而是护城河。

**用户明确回答的两个问题**（见 review 末尾）：
- Doubao: **不常用 → 砍掉**
- extract 模板: **走简单版 (2 个预设)**
