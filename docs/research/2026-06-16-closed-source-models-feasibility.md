# 闭源模型（GPT / Claude / Gemini / Grok）接入 Musage 的可行性调研

> 日期: 2026-06-16
> 范围: 评估 OpenAI / Anthropic / Google / xAI 四家闭源模型官方 quota API 的可接入性，以及中转站生态作为"杠杆方案"的覆盖度
> 目的: 为是否扩展 Musage 接入 4 家闭源模型、扩展哪些、走官方还是走中转站，提供决策依据
> 状态: 调研已完成，结论见 §0 TL;DR。**待维护者拍板**

---

## 0. TL;DR

### 0.1 一句话结论（按用户 friction 排序）

> **用户的明确诉求**：丢个 API key 就拿到 > 点一下按钮浏览器登录 > 自己扒 cookie。任何需要"先做点前置操作"的路径（F3 及以上）都让用户多走弯路。

Musage 应当优先做 **F1 + F1.5 friction 的方案**：

1. **F1.5 OAuth 一键登录**（比小米扒 cookie 还简单——用户点按钮、浏览器登录、自动存 token）
   - **Gemini Code Assist OAuth** — A 评级、零 setup、覆盖 Pro/Ultra/Code Assist 全 tier
   - **Claude OAuth /usage** — C 评级（2026-04 Anthropic 收紧，但浏览器登录 0 friction）
2. **F1 丢 key 即可**
   - **CustomSource NewAPI 通用模板**（已规划）— 一次实现吃掉 15+ 中转站，覆盖 80%+ 国内用户
   - **OpenRouter preset**（Musage 已实现）
3. **F3+ 需要前置操作**（次优）
   - OpenAI Platform admin key（需先建 admin key）
   - xAI Grok（需找 team_id）
4. **D 完全不做**
   - ChatGPT 订阅、X Premium、AI Studio key、sessionKey cookie

**预计 P0+P1 覆盖 90%+ 用户的闭源模型场景**。

### 0.2 用户 friction 分级（决策主轴）

| Friction | 含义 | 用户操作 | 例子 |
|---|---|---|---|
| **F1** | 丢个 key 完事 | 粘贴 → 点保存 | NewAPI 中转、OpenRouter |
| **F1.5** | 一键浏览器登录 | 点按钮 → 浏览器自动登录 → 自动拿 token | Gemini Code Assist OAuth、Claude OAuth /usage |
| **F2** | 自己扒 cookie | 打开 DevTools → 找 cookie → 复制粘贴 | Xiaomi MiMo（Musage 现状） |
| **F3** | 先做点前置操作 | 去 console 建 admin key / 找 team_id / 创 GCP project | OpenAI Platform、xAI Grok、Vertex AI |
| **F4** | 自建 GCP / 配 SA / OAuth client | 5+ 步骤 | Vertex AI、Gemini 自建 OAuth |
| **D** | 不可行 / 风险过高 | — | ChatGPT 订阅、X Premium、AI Studio key |

**维护者 2026-06-16 拍板**：F1 优先于 F1.5，**F1.5 优先于 F2**，F2 优先于 F3+。**任何 F3+ 的 source 必须有强理由**（如覆盖用户群巨大且无 F1/F1.5 替代）。

### 0.3 可行性评级 + friction 一览

| 模型 / 路径 | Friction | 评级 | 关键 endpoint | 鉴权 | 用户操作 |
|---|---|---|---|---|---|
| **NewAPI / OneAPI 兼容中转** | **F1** | A | `GET /api/user/self` | Bearer + `New-Api-User` | 粘贴 api_key + user_id |
| **OpenRouter `/api/v1/key`** | **F1** | A | 已有 | Bearer | 粘贴 key |
| **LiteLLM Proxy `/key/info`** | **F1** | A | `GET /key/info` | Bearer | 粘贴 key |
| **Google Gemini Code Assist OAuth** | **F1.5** | A | `POST cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` | OAuth `cloud-platform` | 点按钮 → 浏览器登录 |
| **Anthropic Claude OAuth usage** | **F1.5** | C | `GET /api/oauth/usage` | Bearer OAuth + `anthropic-beta: oauth-2025-04-20` | 引导用户 `claude login` 一次 → Musage 读 Keychain |
| **OpenAI Platform API** | **F3** | A | `GET /v1/organization/costs` | Admin key (`sk-admin-...`) | 去 console 建 admin key + 粘贴 |
| **xAI Grok Management API** | **F3** | B+ | `GET /v1/billing/teams/{team_id}/prepaid/balance` | Bearer `$XAI_API_KEY` | 粘贴 key + 找 team_id（从 console URL 复制） |
| **Anthropic Claude Admin API** | **F3** | A | `GET /v1/organizations/usage_report/messages` | Admin key (`sk-ant-admin-...`) | 仅 org admin 能建；用户群小 |
| Google Gemini Vertex AI | F4 | C- | Cloud Monitoring API | Service Account JSON | 自建 GCP + SA + 配 IAM |
| OpenAI ChatGPT Plus/Pro 订阅 | — | D | — | — | 无公开 API；cookie 爬有封号案例 |
| Google Gemini AI Studio API key | — | D | — | — | 无 usage endpoint |
| xAI Grok X Premium 订阅 | — | D | — | — | 无公开 quota API |
| Anthropic Claude sessionKey cookie | F2 | D | — | — | Cloudflare `__cf_bm` 战；1 月 cookie 轮换 |

**评级图例**:
- **A** = 官方端点 + 普通/标准鉴权 + 无 ToS 风险
- **B** = 官方端点 + 鉴权略麻烦 / 字段不全
- **C** = 未文档化端点 / ToS 灰区，需要用户知情同意
- **D** = 只能逆向 / cookie 爬 / 公开 ban 案例，**不推荐**

### 0.4 推荐路线图（按"用户 friction + 评级"排序）

