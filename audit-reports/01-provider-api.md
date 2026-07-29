# Provider API 实现审查报告

## 概览
- 审查文件:16 个 `.rs` 文件,约 13,000 行。其中 14 个 provider 实现,加 `mod.rs` / `parse.rs` 两个公共层文件
- 总 bug 数:6(CRITICAL 0 / HIGH 3 / MEDIUM 3 / LOW 0)
- 整体健康度评分:7.5 / 10
- 主要风险集中在 RefreshToken 单次轮换竞态、MiniMax 状态门控、401 分类,以及响应体上限未全面落地
- 未发现无限续期循环:StepFun 每次 `do_fetch` 最多主动续期一次,再在鉴权失败后兜底续期一次,第二次 `fetch_once` 不会再次进入续期分支

## CRITICAL 级别(阻塞 / 数据丢失 / 安全)

未发现符合 CRITICAL 标准的问题。

## HIGH 级别

### [BUG-001] StepFun / AnySearch 单次轮换 RefreshToken 缺少并发串行化
- **位置**:`src-tauri/src/providers/stepfun.rs:252`、`src-tauri/src/providers/anysearch.rs:323`
- **关联位置**:`src-tauri/src/commands/mod.rs:1575`、`src-tauri/src/commands/extra_instances.rs:636`
- **类型**:并发竞态 / 凭据状态损坏
- **描述**:两个 provider 都明确把 refresh token 当作单次轮换凭据,但续期流程没有按 `unique_id` 加异步锁,也没有在续期前重新读取最新凭据。全量刷新虽然有全局防重,但 `refresh_single_inner`、保存配置后触发的后台刷新、额外实例验证等入口仍可独立调用 `fetch`。两个任务若同时读到同一旧 pair,都会进入主动续期。
- **具体触发路径**:access token 进入 `SKEW_SECS` 窗口;poller 开始刷新;用户同时执行单 provider 刷新、重新保存配置或实例验证;两个任务同时使用旧 refresh token;第一个请求轮换成功并写回新 pair;第二个请求使用已作废的旧 refresh token,返回 AuthFailed;若服务端对并发轮换不是严格单赢家,也可能后完成的旧分支覆盖先写入的新 pair。
- **影响**:用户会随机看到"凭据失效、请重新登录";更严重时 keys.json 最终保存的 pair 不再是服务端当前有效 pair,下一轮刷新必然失败。StepFun 和 AnySearch 都受影响,多实例只按不同 `unique_id` 隔离,不能解决同实例并发。
- **建议修法**:建立按 `unique_id` 分片的 `tokio::sync::Mutex`。拿锁后重新从 keys.json 加载凭据并比较 refresh half;若其他任务已经完成续期,直接使用新 pair。网络续期和写回必须处于同一个实例级临界区内。

### [BUG-002] MiniMax 将 `status=2/3 + remaining_percent=0` 错当成额度耗尽
- **位置**:`src-tauri/src/providers/minimax.rs:545`
- **类型**:Schema 语义错误 / 套餐状态误判
- **描述**:项目记录的 MiniMax 新 schema 明确规定 `current_*_status == 1` 才表示该 tier 有效,`2/3` 表示不在套餐内。当前实现却对 `status != 1` 设置了一个 `remaining_percent == 0` 例外,将其解析为 `utilization=100%`。
- **具体触发路径**:响应包含 `{current_interval_status: 2, current_interval_remaining_percent: 0, end_time: 0}` 时,代码不会返回 `None`,而是创建一条 100% 用量行。相同逻辑也应用于 weekly tier。
- **影响**:不包含 5h 或周额度的套餐会显示成"额度已全部用完",可能触发红色告警、托盘 100% 图标及错误的用户判断。它也绕过了原本"新 schema 失败后回退旧 schema"的设计。
- **建议修法**:严格以 `status == 1` 作为 percent schema 的有效门;只有确认 MiniMax 官方在额度耗尽时确实把 status 改成 2/3,才能增加经过实测 schema 约束的例外,不能仅根据 percent 为 0 推断。

### [BUG-003] Xiaomi 将部分真实 401 错误分类成 ServerError
- **位置**:`src-tauri/src/providers/xiaomi.rs:500`
- **类型**:鉴权边界 / 错误分类
- **描述**:HTTP 401 的分类依赖响应正文是否包含三个英文关键词(login/session/token)。真实鉴权失败响应完全可能为空、为中文、只有业务码,或只有 `"Unauthorized"`。这些响应全部被归为 `ServerError`。
- **具体触发路径**:用户 Cookie 过期,服务端返回 `401` 且 body 为空,或返回 `{"code":401,"message":"凭证已失效"}`。`looks_like_auth` 为 false,代码返回 ServerError,而不是 AuthFailed。
- **影响**:前端不会展示重新登录入口,因为 `ErrorKind::needs_settings()` 只对 `UnconfiguredKey` / `AuthFailed` 返回 true。用户只看到服务端错误,无法获知 Cookie 已过期;poller 也按错误的类别记录状态。
- **建议修法**:401 默认归为 AuthFailed。若确实要识别 CDN 伪 401,应只对经过验证的特定 CDN 响应特征降级为 ServerError。

