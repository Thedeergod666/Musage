# Musage 未来想法

> 暂不做 / 等需求 / 等灵感。这里是**停车场，不是承诺**。
> 排序按"何时可能捡起来"从近到远。

## 已砍 / 暂缓

### Doubao（字节火山方舟）— 暂缓
- **原因**：IAM 签名是 AWS-style V4 变体（`X-Date` + 多个 derived signing key + canonical request），光 RFC + 单测 1 天，还没算调通
- **ROI 低**：CustomSource 已经吃掉 10+ 个中转站，Doubao 杠杆不高
- **未来捡起来的条件**：用户真实使用需求出现
- **估时重做**：1-2 周（单开一个 Phase）

### JS extractor (boa_engine / rquickjs) — 已砍
- **原因**：启动 +10MB，用户写 JS extractor 失败率高
- **替代方案**：CustomSource JSONPath 模板，2 个预设（New API 系 + 余额系）覆盖 90% 场景
- **未来捡起来的条件**：JSONPath 模板确实不够用（目前没看到信号）

### 用量历史 / 趋势图 / 成本预测 — 不做
- **原因**：Musage 是"看一眼"工具，不是 BI。**这个定位不要漂移**。
- **替代方案**：导出原始 JSON，让用户自己用 Excel / Notion 处理

### 配额预警通知 — 暂缓
- **原因**：先做准确展示（当前还在修浮窗刷新逻辑），通知是后置
- **未来**：system notification（Windows toast / macOS NSUserNotification），不开源第三方推送

### 支付集成 / 自动充值 — 不做
- **理由**：Musage 只监控，**不操作**。这条是产品边界。

### 云同步 key — 永远不做
- **理由**：你信任本地文件胜过信任我们，**也对我们好**（零服务器成本、零法律责任）

### 账号体系 — 永远不做
- **理由**：Musage 永远不需要登录。无账号 = 无 GDPR / 无密码泄露 / 无需 server。

## 想到但没排期

### webview 自动登录（Xiaomi cookie 痛点）
- 现状：用户手动粘 Cookie（虽然 2026-06-14 加了"一键登录"WebView 抓取）
- 痛点：Token 8h 过期，重登频繁
- 方案：定时检测 cookie 失效 → 弹通知 → 引导重登
- 估时：1-2 周

### 浮窗"按主屏取色"自定义
- 现状：固定绿/橙/红阈值
- 用户呼声：想让卡片颜色跟主屏壁纸配
- 估时：1-2 天

### 多语言（en-US / zh-CN / ja-JP）
- 现状：UI 文字中文硬编码
- 估时：1 周（含 i18n 框架选型 + 字符串抽取）

### 移动端镜像
- 现状：Tauri 桌面 only
- 方案：Tauri Mobile / Flutter / 单独 RN
- 估时：未知

### 多 provider 并发 fetch 限速
- 现状：并发抓 8 个 provider，无 throttle
- 风险：触发反爬（特别是 Claude / OpenRouter）
- 方案：每 provider 独立 rate limit
- 估时：2-3 天

### 卡片位置记忆
- 现状：每次启动位置是固定默认
- 痛点：用户拖到左下角，重启后又回右上
- 方案：保存到 `config.json`，启动恢复
- 估时：0.5 天

### 卡片位置跨屏感知
- 现状：多屏用户拖到副屏，重启后位置"丢了"（因为是按主屏坐标存的）
- 方案：按"相对主屏"存储
- 估时：1 天

## 可能会做但还没想清楚

### Provider 官方 SDK 集成
- 一些 provider 提供官方 SDK（如 OpenAI Python SDK）
- 走 SDK 比手写 reqwest 简单，但增加二进制大小
- 决策点：要不要走 SDK

### 设置面板 import/export
- 用户多设备 / 重装系统时方便迁移
- 风险：key 导出 → 误传到 GitHub
- 决策点：要不要做 / 怎么做

### 提供"团队"模式
- 一份配置 → 多台机器同步
- 跟"云同步 key" 边界模糊，需要想清楚

## 绝对不会做

- 云同步 key
- 账号体系
- 内置"AI 助手"聊天窗口（Musage 是工具不是 chatbot）
- 浏览器扩展（保持桌面单一形态）