| 优先级 | 任务 | Friction | 评级 | 工作量 | 备注 |
|---|---|---|---|---|---|
| **P0** | CustomSource NewAPI 通用模板 | **F1** | A | 2-3 天 | 已规划；吃 15+ 中转站覆盖 GPT/Claude/Gemini/Grok |
| **P0** | OpenRouter preset（已有，需补 preset） | **F1** | A | 0.5 天 | Musage 已实现 source，需接入 CustomSource preset |
| **P1** | **Gemini Code Assist OAuth** | **F1.5** | A | 2 天 | 一键浏览器登录；零 setup；onWatch/caut 完整参考实现 |
| **P1** | **Claude OAuth /usage** | **F1.5** | C | 1.5 天 | 引导用户跑一次 `claude login`；Musage 读 `~/.claude/.credentials.json` 自动 refresh |
| **P2** | OpenAI Platform admin key source | F3 | A | 1 天 | 用户需先建 admin key；UX 加 "如何建 admin key" 教程 |
| **P2** | xAI Grok Management API source | F3 | B+ | 1.5 天 | 需 team_id 自动发现；UX 加 "如何找 team_id" 教程 |
| **P3** | Claude Admin API source | F3 | A | 1 天 | 仅 org admin 能用，用户群小；作为 Claude OAuth 之外的"白名单"备选 |
| **P3** | LiteLLM Proxy preset | **F1** | A | 0.5 天 | `/key/info` schema 简单 |
| ~~不写~~ | ~~ChatGPT 订阅 / X Premium / AI Studio / sessionKey cookie~~ | — | D | — | **风险 > 收益，砍掉** |

**P0 (3 天) + P1 (3.5 天) = 6.5 天覆盖 90%+ 用户的闭源模型场景**。
**含 P2 共 9 天**。

### 0.5 已规划内容的对齐检查

[extend-providers.md](../codeplan/2026-06-15-extend-providers.md) 已规划：
- Phase 2: Claude 官方 OAuth（**已对齐**本文档 §3.2）
- Phase 3: CustomSource NewAPI 通用模板（**已对齐**本文档 §5）

**新增建议**（本文档提出，未在原 plan）：
- P1: Gemini Code Assist OAuth source
- P1: OpenAI Platform admin source
- P2: Claude Admin API source
- P2: xAI Grok Management API source
- P3: LiteLLM Proxy preset

---

## 1. 调研方法

### 1.1 信息来源

- 5 个并行 subagent 通过 Tavily 搜索官方 docs / GitHub / Reddit / 博客，多源交叉验证
- 关键 schema 都附原始 URL（见 §8）
- 参考实现: CodexBar（macOS 30+ provider 工具）、onWatch（Gemini）、cc-switch 通用 extractor

### 1.2 评级口径

- **A**: 官方文档明确支持 + 普通凭据可用 + 无已知 ToS 风险
- **B**: 官方支持但鉴权 / 字段 / 步骤有 friction
- **C**: 端点存在但未文档化 / ToS 灰区，需要用户主动知情同意 + 风险提示
- **D**: 只有逆向 / cookie 爬方案 + 公开 ban 案例 + 不可接受的品牌风险

### 1.3 假设

- 目标用户：Musage 现有用户群体（开发者为主，使用 Claude Code / Codex / Cursor 等 CLI 工具）
- 数据用途：托盘 / 浮窗展示 + 5h/7d/30d 限额提醒
- 实时性要求：5 分钟级别延迟可接受；浮窗不要求秒级

---

## 2. OpenAI (GPT)

### 2.1 OpenAI Platform API（开发者付费）— **A 级**

**端点**: `GET https://api.openai.com/v1/organization/costs?start_time=...&end_time=...&bucket_width=1d&group_by=line_item`

**鉴权**: Admin API key，prefix `sk-admin-...`（**与普通 `sk-...` 推理 key 分开**，用户需在 Platform Settings → Organization → Admin Keys 单独创建，建议 scope `api.usage.read`）

**响应 schema**（from [developers.openai.com](https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs)）:
```json
{
  "object": "page",
  "data": [{
    "object": "bucket",
    "start_time": 1730419200,
    "end_time": 1730505600,
    "results": [{
      "object": "organization.costs.result",
      "amount": {"value": 0.06, "currency": "usd"},
      "line_item": null,
      "project_id": null,
      "api_key_id": null,
      "quantity": null
    }]
  }],
  "has_more": false,
  "next_page": null
}
```

**配套 endpoint**（2024-12 上线）:
- `GET /v1/organization/usage/completions` — token-level 计数（按 model/project/key 维度）
- `GET /v1/organization/usage/embeddings` / `images` / `audio_*` / ... 同族

**QuotaRow 设计**:
- 主行：今日 USD 花费
- 第二行：7 日 / 30 日累计 USD
- `plan_name`: 显示组织名（from admin key）
- 没有"剩余 quota"概念（pay-as-you-go 不限）→ 主行展示 spend

**已知坑**:
- `amount.value` **可能是 string 不是 number**（CodexBar issue #999），解析必须 defensive
- 旧端点 `/v1/dashboard/billing/credit_grants` 在 2024 已禁用普通 key → **不要用旧端点**
- bucket_width 只支持 `1d`（粒度限制）