## MEDIUM 级别

### [BUG-004] 8 MiB 响应体上限未覆盖多数 provider 和 CustomSource
- **位置**:`src-tauri/src/providers/custom.rs:290`
- **关联位置**:`src-tauri/src/providers/mod.rs:912`、`anysearch.rs:415`、`xiaomi.rs:540`、`zenmux.rs:277`
- **类型**:资源耗尽 / HTTP 响应处理不一致
- **描述**:公共层已经实现流式 8 MiB 限制,但大量 provider 仍直接调用 `resp.json()` / `resp.text()`。至少 CustomSource、AnySearch、Xiaomi、ZenMux、OpenRouter、Claude Official、Kimi、Tavily、SiliconFlow、Zhipu 和 DeepSeek 存在未受限读取。特别是 CustomSource 的响应端点完全由用户配置,是最需要限制的路径。
- **影响**:桌面常驻进程可能出现明显内存峰值、被系统杀死或 UI 卡死。10 秒总 timeout 不能等价替代大小限制。
- **建议修法**:所有响应读取统一走 `json_body_limited` / `text_body_limited`。错误响应也必须限流。CustomSource 应优先修复。

### [BUG-005] 公共限流读取器把传输中断错误分类成 JSON Parse
- **位置**:`src-tauri/src/providers/mod.rs:927`
- **类型**:错误分类 / 网络错误传播
- **描述**:`resp.chunk().await` 失败属于响应传输阶段错误(连接 reset、HTTP/2 stream reset、body timeout、代理提前断开),并不是 JSON 语法错误。当前 helper 无条件创建 `FetchError::parse`,且文案也是"解析 JSON 失败"。`text_body_limited` 同样复用该错误。
- **影响**:用户收到误导性的"JSON 格式错误",而不是网络错误;前端样式、日志去重和故障诊断都会错误。
- **建议修法**:chunk 读取错误应返回 `FetchError::network`。只有完整 body 成功读取后 `serde_json::from_slice` 失败,才归 Parse。超出体积上限可保留 Parse 或改为独立 ServerError/Other。

### [BUG-006] AnySearch refresh 在检查 HTTP 状态前解析 JSON,HTML 401 被误报为 Parse
- **位置**:`src-tauri/src/providers/anysearch.rs:252`
- **类型**:RefreshToken 错误传播 / 鉴权错误误分类
- **描述**:refresh endpoint 的 HTTP 状态只有在 body 成功解析成 JSON 后才检查。如果失效 refresh token 经 CDN/WAF/反向代理返回 HTML/空 body 的 401/403,函数会提前返回 Parse,永远到不了 AuthFailed 分支。
- **建议修法**:先保存 status,再用受限文本读取 body。401/403 无条件映射 AuthFailed;429 映射 RateLimited;其他非成功状态映射 ServerError。仅成功响应再解析 JSON。业务码应同时容忍数字和数字字符串。

## LOW 级别

未发现值得单独报告的 LOW 问题。

## 未发现问题的亮点
- `shared_client()` 正确使用进程级 `OnceLock<reqwest::Client>`,配置了总 timeout、connect timeout、连接池 idle 上限和稳定 User-Agent,连接复用设计合理。
- StepFun 续期没有死循环:主动续期后最多再执行一次 401 兜底续期,重试调用的是 `fetch_once` 而不是递归调用 `do_fetch`。
- StepFun 正确在主动续期后更新本地 token,并在 AnySearch 中同步更新新的 refresh half,避免继续使用已轮换的旧 refresh token。
- StepFun 双 schema 成功判定覆盖现行 `status == 1` 和旧版 `code == 0`;credit 时间支持 epoch 秒、毫秒、字符串和 ISO 8601,且区分"重置"和"到期"。
- Provider 多实例实现整体一致:内置 provider 均包含 `instance_index`、`with_instance_index` 和带 `#N` 后缀的 `unique_id()`。
- MiniMax 的 count / percent 双路径、`general` 模型优先选择及 duration-seconds / epoch-ms 智能时间转换覆盖了主要历史 schema。
- `num_f64` 已过滤 `NaN` / `inf`,避免非有限数字进入前端并被序列化成 `null`。