**参考实现**: [steipete/CodexBar](https://github.com/steipete/codexbar)（macOS 30+ provider 工具）

### 2.2 ChatGPT Plus / Pro / Team / Enterprise 订阅 — **D 级**

**结论**: **不做**。

**为什么不做的具体原因**:
1. **无官方 quota API**：OpenAI 长期未提供 ChatGPT 订阅的"剩余配额"端点
2. **OAuth 路径不通**：Codex CLI 的 OAuth flow 颁发的是给 `app_EMoamEEZ73f0CkXaXp7hrann` 的 token，目的是给 `codex responses` 推理用，**不暴露 quota**
3. **Cookie 爬风险高**：
   - `chatgpt.com/backend-api/usage/*` 端点存在但未文档化
   - 18 月 Pro 订阅者因自动化模式触发风控被 ban 的案例已公开记录（[community.openai.com/t/.../1381906](https://community.openai.com/t/codex-chatgpt-pro-account-banned-with-no-warning-no-explanation-18-month-subscriber/1381906)）
   - OpenAI ToS 明确禁止"Automatically or programmatically extract data" + "circumvent rate limits"
4. **多 CC 项目已踩坑**：cc-switch v3.13 release notes 明确写 *"Users enable these features at their own risk. CC Switch is not responsible for any account restrictions, warnings, or service suspensions"*，Musage 不应承担这个品牌风险

**兜底说明**: 用户如果非要看 ChatGPT 订阅额度，可以引导去 ChatGPT 网页 settings 页（不是 Musage 的责任）。

---

## 3. Anthropic Claude

### 3.1 路径选择（决策树）

```
用户是什么类型？
├─ Org / Team / Enterprise admin (有 sk-ant-admin-...) → 走 §3.2 Admin API
├─ Claude Pro / Max 个人订阅 + 装了 Claude Code CLI → 走 §3.3 OAuth /usage
└─ 都不想用 → 跳过；走 CustomSource 中转站方案（§5）
```

### 3.2 Claude Admin API（sk-ant-admin）— **A 级**

**端点**（[platform.claude.com/docs/en/manage-claude/usage-cost-api](https://platform.claude.com/docs/en/manage-claude/usage-cost-api)）:

| 端点 | 用途 |
|---|---|
| `GET /v1/organizations/usage_report/messages` | 按时间桶的 token usage（model / workspace / breakdown 维度）|
| `GET /v1/organizations/cost_report` | USD 花费 |
| `GET /v1/organizations/rate_limits` | org 配置的限额（programmatic 镜像 console Limits 页） |
| `GET /v1/organizations/usage_report/claude_code` | per-user Claude Code 活动 |

**鉴权**: Admin key（**仅 org 管理员能创建**）。`x-api-key: sk-ant-admin-...` + `anthropic-version: 2023-06-01`

**QuotaRow 设计**:
- 主行：今日 USD 花费
- 第二行：周 / 月累计
- `plan_name`: org 名

**已知坑**:
- 仅 org/team/enterprise 计划支持，**个人 Build 账号不能用**
- 不反映 claude.ai 订阅窗口（5h/7d 是订阅概念，不是 API 用量）

**适用人群**: 极小（多数个人开发者不是 org admin）。但**可作为 Phase 2 的"白名单"备选**。

### 3.3 Claude OAuth `/api/oauth/usage`（订阅 Pro/Max）— **C 级**

**端点**: `GET https://api.anthropic.com/api/oauth/usage`

**鉴权**:
```
Authorization: Bearer <oauth_access_token>   # 来自 Claude Code login
anthropic-beta: oauth-2025-04-20              # 必需
User-Agent: claude-code/<version>             # 必需，缺则持续 429
Content-Type: application/json
```

**响应 schema**（from [Maciek-roboblog/Claude-Code-Usage-Monitor](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor/issues/202)）:
```json
{
  "five_hour":        {"utilization": 23.5, "resets_at": "2026-06-16T18:00:00Z"},
  "seven_day":        {"utilization": 41.2, "resets_at": "2026-06-19T12:00:00Z"},
  "seven_day_opus":   {"utilization": 12.0, "resets_at": "..."},
  "seven_day_sonnet": {"utilization": 35.5, "resets_at": "..."},
  "seven_day_routines": {...},
  "seven_day_cowork":   {...},
  "extra_usage": {"current_spending": 1.23, "budget_limit": 50.0},
  "subscriptionType": "claude_pro",
  "rate_limit_tier": "default"
}
```

**OAuth token 获取**:
- macOS: Keychain entry `Claude Code-credentials`（`security find-generic-password -s 'Claude Code-credentials' -w`）
- 所有 OS: `~/.claude/.credentials.json`
- Token TTL: ~1h；refresh token ~数月
- 字段: `{ claudeAiOauth: { accessToken, refreshToken, expiresAt, scopes: ['user:profile','user:inference'] } }`

**QuotaRow 设计**（完美匹配 MiniMax 现有 5h/周模式）:
- 第一行：5h utilization + resets_at
- 第二行：7d utilization + resets_at
- plan_name: `subscriptionType`（"claude_pro" / "claude_max" / "claude_team"）

**已知坑**（**重点**）:
- **端点未文档化**：anthropic 不承诺稳定，可能 schema 变更
- **UA spoof 必修**：缺 `User-Agent: claude-code/...` 会被持续 429
- **OAuth TTL 短**：1h 后 token 过期，Musage 需要自动 refresh（OAuth refresh_token 在 ~/.claude/.credentials.json 里）
- **2026-04 政策收紧**：Anthropic 4 月起将第三方订阅 auth 流量归入 "extra usage" 桶（独立、更低），不是 plan 限额
- **anomaly #31021**：[anthropics/claude-code#31021](https://github.com/anthropics/claude-code/issues/31021) 报告 `/usage` HUD 在某些用户上整体 429'd
- **品牌风险**：至少 33 个开源项目在做同类监控（[GitHub topic: usage-tracker](https://github.com/topics/usage-tracker)），Anthropic 当前容忍，但舆论风向在收紧

**Musage 实施建议**（如果做）:
1. UI 加 disclaimer: "本数据源为非官方 OAuth API，Anthropic 可能随时变更或限制"
2. 缓存 TTL 至少 60-180s（避免触发 429）
3. token 自动 refresh：监听 `~/.claude/.credentials.json` mtime + 后台 task 调 OAuth refresh endpoint
4. 提供 fallback 链接：toS 变严时跳转到 claude.ai `/settings/usage` 网页

**这是 [extend-providers.md §5](../codeplan/2026-06-15-extend-providers.md) Phase 2 的内容**——plan 已认可。**评级从 B 降到 C**，建议 plan 同步更新。

### 3.4 Cookie sessionKey 爬虫 — **D 级**

**结论**: **不做**。

具体坑见调研：sessionKey HTTP-only + 1 月寿命 + Cloudflare `__cf_bm` 战 + 需 `lastActiveOrg` cookie + organization UUID。**比 OAuth 路径更脆**（甚至更明确违反 ToS）。Musage 不应提供这个选项。

---

## 4. Google Gemini

### 4.1 路径选择（决策树）

```
用户用什么 Gemini？
├─ AI Studio 免费 API key → 走 §4.3 静态限额展示（D，标 "usage: unknown"）
├─ Gemini Code Assist (Individual / Pro / Ultra / Workspace) → 走 §4.2 OAuth（A，必做）
├─ Vertex AI (GCP 项目) → 走 §4.4 Cloud Monitoring（C-，可选）
└─ 自建 Gemini 兼容中转 → 走 CustomSource（§5）
```

### 4.2 Gemini Code Assist OAuth（推荐）— **A 级**

**端点**: `POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`

**鉴权**: OAuth 2.0 with PKCE
- **复用 Gemini CLI 的公开 OAuth client**（不需要用户自建 GCP project）
- Scope: `cloud-platform`
- 颁发后存 refresh_token 到 Musage keys.json

**请求体**:
```json
{"project": "<gcp_project_id>"}
```

**响应 schema**（from [opencode-gemini-auth](https://github.com/jenslys/opencode-gemini-auth) + [geminicli.com/docs/resources/quota-and-pricing](https://geminicli.com/docs/resources/quota-and-pricing)）:
```json
{
  "quota": {
    "used": 234,
    "limit": 1000,
    "remaining": 766,
    "resetTime": "2026-06-17T00:00:00Z"
  },
  "tier": "ai_pro"   // individual / ai_pro / ai_ultra / code_assist_standard / code_assist_enterprise / workspace_ai_ultra
}
```

**QuotaRow 设计**（匹配 MiniMax 风格）:
- 第一行：今日 quota utilization% + reset countdown
- 第二行：周累计 / 月累计（如果 schema 提供）
- plan_name: `tier` → 映射成中文（"AI Pro" / "AI Ultra" 等）

**配额 tier 表**（from [geminicli.com](https://geminicli.com/docs/resources/quota-and-pricing)）:

| Tier | Daily Quota |
|---|---|
| Free Google account (Individual) | 1000 |
| AI Pro | 1500 |
| AI Ultra | 2000 |
| Code Assist Standard | 1500 |
| Code Assist Enterprise | 2000 |
| Workspace AI Ultra | 2000 |
| Free API key | 250 |
| Vertex | varies |

**OAuth flow 实现要点**:
1. 启动浏览器到 `https://accounts.google.com/o/oauth2/v2/auth?client_id=...&redirect_uri=http://localhost:8085/callback&scope=...&access_type=offline`
2. local loopback 监听 8085，捕获 `code`
3. POST token endpoint 换 `access_token` + `refresh_token`
4. 存到 `keys.json`
5. 后台 task 每 50 分钟自动 refresh

**已知坑**:
- `project` 字段不能省略；初次 OAuth 后需用户填或自动从 token claims 取
- 端点 `cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` 是 internal 命名空间，Google 没承诺 SLA
- 多个开源项目（[onWatch](https://github.com/onllm-dev/onwatch), [caut](https://github.com/Dicklesworthstone/coding_agent_usage_tracker), [opencode-gemini-auth](https://github.com/jenslys/opencode-gemini-auth)）已经在用此端点多年没出问题——这是关键 social proof

**参考实现优先级**:
- [onWatch](https://github.com/onllm-dev/onwatch) — 最完整，**GPL-3.0 不可直接抄代码，但可以读实现**
- [caut](https://github.com/Dicklesworthstone/coding_agent_usage_tracker) — Rust CLI，**MIT+Rider 友好**
- [opencode-gemini-auth](https://github.com/jenslys/opencode-gemini-auth) — MIT；最好 reference

### 4.3 AI Studio API key — **D 级**

**结论**: **不做事后监控，只展示静态限额**。

Gemini API key 没有 usage endpoint。只能从 429 错误体解析 `quota_limit_value`（每次响应才会出现）。Musage 顶多在用户填 API key 后展示"你的 key 关联项目的免费 tier 限额是 250 req/day"作为静态信息，**usage 部分就标 "unknown"**。

### 4.4 Vertex AI（Cloud Monitoring）— **C- 级**

**端点**: Cloud Monitoring API `metric.type = "serviceruntime.googleapis.com/quota/rate/net_usage" AND resource.labels.service = "aiplatform.googleapis.com"`

**鉴权**: Service Account JSON key 或 ADC

**为什么不推荐**:
- 项目级非用户级（用户只有一个项目但有 50 个 user，监控不到 per-user）
- 数据延迟 24h+
- Setup 摩擦巨大：用户需创建 GCP project → enable billing → create SA → 下载 JSON key
- BigQuery export 才能拿到 cost 数据，进一步增加 setup

**适用**: Musage 不应花时间做这个；如有用户要求，可以文档化一个 wiki 教程，但不在产品里 ship。

### 4.5 Workspace Reports — **C 级**

**结论**: 2-3 天延迟，admin-only，**完全不适合浮窗**。

---

## 5. xAI Grok

### 5.1 xAI Management API（推荐）— **B+ 级**

**端点**（[docs.x.ai/developers/rest-api-reference/management/billing](https://docs.x.ai/developers/rest-api-reference/management/billing)）:

| 端点 | 用途 |
|---|---|
| `GET /v1/billing/teams/{team_id}/prepaid/balance` | 当前 prepaid credit（USD cents） |
| `GET /v1/billing/teams/{team_id}/postpaid/invoice/preview` | 月初至今账单预览（vs 限额） |
| `GET /v1/billing/teams/{team_id}/postpaid/spending-limits` | 硬 / 软月度限额 |
| `POST /v1/billing/teams/{team_id}/usage` | 历史 usage time series（per model / per api-key） |

**鉴权**: Bearer `$XAI_API_KEY`（**与推理 key 同**——这点好！不需要单独管理 key）

**响应 schema 示例**:
```json
// GET /v1/billing/teams/{team_id}/prepaid/balance
{"changes": [...], "total": {"val": "-1000"}}  // val = USD cents
```
```json
// GET /v1/billing/teams/{team_id}/postpaid/invoice/preview
{
  "coreInvoice": {...},
  "effectiveSpendingLimit": "20000",   // USD cents = $200
  "defaultCredits": "0",
  "billingCycle": {"year": 2026, "month": 6}
}
```
```json
// POST /v1/billing/teams/{team_id}/usage
{
  "timeSeries": [{
    "group": ["Chat grok-4-0709"],
    "dataPoints": [{"timestamp": "2026-06-15T00:00:00Z", "values": [0.76]}]
  }]
}
```

**QuotaRow 设计**:
- 第一行：当前 prepaid credits（USD）
- 第二行：本月已花 / 软限额
- 第三行（可选）：per-model 本月 USD

**已知坑**:
- **`{team_id}` 必填** — 用户必须从 console URL 复制 UUID（`https://console.x.ai/team/<UUID>/...`）
- 没有 self-discovery endpoint 拿 team_id from API key
- **Musage UX 设计**：设置面板提供 "如何找 team_id" 帮助文字 + "自动发现（需先访问 console.x.ai 登录）"按钮

**实施策略**（推荐）：
1. v1: 用户手动 paste team_id（简单直接）
2. v2: 自动发现（引导用户去 console.x.ai 复制 team_id 字符串；解析 URL）
3. v3: 引导用户跑 `curl -H "Authorization: Bearer $XAI_API_KEY" https://api.x.ai/v1/auth/teams`（如果官方加这个 endpoint 的话）

### 5.2 X Premium / SuperGrok 订阅 — **D 级**

**结论**: **不做**。

xAI/X 没有公开 SuperGrok / Premium+ 用户的 quota API。已知 endpoint（`grok.com/rest/app-chat/conversations/new`）是 SSO cookie 鉴权，且**没有剩余 quota 字段**（streaming 响应里只有 token 计数）。第三方案例（[openclawdir xai-auth](https://openclawdir.com/plugins/xai-auth-ryhhje)）明确警告 *"Possible ToS issues — programmatic access via cookies may violate xAI's Terms of Service"*。

---

## 6. 中转站 / 代理生态（杠杆方案）

### 6.1 覆盖矩阵

**NewAPI 兼容国内中转（≥15 站）**：

| 中转站 | GPT | Claude | Gemini | Grok | NewAPI 兼容 | 公开 quota endpoint |
|---|---|---|---|---|---|---|
| DMXAPI | ✅ | ✅ | ✅ | ⚪ | ✅ | `/api/user/self` |
| API易 (apiyi) | ✅ | ✅ | ✅ | ✅ | ✅ | `/api/user/self` |
| Crazyrouter | ✅ | ✅ | ✅ | ✅ | ✅ | `/api/user/self` |
| PoloAPI | ✅ | ✅ | ✅ | ✅ | ✅ | `/api/user/self` |
| 147API | ✅ | ✅ | ✅ | ⚪ | ✅ | `/api/user/self` |
| 诗云 ShiyunApi | ✅ | ✅ | ✅ | ⚪ | ✅ | `/api/user/self` |
| PackyCode | ✅ | ✅ | ✅ | ⚪ | ✅ | `/api/user/self` |
| YesCode | ✅ | ✅ | ⚪ | ❌ | ✅ | `/api/user/self` |
| AICodeMirror / AIGoCode | ✅ | ✅ | ⚪ | ❌ | ✅ | `/api/user/self` |
| PatewayAI | ✅ | ✅ | ⚪ | ❌ | ✅ | `/api/user/self` |
| Cubence | ⚪ | ✅ | ⚪ | ❌ | ✅ | `/api/user/self` |
| DawCode / NekoCode / Ekan8 / 智惠API / aifast.club / token5u / jiekou.ai | ✅ | ✅ | ⚪ | ⚪ | ✅ | `/api/user/self` |

**海外聚合**：

| 平台 | 评级 | 备注 |
|---|---|---|
| OpenRouter | **A** | `/api/v1/key` + `/api/v1/credits`，Musage 已实现 |
| Requesty | **C** | 无公开 REST balance endpoint；管理后台 only |
| LiteLLM (自部署) | **A** | `/key/info` 端点简单清晰 |
| Portkey | **D** | 无 user balance；admin analytics only |
| Helicone | **D** | 纯 observability 平台，无余额 |

**官方 OAuth passthrough**：

| 平台 | 评级 | 备注 |
|---|---|---|
| OpenAI 官方 | **D** | 无 balance API；admin key 走 `/v1/organization/costs`（已 §2.1）|
| Anthropic 官方 | **C** | OAuth /usage 已 §3.3 |
| Google AI Studio | **D** | 无 usage endpoint（已 §4.3）|
| xAI 官方 | **B+** | Management API 已 §5.1 |

### 6.2 CustomSource 通用模板（NewAPI）

**endpoint**: `GET {base_url}/api/user/self`

**鉴权**（兼容 QuantumNous new-api / Calcium-Ion new-api）:
```
Authorization: Bearer <api_key>
New-Api-User: <user_id>      # new-api 扩展；one-api v1 不需要
User-Agent: Musage/1.0 (CustomSource)
```

**响应 schema**（one-api v1 兼容子集）:
```json
{
  "success": true,
  "message": "",
  "data": {
    "id": 123,
    "username": "alice",
    "display_name": "Alice",
    "role": 1,
    "status": 1,
    "quota": 50000,           // 1 USD = 500000 quota
    "used_quota": 5000,
    "group": "default",        // v1 早期可能缺失
    "request_count": 1234
  }
}
```

**Musage 默认模板设计**（已在 [extend-providers.md §6.4](../codeplan/2026-06-15-extend-providers.md) 规划）:
- 主行: `data.quota / 500000 - data.used_quota / 500000` USD 剩余
- 第二行: `data.used_quota / 500000` USD 已用
- plan_name: `data.group || data.display_name || data.username`
- unit: "USD"

**user_id 自动发现**:
- one-api v1: 没有自动发现端点（cookie 里拿）
- new-api v2: `GET /api/user/token` 列出用户的所有 token，含 user_id
- **建议**: Musage 设置面板让用户手动填一次（一次成本）

### 6.3 用户覆盖估算

参考同源工具 CC Switch（67K+ stars）的内置 50+ 预设 + claude-code-hub 赞助商列表 + 中文社区 2026 测评：

- NewAPI 兼容统一抓 → 覆盖 ≥80% 国内用户
- + OpenRouter / LiteLLM 海外 preset → 推到 90%+
- + 3 个官方 OAuth 路径（Claude / Gemini / OpenAI Admin）→ 推到 95%+

**剩余 5% 场景**: 自建中转 + 非 NewAPI 兼容 + 偏门 quota 需求 → 走 CustomSource 高级模式（用户手填 JSON path）

---

## 7. 集成路线图

### 7.1 工作量汇总（按 P0-P3）

| Phase | 内容 | 工作量 | 依赖 | 评级 |
|---|---|---|---|---|
| **P0** | CustomSource NewAPI 通用模板（已规划） | 2-3 天 | — | A |
| **P0** | OpenRouter preset 接入 CustomSource 模板系统 | 0.5 天 | CustomSource | A |
| **P1** | Gemini Code Assist OAuth source | 2 天 | — | A |
| **P1** | OpenAI Platform admin key source | 1 天 | — | A |
| **P2** | Claude Admin API source | 1 天 | — | A |
| **P2** | Claude OAuth /usage source | 1.5 天 | — | C |
| **P2** | xAI Grok Management API source | 1.5 天 | — | B+ |
| **P3** | LiteLLM `/key/info` preset | 0.5 天 | CustomSource | A |

**P0 = 3 天**, **P0+P1 = 6 天**, **P0+P1+P2 = 10 天**

### 7.2 PR 拆分建议

| PR | 内容 | 工作量 | 风险 |
|---|---|---|---|
| **PR-1** | P0: CustomSource + OpenRouter preset | 3 天 | 中（QuotaSource trait 改 Cow）|
| **PR-2** | P1: Gemini Code Assist OAuth source | 2 天 | 中（OAuth flow + loopback 监听）|
| **PR-3** | P1: OpenAI Platform admin source | 1 天 | 低（单 endpoint）|
| **PR-4** | P2: Claude OAuth + Claude Admin + xAI Grok | 4 天 | 中（3 个 OAuth / team_id 发现）|
| **PR-5** | P3: LiteLLM preset | 0.5 天 | 低 |

**总共**: 10.5 天，比 extend-providers.md 9-12 天 + 10-20% 增量，**目标 v0.3 / v0.4 release**。

### 7.3 每个 P1+ source 的统一 schema

| QuotaRow 字段 | OpenAI Platform | Claude Admin | Claude OAuth | Gemini Code Assist | xAI Grok |
|---|---|---|---|---|---|
| label | "今日" / "本周" / "本月" | 同左 | "5h" / "7d" / "7d Opus" / "7d Sonnet" | "今日" | "Prepaid" / "MTD" |
| utilization | n/a（无 quota）| n/a | ✅ % | ✅ % | n/a（无 quota）|
| remaining | n/a | n/a | n/a | ✅ | ✅ USD |
| used | ✅ USD | ✅ USD | n/a | ✅ req | ✅ USD |
| total | n/a | n/a | n/a | ✅ req/day | ✅ USD cap |
| unit | "USD" | "USD" | "%" | "req" / "%" | "USD" |
| resets_at | n/a | n/a | ✅ | ✅ | 月底 |
| plan_name | org 名 | org 名 | subscriptionType | tier | n/a |

---

## 8. 风险与回退

| 风险 | 概率 | 影响 | 回退策略 |
|---|---|---|---|
| Anthropic 进一步收紧 OAuth 第三方使用（彻底 ban）| 中 | Claude OAuth source 失效 | 提示用户切到 Admin API；降级显示静态 info |
| Google 修改 `cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` schema | 低 | Gemini source 部分字段失效 | schema_unknown error；用户切到 Vertex AI / CustomSource 中转 |
| xAI 仍不提供 team_id self-discovery | 高 | UX 摩擦 | 提供"如何找 team_id"图文教程 |
| NewAPI 某中转站 schema 变更 | 中 | 那个特定用户的 CustomSource 失效 | 引导用户报 issue；extractor 容错 |
| 国内中转站跑路 | 高 | 用户数据丢失 | UI 标"中转站可能跑路"；不强推单一站 |
| OpenAI 进一步禁用 `/v1/organization/costs` | 极低 | OpenAI Platform source 失效 | 极不可能（这是 Admin API 的核心） |
| quota 单位混淆（500000 quota = 1 USD）| 中 | 数值显示错 500000x | UI 明确显示 USD；preset 强制 unit 字段 |

---

## 9. 关键决策点（待维护者拍板）

> **决策主轴：用户 friction**。F1 > F1.5 > F2 > F3+。D 不做。

### 9.1 已确定

1. ✅ **CustomSource NewAPI 通用模板**（P0）— F1 friction，吃 15+ 中转站
2. ✅ **OpenRouter preset**（P0）— F1 friction，Musage 已有 source
3. ✅ **砍掉 D 级的所有方案** — ChatGPT 订阅、X Premium、AI Studio、sessionKey cookie

### 9.2 待拍板

| # | 问题 | 推荐 | 备选 |
|---|---|---|---|
| 1 | **Claude OAuth /usage 做不做**（P1 F1.5 C 评级）| ✅ **做**——friction 太香（浏览器一键登录）；ToS 风险靠 disclaimer + 60-180s 缓存 + auto refresh 缓解；33+ 同类工具在做，Anthropic 当前容忍 | 推迟 v0.5；只做 Admin API（F3） |
| 2 | **Gemini Code Assist OAuth**（P1 F1.5 A 评级）| ✅ **强烈推荐做**——隐藏金矿；onWatch/caut 三个项目已稳定使用多年；复用 Gemini CLI 公开 OAuth client（用户零 setup） | 自建 GCP OAuth client（F4，不要选）|
| 3 | **OpenAI Platform admin key**（P2 F3 A 评级）| ✅ 做——admin key 是 OpenAI 官方为 quota 监控设计的；CodexBar 已用 | 只做 ChatGPT 订阅（D，不要选）|
| 4 | **xAI Grok Management API**（P2 F3 B+ 评级）| ⚠️ **看用户反馈**——friction 高（要 team_id）；用户群小（多数 Musage 用户不用 xAI）| 暂不做；只通过 CustomSource 间接覆盖（如果中转站转售 Grok）|
| 5 | **Claude Admin API**（P3 F3 A 评级）| ⚠️ 暂不做——仅 org admin 能建 key，用户群极小；Claude OAuth 已覆盖大多数 | 如果有企业用户反馈再做 |
| 6 | **LiteLLM Proxy preset**（P3 F1 A 评级）| ✅ 做——成本极低（0.5 天），schema 简单，扩海外覆盖 | — |
| 7 | **OAuth UX 模式抽象**：Gemini 和 Claude OAuth 都是 F1.5，要复用"浏览器一键登录" UX 组件吗？| ✅ **强烈推荐抽象**——避免每个 OAuth source 重复实现 loopback 监听 + token refresh；建议扩展 AuthKind enum 加 `OAuth` 变体 | 每个 source 自己实现（重复）|
| 8 | **AuthKind enum 是否扩展？** 当前只有 `ApiKey` / `Cookie` / `ApiKeyOrCookie`，OAuth 路径要不要加 `OAuth`？| ✅ **加**——和 Xiaomi cookie 流程一样，store refresh_token 在 `Credentials.api_key` 字段里 + 后台 task 自动 refresh | 复用 ApiKey 字段 + 加 magic prefix 区分（hacky）|

### 9.3 关键 UX 决策（需实现前敲定）

- **OAuth 登录 UI**：设置面板里点 "Connect Claude" / "Connect Gemini Code Assist" 按钮 → 弹 OAuth 弹窗 → 浏览器自动打开 → 用户登录 → 跳回 localhost:8085/callback → 自动存 token → 按钮变 "✓ Connected"
- **token refresh 机制**：所有 OAuth source 后台 tokio task，每 50 分钟检查 token expiry → 自动调 refresh endpoint
- **失败兜底**：token 失效 / refresh 失败 → 卡片显示 "Reconnect" 按钮 → 用户点一下重新走 OAuth
- **disclaimer UI**：C 级 source（Claude OAuth）卡片顶部加 ⚠️ "Non-official API, may break at any time" + 链接到官方页面
- **管理员文档**：F3 source（OpenAI / xAI Grok / Claude Admin）在设置面板加 "📖 如何获取 xxx key" 折叠帮助区

---

## 10. 关键引用（验证过的真实 URL）

### 10.1 OpenAI
- [OpenAI Usage API](https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs)
- [CodexBar OpenAI docs](https://github.com/steipete/CodexBar/blob/main/docs/openai.md)
- [CodexBar issue #877](https://github.com/steipete/CodexBar/issues/877) — `amount.value` is sometimes string
- [OpenAI community: 5+ year old feature request for balance API](https://community.openai.com/t/why-is-there-no-api-for-account-balance/937)

### 10.2 Anthropic Claude
- [Anthropic Admin API](https://platform.claude.com/docs/en/manage-claude/usage-cost-api)
- [Maciek-roboblog/Claude-Code-Usage-Monitor#202](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor/issues/202) — OAuth usage endpoint
- [anthropics/claude-code#31021](https://github.com/anthropics/claude-code/issues/31021) — 429 rate limit
- [Anthropic April 2026 policy](https://www.reddit.com/r/Anthropic/comments/1scrxy2/normal_users_now_get_penalized_too_for_using) — third-party OAuth crackdown
- [cc-switch v3.13 release notes](https://github.com/farion1231/cc-switch/blob/main/docs/release-notes/v3.13.0-en.md)

### 10.3 Google Gemini
- [Gemini CLI quota docs](https://geminicli.com/docs/resources/quota-and-pricing)
- [opencode-gemini-auth](https://github.com/jenslys/opencode-gemini-auth) — endpoint documentation
- [onWatch](https://github.com/onllm-dev/onwatch) — full implementation (GPL-3.0)
- [caut](https://github.com/Dicklesworthstone/coding_agent_usage_tracker) — Rust CLI (MIT+Rider)

### 10.4 xAI Grok
- [xAI Billing API](https://docs.x.ai/developers/rest-api-reference/management/billing)
- [xAI FAQ billing](https://docs.x.ai/docs/resources/faq-api/billing)
- [xAI Enterprise ToS](https://x.ai/legal/terms-of-service-enterprise)
- [Grok rate limits (apidog)](https://grok-api.apidog.io/consumption-and-rate-limits-934014m0)
- [openclaw xai-auth](https://openclawdir.com/plugins/xai-auth-ryhhje) — cookie path warning

### 10.5 Middleware / Router
- [QuantumNous/new-api](https://github.com/QuantumNous/new-api)
- [songquanpeng/one-api](https://github.com/songquanpeng/one-api/blob/main/controller/user.go)
- [MartialBE/one-api-upstream](https://github.com/MartialBE/one-api-upstream)
- [cc-switch NewAPI preset](https://www.newapi.ai/en/docs/apps/cc-switch)
- [CC Switch v3.16 release](https://github.com/farion1231/cc-switch/blob/main/docs/release-notes/v3.16.0-en.md)
- [CodexBar OpenRouter docs](https://github.com/steipete/CodexBar/blob/main/docs/openrouter.md)

---

## 11. 附录: Musage 现有架构兼容性检查

| 现状 | 适配性 | 说明 |
|---|---|---|
| `QuotaSource` trait（Send + Sync）| ✅ | 所有新 source 都 implement 这个 trait |
| `AuthKind` enum (ApiKey / Cookie / ApiKeyOrCookie) | ⚠️ **需扩展** | OAuth source（F1.5 friction）需要新 `OAuth` 变体 + 浏览器 loopback 登录 UX；详见 §11.1 |
| `Credentials { api_key, cookie }` | ✅ | OAuth `refresh_token` 存 `api_key` 字段（约定 `oauth:gemini:` / `oauth:claude:` prefix 区分）；cookie 路径（D 级，不用） |
| `Provider` enum | ⚠️ | Phase 1 后已**不强制新 source 扩 enum**；现有 source_id 字符串路径够用 |
| `QuotaRow` 字段（utilization/remaining/used/total/resets_at/unit/extra）| ✅ | 所有字段都覆盖 |
| `QuotaSource::id() -> &'static str` | ⚠️ | extend-providers §6.3 计划改 `Cow<'_, str>`，**CustomSource 必走 Cow** |
| `builtin_sources()` 注册表 | ✅ | 加新 source 只在这里 Box::new() |
| CustomSource（已规划）| ✅ | NewAPI 通用 + OpenRouter + LiteLLM 都走这个 |
| 设置面板分组（已规划）| ✅ | extend-providers §8.3 加分组定义；新 source 加进对应 group |
| `ErrorKind` enum | ✅ | OAuth 失败 → AuthFailed；rate limit → RateLimited；schema 变 → SchemaUnknown |
| **OAuth UX 组件**（新）| ❌ **需新建** | 浏览器 loopback 监听 + token exchange + 后台 refresh task；详见 §11.2 |

### 11.1 需扩展：AuthKind 加 OAuth 变体

**当前**:
```rust
pub enum AuthKind {
    ApiKey,           // Bearer / 空前缀
    Cookie,           // Cookie: xxx
    ApiKeyOrCookie,   // Xiaomi: 401 fallback
}
```

**建议扩展**（F1.5 friction 用）:
```rust
pub enum AuthKind {
    ApiKey,
    Cookie,
    ApiKeyOrCookie,
    OAuth,            // 新：浏览器一键登录 + 后台自动 refresh
}
```

**UX 流程**:
1. 设置面板新 OAuth source 卡片显示 "Connect {Provider}" 按钮（不带 key 输入框）
2. 用户点按钮 → Musage 打开系统浏览器到 OAuth authorization URL
3. 用户在浏览器登录 + 授权
4. OAuth server redirect 到 `http://localhost:8085/callback?code=...`
5. Musage 后台 tokio task 监听 8085，捕获 code → POST token endpoint 换 `access_token` + `refresh_token`
6. 存到 `keys.json`，prefix `oauth:gemini:` 或 `oauth:claude:`（标识这是 OAuth 凭据）
7. 卡片状态变 "✓ Connected · {user_email}"
8. 后台 task 每 50 分钟检查 expiry → 自动 refresh

**Credentials 复用**: refresh_token 存 `Credentials.api_key` 字段；access_token 内存缓存（不持久化）；loopback port 用 `OnceLock<u16>` 存首次分配

### 11.2 需新建：OAuth UX 组件

| 组件 | 位置 | 职责 |
|---|---|---|
| `oauth_server.rs` | `src-tauri/src/` | 本地 HTTP server（hyper / axum mini）；监听 loopback port；处理 `/callback`；触发 token exchange |
| `oauth_refresh.rs` | `src-tauri/src/` | 后台 tokio task；每 50 分钟检查所有 OAuth source 的 token expiry；调 refresh endpoint |
| 设置面板 OAuth 卡片 | `settings.html` | 新增组件 "Connect 按钮 + Connected 状态"；点击触发 system browser 打开 OAuth URL |
| IPC 命令 | `commands.rs` | `start_oauth_flow(provider_id) → 启动浏览器 + 返回 success/fail` / `disconnect_oauth(provider_id) → 清 tokens` |

**复用性**: Gemini Code Assist OAuth + Claude OAuth /usage 都能用这一套。只需 source 各自实现：
- `authorization_url()` → 拼 OAuth URL（含 client_id / scope / redirect_uri）
- `exchange_code(code) → (access_token, refresh_token, expires_in)`
- `refresh(refresh_token) → (access_token, expires_in)`

**参考实现**:
- [onllm-dev/onwatch](https://github.com/onllm-dev/onwatch) — Gemini OAuth 完整 Rust 实现
- [jenslys/opencode-gemini-auth](https://github.com/jenslys/opencode-gemini-auth) — TypeScript OAuth flow

### 11.3 结论

| 改造 | 工作量 | 是否阻塞 P1 |
|---|---|---|
| AuthKind 加 OAuth 变体 | 0.5 天 | ✅ 阻塞 |
| Credentials 加 prefix 约定 | 0（约定不动结构）| — |
| OAuth UX 组件（oauth_server + oauth_refresh + 设置面板卡片）| 1.5 天 | ✅ 阻塞 |
| QuotaSource trait 改 Cow（extend-providers 已规划）| 0.5 天 | 否（CustomSource 必走 Cow；OAuth source 可以先用静态 `id()`）|

**P1 阻塞项总计 2 天**。P1 本身（Gemini + Claude OAuth source 实现）= 3.5 天。**P1 总计 5.5 天**。

P0（CustomSource + OpenRouter preset）+ P1（OAuth 框架 + 2 个 OAuth source）= 3 + 5.5 = **8.5 天**，覆盖 90%+ 用户闭源模型场景。

---

> **下一步**: 等维护者对 §9 决策点拍板，然后本文档会变成具体 codeplan（类似 extend-providers.md）的输入。
