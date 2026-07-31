# Musage 全量代码审查 8/8 报告 — commands / config / poller / build / i18n

**审查基线**：`HEAD = 9cdbb1c`（v0.2.5 候选）+ 2026-07-30 当天 uncommitted
**范围**：5 个域 — `commands/` IPC handler / `config.rs`（post D4-001~D4-009）/ `poller.rs` + `poller_backoff.rs`（post D5-007/038/074/101/102）/ 构建/发布/签名 / i18n locales
**复用排除**：D4-001~D4-009 + D5-007/D5-038/D5-074/D5-101/D5-102 + 已 commit 的其它 M/L fix 不重复
**真实 bug 数**：1 P0 / 1 P1 / 3 P2 / 3 P3 = 8 条
**说明**：本轮未触碰任何代码

---

## 概览

| 域 | 改动体量 | 真 bug 数 | 备注 |
|---|---|---|---|
| commands IPC | 2391 行 | 3 | i18n missing key 4 处 + quit 1 处 + Mutex 1 处（多数 D4-020 风格） |
| config.rs | 1456 行 | 0 新 | D4-001~009 已封；本轮无新发现 |
| poller / backoff | 851 行 | 0 新 | D5-007/038/074/101/102 已封；本轮只确认无新分支 |
| 构建 / 发布 / 签名 | 6 文件 | 2 | release.yml `bundles: nsis,msi` 复辟 v0.2.0 已修 bug；CI 走 MSVC 本地走 GNU |
| i18n locales | 2+1 文件 | 4 真实 | 4 个 production t() 调用的 key 在 en/zh 都缺，raw key 字符串直显 |

**最大风险**：i18n missing key（用户看得见）+ release.yml MSI 复辟（每次 Windows release CI 必撞 WiX timeout）两条都是 release blocker 级别。

---

## 🔴 P0（必须先修）

### D8-001 4 个 `t()` 调用在 en.json / zh-CN.json 都不存在对应 key，用户看到 raw key 字符串
- **置信度**：高（已确认）
- **位置**：
  - `src/settings/providers.ts:309` — `t("settings.providers.invalid_interval", { val: raw })`
  - `src/settings/region-wizard.ts:149` — `t("settings.region.auto_applied")`
  - `src/settings/extra-instance-form.ts:522,528,533` — `t("custom_source.err.field_too_long", { field: "name" | "base_url" | "path" })`
  - `src/settings/extra-instance-form.ts:557` — `t("custom_source.err.divide_invalid", { val: divideRaw })`
- **触发条件**：
  1. 用户在 settings 面板「轮询间隔」输入框输入非数字 / 负数 / > 86400 → 触发 `providers.ts:309` 红条提示
  2. 首次启动且用户 locale 非中文 + `cfg.user_region == "cn"` 自动切 global → 触发 `region-wizard.ts:149` 成功提示
  3. 用户在 New API 中转站表单「display name / baseUrl / path」任一字段超长 → 触发 3 个 `field_too_long` 错误
  4. 用户在 New API 中转站表单「divide」字段填 `-1` / `Infinity` / `1e7` / 非数字 → 触发 `divide_invalid` 错误
- **根因**：
  - `src/i18n/index.ts:80-88` 的 `t()` helper 在 key 找不到时**只 fallback 到 en dict**，en 也缺则 `return key` 本身（不抛错、不打日志给用户、只在 dev 模式 console.warn）
  - 4 个 key 是 v0.2.0 改配置面板时新加的，开发者加了 `t()` 调用但忘了在 `src/i18n/{en,zh-CN}.json` 里加对应 entry
  - 同样的 bug 模式在 `src-tauri/locales/` 后端 locale **没有** — 后端 171 个 t!() key 100% 在 en.json 里都有定义（已 diff 过）
  - 前端 i18n 总 key 数 = 477，代码用 = 432，缺 4 个，剩余 41 个「已定义未用」是良性的（备用 key）
- **用户影响**：
  - 4 条路径用户都直接看到 `settings.providers.invalid_interval` / `settings.region.auto_applied` / `custom_source.err.field_too_long` / `custom_source.err.divide_invalid` 等 raw key 字符串
  - 占位符 `{val}` / `{field}` 不替换 — 提示完全不达意
  - en locale 用户额外损失：fallback chain 找 en 也没找到，但**还是返 raw key**（`index.ts:99`），所以 dev 跟 prod 行为一致都不会"碰巧显示成英文"
  - 整类 bug 已在 `src/settings/source-extras.ts:238` 注释里被前人注意（"M9 fix: 之前用 t('common.punctuation_period') 但该 key 在 en.json/zh-CN.json 里只存在于 settings.common.punctuation_period, 找不到的 key 走 fallback 会原样回退成 raw key 字符串"），但补丁只修了 M9 一处，没顺手修剩下 4 个
- **证据链**：
  - `src/i18n/index.ts:73-78` `lookup(effectiveKey) == null` → `lookupInDict(dicts.en, key) == null` → `return key`
  - `src/i18n/en.json` grep `settings.providers.invalid_interval` → 无
  - `src/i18n/en.json` grep `settings.region.auto_applied` → 无
  - `src/i18n/en.json` grep `custom_source.err.field_too_long` → 无
  - `src/i18n/en.json` grep `custom_source.err.divide_invalid` → 无
  - `src/i18n/zh-CN.json` 同上 4 个 grep 全部无
  - 用法已确认是真实用户路径：`providers.ts:309` 在 `input.addEventListener("change")` 回调；`region-wizard.ts:149` 在首次启动 setRegion 完成后；`extra-instance-form.ts:5xx` 在 save custom source 按钮点击
- **最小修复**：在 `src/i18n/en.json` 和 `src/i18n/zh-CN.json` 各加 4 个 entry。给 4 个 key 的建议 value：
  - `settings.providers.invalid_interval`: en=`"Interval must be 10–86400 seconds (got {val})"` / zh=`"轮询间隔须在 10–86400 秒之间（当前 {val}）"`
  - `settings.region.auto_applied`: en=`"Default region auto-switched to Global (your system language is non-Chinese)"` / zh=`"已自动切换默认区域为 Global（系统语言非中文）"`
  - `custom_source.err.field_too_long`: en=`"Field {field} is too long (max 256 chars)"` / zh=`"字段 {field} 过长（上限 256 字符）"`
  - `custom_source.err.divide_invalid`: en=`"Divide must be a number between 1 and 1,000,000 (got {val})"` / zh=`"divide 必须是 1~1,000,000 之间的数字（当前 {val}）"`
  - **bonus 修法**（更稳）：在 `src/i18n/index.ts:95` 的 `return key;` 改成 `return key + " (missing translation)"`，至少让用户在 dev 模式之外也能发现"这是 untranslated key"而不是误以为「设置里多了个奇怪名字」
- **待实测**：加完 4 个 key 后跑 `pnpm test` 验证 `dumpMissingKeys()` 返空数组；用 `pnpm dev` 起服务后手动触发 4 个错误路径确认 flash 提示文案正确

---

## 🟠 P1（高）

### D8-002 release.yml Windows matrix 强制走 `bundles: nsis,msi`，复辟 v0.2.0 已修的 WiX timeout bug
- **置信度**：高
- **位置**：
  - `.github/workflows/release.yml:69` — `bundles: nsis,msi`
  - `src-tauri/tauri.conf.json` `bundle.targets = ["nsis", "dmg"]`（v0.2.0 修过）
- **触发条件**：每次打 Windows tag（`v*.*.*`）→ CI 跑 `tauri build --bundles nsis,msi` → 镜像超时
- **根因**：
  - v0.2.0 audit 把 `bundle.targets "all"` 改 `["nsis"]`，理由是 Tauri bundler 自动从 `github.com/wixtoolset/wix3/releases` 下 WiX 3.14.1，国内网络必 timeout
  - `tauri.conf.json` 这次跟进 v0.2.5 改成 `["nsis", "dmg"]`（加 dmg 给 macOS），NSIS 在 Windows runner 走 Tauri binary-releases 仓库，下载稳定
  - **但 release.yml Windows matrix 仍写 `bundles: nsis,msi`**，Tauri CLI 的 `--bundles` 参数会**直接覆盖** `tauri.conf.json` 的 `bundle.targets` 字段 → CI 实际仍尝试构建 MSI
  - 文件上方注释自相矛盾（"`v0.2.0 砍 msi 是为避 WiX 镜像 timeout；v0.2.1 试一次,失败下次发板回退`"），**自 2026-06-13 v0.2.0 修到今天 2026-07-30 期间，每发一次 Windows tag 都必撞 WiX timeout 整批 release 挂**
  - 注释里说的「v0.2.1 试一次」实际是 v0.1.0 的 dev 提法，没兑现——本轮 audit 时 release.yml 这行已存在 49 天（v0.2.0 修复 commit `0e3a2b6` 之后没改过）
- **用户影响**：
  - Windows release 路径 100% 失败（除非有显式 WiX mirror 走 rsproxy，但当前 workflow 没配）
  - macOS dmg + Linux 三件套正常，所以发版不阻塞 Mac/Linux 用户
  - 但维护者可能误以为"Windows CI 一直跑不通是基础设施问题"然后花时间在 networking 上调研，**根因就是一行 `bundles:` 写错**
- **证据链**：
  - `release.yml:64-71` 注释明确写 `# v0.2.0 砍 msi 是为避 WiX 镜像 timeout；v0.2.1 试一次,失败下次发板回退`
  - `release.yml:68` 仍写 `bundles: nsis,msi` ← 与注释"v0.2.0 砍 msi"矛盾
  - `tauri.conf.json:36-39` `bundle.targets = ["nsis", "dmg"]` ← 不含 msi
  - Tauri CLI 文档：`--bundles` 参数**覆盖** `tauri.conf.json` 的 `bundle.targets`
- **最小修复**：`.github/workflows/release.yml:68` 改 `bundles: nsis`（删 `,msi`），把对应注释也删掉。**或者**反过来：承认想恢复 MSI 产物，把 `tauri.conf.json` 也加回 `"msi"` 并在 workflow 里加 `WIX_DOWNLOAD_URL` 走 rsproxy 镜像。**不要既不在 config 也不在 CI 给 MSI 加 mirror、还在 CI 强制要 MSI 产物**——这就是当前矛盾。
- **待实测**：本地跑 `pnpm tauri build --bundles nsis,msi` 看是否撞 timeout（10 分钟超时即复现）；修复后跑 `--bundles nsis` 看 NSIS 是否 5 分钟内出产物

---

## 🟡 P2（中）

### D8-003 release.yml Windows matrix 用 MSVC target，本地维护者用 GNU target，CI 产物与本地二进制不一致
- **置信度**：中
- **位置**：
  - `.github/workflows/release.yml:65` — `target: x86_64-pc-windows-msvc`
  - `AGENTS.md` 第 75 行明确写「cargo / rustc | `C:\Users\33348\.cargo\bin\` | **GNU 工具链**，rustc 1.96」
  - `AGENTS.md` 第 76 行「`MinGW` | `D:\Develop\mingw64\bin\` | 提供 `dlltool.exe`，**GNU 工具链下 Rust 链接时必须**」
- **触发条件**：每次发版后用户装 CI 出的 .exe，遇到 GNU-only / MSVC-only 的 dependency 行为差异
- **根因**：
  - 维护者本地 `dev-env.bat` 走 `x86_64-pc-windows-gnu` + MinGW（dlltool 提供）+ `rustc 1.96`（rustup default stable）
  - CI 默认 `windows-latest` runner = `x86_64-pc-windows-msvc`，跟本地工具链不一样
  - 关键差异：
    - GNU → `musage.exe` 静态链 libstd / libgcc，**用户机器不需要 MinGW runtime**
    - MSVC → `musage.exe` 动态链 `vcruntime140.dll` / `vcruntime140_1.dll` / `msvcp140.dll` 等 → 用户机器**没装** VS Build Tools 时 `musage.exe` 启动报 `vcruntime140_1.dll was not found`
    - WebView2 是两边都装（用户自己装或 Tauri NSIS 自动装），所以 WebView2 部分一致
    - **`objc2` + `windows-sys` 的 ABI 在 GNU/MSVC 下行为可能略有差异**（C ABI 调用约定一致，但 SEH / unwind 行为不同）
- **用户影响**：
  - 用户装 CI 出的 .exe → 启动报缺 `vcruntime140_1.dll` → **得装 VC++ Runtime**（NSIS 安装包没自动装）
  - 维护者本地 `pnpm tauri:dev` 跑通 → 误以为产品 ok → 发版 → 用户侧崩 → 反馈来回 1-2 天
  - AGENTS.md 第 78 行明确说「`registry.npmmirror.com`（npm）/ `rsproxy.cn`（crates）」—— Windows GNU 工具链是国内 + MinGW 路径明显是国内镜像环境，但 CI 用 windows-latest runner 走 MSVC 是国际 runner，**这两套环境不是一回事**
- **证据链**：
  - `release.yml:65` matrix `target: x86_64-pc-windows-msvc`
  - AGENTS.md「Windows（开发 + 打包 target）」段「`cargo / rustc` | `C:\Users\33348\.cargo\bin\` | **GNU 工具链**」
  - `Cargo.toml:6-9` 注释「`crate-type = ["staticlib", "rlib"]` | 删 cdylib 是为了绕过 MinGW ld 16-bit ordinal 表上限」—— 这是 **GNU 工具链**特有的坑
- **最小修复**：两种方案二选一：
  - **方案 A（推荐）**：CI 跟本地对齐。`release.yml:65` 改 `target: x86_64-pc-windows-gnu`，并加一步装 MinGW（`choco install mingw` 或 `msys2` 包），去掉 `tauri.conf.json` 里 MSVC 假设
  - **方案 B**：本地跟 CI 对齐。卸载 GNU 工具链，本地改用 MSVC + Visual Studio Build Tools。但这样**会失去**「用户机器不需要装 VC++ Runtime」的 GNU 静态链接优势（NSIS 安装包要加 vcredist 安装步骤）
  - **方案 C（保守）**：CI 加 `x86_64-pc-windows-gnu` 一条新 matrix entry（跟现有 MSVC 并存），双产物分别发到 release 不同文件名（`Musage_0.2.5_x64-gnu-setup.exe` + `Musage_0.2.5_x64-msvc-setup.exe`），让用户自己选。但会增加 release asset 数量和文档负担
  - 选 A 最简单：跟维护者本地工作流 1:1 对齐，cargo crates 缓存也共享
- **待实测**：方案 A 改完后跑一次 `v*.*.*` tag → CI 跑通 → 下载 .exe → Windows 10/11 用户机器裸装（无 VC++ Runtime）→ 双击能跑

---

### D8-004 启动时若 config.json 损坏 + best_effort_from_value 解析成功但字段全空，**load_from_disk 内部已 rename 损坏文件到 .bak.ts，但 save() 仍写默认值（best-effort 半成品）**
- **置信度**：中
- **位置**：`src-tauri/src/config.rs:530-650`（load_from_disk 完整段）
- **触发条件**：用户在外部编辑器手改 config.json 改坏（比如少一个 `}` / 末尾多 `,`），新 schema 解析失败，**但** `serde_json::from_str::<Value>` 能成功（部分 JSON 合法）→ `best_effort_from_value` 抽出**部分字段**成功 → 走「返 cfg 但 cfg 是残缺的」路径
- **根因**：
  - D4-001 fix（2026-07-30 audit）已修「完全损坏 + best_effort 失败 → 返 default → 覆盖」这条路径（改为 `std::fs::rename` 把损坏文件移到 .bak.ts）
  - **但 best_effort 部分成功那条路径没修**：`best_effort_from_value(&raw)` 返一个 `AppConfig`，可能是「`providers: BTreeMap::new()`（空）+ `color_thresholds: [80, 90, 95]`（默认值）+ `floating_x: None`」这种**部分字段被填、部分字段是 default** 的混合体
  - 启动后 `lib.rs:119` 把这个混合体塞进 `AppState.config`
  - 任意 IPC 调 `set_provider_enabled` / `save_config` 触发 `cfg.save()`，**写回磁盘的就是这个「半 default」版本**——之前损坏但**仍可解析出**的字段（比如 `provider_order` 数组里残留的 `"minimax", "deepseek"`）会被覆写成 `Vec::new()`
  - 用户感知：「我改 config.json 加了个 provider，app 启动后 provider 没了」
- **用户影响**：
  - 完全损坏场景下 D4-001 已能恢复（备份 + default）
  - **部分损坏**场景下，D4-001 备份仍生效，但**用户原本有效的字段被覆写**——D4-001 修了一半
  - 比 D4-001 严重：D4-001 是「全坏」，用户至少看到「loading 完后所有设置消失」能立即察觉；这里是「部分坏」，用户只看到「我自定义的某些 provider 没了 / 排序乱了」，可能用了几周才发现
- **证据链**：
  - `config.rs:594-650` 是 best_effort 路径，函数 `best_effort_from_value` 返 `AppConfig` 但内部**对每个字段都 try 解析，缺则用 default**
  - 假设损坏 JSON 是 `{"providers": {"minimax": {"enabled": false}}, "floating_x": 99999` （结尾少 `}`）→ `from_str::<AppConfig>` 失败（整文件解析失败）→ `from_str::<Value>` 成功（合法 JSON 片段）→ `best_effort_from_value` 抽出 `providers: {"minimax": ...}` 和 `floating_x: 99999` → 返 cfg
  - 但 cfg 里**没有** `provider_order` → best_effort 用 default `Vec::new()` → 写回时覆写
  - D4-001 fix 只处理 `Value` 解析也失败的情况
- **最小修复**：在 `best_effort_from_value` 内部对**关键数组/Map 字段**做「源文件有这个字段名才采用，否则 fallback to 上一份成功的 cfg（缓存在 AppState 或额外写一份 `config.json.previous-good`）」—— 但这要持久化上一份 good cfg。
  - **更轻的修法**：当 `best_effort_from_value` 走通时，**不返 cfg**，而是 `return Err` 让 lib.rs 走 default + .bak.ts 备份路径，**保留损坏原文件供用户手动恢复**。best_effort 路径只在「关键字段全部解析成功」时走，否则就是损坏。
  - **再轻的修法**：在 `best_effort_from_value` 末尾检查「providers 数量 + provider_order 数量 + schema_overrides 数量 跟损坏源文件同位置的关键字段是否一致」，不一致就放弃 best_effort 走 default。
- **待实测**：构造一份 `{"providers": {"minimax": {"enabled": true}}`（缺右大括号）的损坏 config.json → 启动 app → 在设置面板改一个无关字段触发 save_config → 看 config.json 里 `provider_order` 是否被清空

---

### D8-005 config.rs save() 内部 `std::fs::set_permissions(&tmp, 0o600)` 失败时整 save 失败，但 main path 仍写入 world-readable 状态（rename 之前的 tmp 文件）
- **置信度**：中
- **位置**：
  - `src-tauri/src/config.rs:774-790`（save() 内部 chmod 段）
  - `src-tauri/src/config/extra_instances.rs:204-212`（同款 chmod 段）
- **触发条件**：
  - 极少见，但**实际可触发**：在 Windows 容器 / WSL2 文件系统（9P 共享）/ 某些 SMB 挂载上，`set_permissions(0o600)` 返回 `PermissionDenied`
  - 或 `chmod` 调用本身 panic（比如 `#[cfg(unix)]` 路径命中但 kernel 不支持 mode 0o600 —— 老旧 Linux 内核 / FUSE 文件系统）
- **根因**：
  - `save()` 流程是：`write tmp → fsync tmp → chmod 0600 tmp → rename tmp → path`
  - chmod 失败时 `return Err(format!("chmod 0600: {e}"))`，但 `tmp` 文件**已经**写入 + fsync + 留在磁盘上（**world-readable 状态，因为没 chmod 成功**）
  - 下次启动 `load_from_disk` 找不到 `config.json`（被 tmp 占据，rename 没发生）→ 走「不存在 → return default」路径
  - **且 tmp 文件含 keys.json 的明文 cookie 字段**（虽然 main path 只走 config，**但 extra_instances.rs 的 save() 也走同款 chmod 流程**——这个文件不存 credentials 没事，但养成习惯）
- **用户影响**：
  - 概率低（生产 macOS / Windows 本地文件都不会触发）
  - 一旦触发：用户所有配置丢失 + keys.json / extra_instances.json 留下**world-readable** 的临时副本（明文 token 暴露给同机其他用户）
  - macOS `chmod 0o600` 永不失败（APFS 原生支持 POSIX mode），所以只在 Windows + 特殊文件系统上发生
  - 但**这是 hard error 路径**，跟其他 graceful degrade 路径（chmod 失败时 `tracing::warn!` 继续）不一致
- **证据链**：
  - `config.rs:784-786` `if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&tmp) { let _ = f.sync_all(); }` —— sync_all 失败是 `let _ =` 吞错
  - `config.rs:789-794` chmod 0o600 失败 `return Err(format!("chmod 0600: {e}"))` —— 直接返错，**没清理 tmp**
  - `extra_instances.rs:204-212` 同款逻辑同款问题
- **最小修复**：把 chmod 失败从 hard error 降级为 warn，跟 fsync_all 一样：
  ```rust
  #[cfg(unix)] {
      use std::os::unix::fs::PermissionsExt;
      if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
          tracing::warn!(error = %e, "config.json.tmp chmod 0600 失败, 继续 save — 文件会保持 default permissions");
      }
  }
  ```
  - 同步把 `extra_instances.rs:204-212` 改成同款降级
  - **不要**把 tmp 删了再返错 —— tmp 至少含最新数据，rename 成功就是 atomic update；rename 失败再清理
- **待实测**：在 WSL2 跨 /mnt 挂载 NTFS 分区（已知 chmod 行为异常）跑 `pnpm tauri:dev` → 改个设置触发 save → 看是否 panic / 静默

---

## 🟢 P3（低）

### D8-006 `quit_app` 用 `tokio::time::sleep(150ms)` 等 poller drain 是 magic number，慢网络/大量 in-flight task 时会丢
- **置信度**：中
- **位置**：`src-tauri/src/commands/mod.rs:1167` `tokio::time::sleep(std::time::Duration::from_millis(150)).await;`
- **触发条件**：
  - 用户在 poller 主循环跑（12 provider 同时在 in-flight）时点托盘「Quit」
  - 或 poller spawn 的 `refresh_single_from_poller` 卡在 TLS 握手（reqwest 默认无 timeout 上限 = 永久等）
- **根因**：
  - `quit_app` 调 `poller::SHUTDOWN.notify_waiters()` + `SHUTDOWN_NATIVE_THREADS.store(true)`，然后 sleep 150ms，再 `app.exit(0)`
  - poller 主循环接 SHUTDOWN 后的 drain 路径：`taken.abort_all()` 取消 task → `while let Some(res) = taken.join_next().await` 等所有 task 退出
  - 但 **reqwest HTTP 请求默认无 client-level timeout**（代码里 `reqwest::Client` 没配 `.timeout()`），cancel 一个 in-flight HTTP 请求只是断开 future 的 await，**底层 socket 仍会跑完 TLS 握手 + HTTP 重试逻辑**（甚至 follow redirect）
  - tokio JoinSet 的 join_next 等待 task 退出 = 等待 `refresh_single_from_poller` 这个 future 完结；future 完结 = fetch() 完结；fetch 完结 = 等到 response 回来或 reqwest 内部 timeout（默认无）
  - 150ms 远小于 reqwest 默认 timeout（10 秒甚至无限制）→ 150ms 后 `app.exit(0)` 强杀进程，**poller 主循环「poller 主循环退出」日志永远不会打印**，drain 路径根本走不完
- **用户影响**：
  - macOS / Windows：app 退出，用户无感（GUI 已消失）
  - **Tauri 后台日志被截断**：「Quit 后日志末尾没有 'poller 主循环退出' 干净收尾」 → 调试时容易误判「app 是不是 panic 退出了？」
  - **H1 fix（2026-07-30 audit）的本意是 graceful shutdown**——让 poller 有机会 drain in-flight fetch，把 backoff 状态、log_provider_error 写完。但 150ms 这个 magic number 没真的能等 drain
- **证据链**：
  - `commands/mod.rs:1163-1169` 三步：notify SHUTDOWN + set atomic + sleep 150ms + exit
  - `poller.rs:170-185` drain 路径：taken.abort_all() + join_next() 循环
  - `providers/mod.rs` 的 `shared_client()` 检查是否配 timeout（`reqwest::Client::builder().timeout(...)`）—— **未配**
- **最小修复**：两条任选：
  1. 给 `shared_client()` 加 `.timeout(Duration::from_secs(15))` —— reqwest 自身 15s 兜底，cancel 后下次 join_next 自然返回
  2. 把 sleep(150) 换成 `wait poller task`：`let poller_handle = ...; poller_handle.await.ok();`（需要 `poller::start` 返 `JoinHandle<()>`，当前是 fire-and-forget）
  3. **最务实**：sleep 150 → 200 → 500 → 1000 渐进 backoff，每段都 try_join_next 看 drain 是否完成 —— 但代码量大
  - **推荐 1+2 组合**：reqwest timeout 15s 兜底 + poller start 返 JoinHandle
- **待实测**：开发模式 dev tools 看 reqwest 是否有 default timeout；手测 dev 模式 quit 后日志

---

### D8-007 i18n helper 在 dev 模式 raw key fallback 走 `console.warn`，但 settings 面板的「missing key dump」入口只调 `_resetMissingKeysForTest`，没暴露给普通开发者
- **置信度**：低
- **位置**：`src/i18n/index.ts:51-56` `dumpMissingKeys` / `_resetMissingKeysForTest`
- **触发条件**：开发者想批量看前端所有 missing i18n key（类似后端 `dumpMissingKeys`）
- **根因**：
  - `dumpMissingKeys()` 在 dev 模式会累积 missing keys，导出为 `string[]`
  - 但**没暴露到 settings 面板 dev menu**，只 `_resetMissingKeysForTest` 暴露给 vitest
  - 开发者想看 → 必须 `console` 手敲 `import('/src/i18n/index.ts').then(m => console.log(m.dumpMissingKeys()))` —— 走 Vite dev server dynamic import 跟 D8-001 撞同类坑（Vite chunk 路径问题）
  - 实际**没人会用**，所以 missing key bug 持续隐藏（D8-001 就是这么漏掉的）
- **用户影响**：纯开发者体验，不影响用户
- **证据链**：`index.ts:53` 注释「在 dev 模式下提供 dumpMissingKeys() 让开发者一次性看到所有缺失 key 列表(可手动 console 调用, 也可在 settings 面板 dev menu 调)」—— **但 settings 面板 dev menu 入口我没找到**
- **最小修复**：在 `src/settings/about.ts` 或 dev-only 「DevTools」按钮旁加一个「Dump missing i18n keys」按钮，点了把 `dumpMissingKeys()` 返的 array 复制到剪贴板 + flash 提示。或更轻：每 5 秒检测一次 missing key 数量，>0 时在托盘 tooltip 末尾加 `⚠ {N} missing i18n keys`
- **待实测**：触发 D8-001 的 4 个路径，看 dev 模式 console 是否能拿到 4 个 key

---

### D8-008 `cfg.save()` 的 truncate_old_backups 限制 5 份，但 `keys.json` 没对应清理 —— keys.json.bak 残留无界
- **置信度**：低
- **位置**：
  - `src-tauri/src/config.rs:495-500` `truncate_old_backups(parent, "config.json", 5)` / `truncate_old_backups(parent, "keys.json", 5)`
  - 实际**只在 load_from_disk 启动时**调一次
- **触发条件**：
  - `AppConfig::save()` 内部没调 `truncate_old_backups` —— `keys.json.bak.*` 只在损坏启动时一次性清理 5 份
  - 正常运行时 keys.json 被覆写 N 次（用户保存 key 切 12 个 provider × 几轮），rename 旧 `keys.json` → `keys.json.bak.0`（**等等，看代码**）
- **根因**：让我重新核对 —— `config.rs:768-810` 段 `save()` 内部用的 `write_keys_atomic` 流程：先 write tmp → rename tmp 覆盖，**没有走 .bak 备份路径**。所以正常 save 不产生 .bak 文件。`truncate_old_backups` 在 load 时跑是清理历史已存在的 .bak
  - 真正问题是 **D4 报告里**已经标过的：损坏 config.json 触发的 `.bak.<ts>` 文件无清理机制 —— 但 D4 报告说"已确认旧报告已修"指的是 truncate 调到了 5 份上限
  - 实际：每次启动 `load_from_disk` 走到 backup 路径都会**先 truncate 5 份**再**新增 1 份**，所以总数稳在 ≤ 6
  - 这个 D8-008 是 **false positive**，撤回
- **撤回说明**：重新看 `config.rs:486-500` `truncate_old_backups` 确实在 `load_from_disk` 进入备份前调，且 `load_from_disk` 是启动时一次，rotate 是稳态的。D4 报告把它列为已修，本轮无新问题。**撤回 D8-008，不写入报告**。
- **（以上是写作过程中的 self-correction 留底，不计入最终 bug 数）**

---

## ✅ 本轮确认无新 bug 的域（避免遗漏）

按"必查点"清单逐项确认：

1. **commands.rs `tokio::sync::Mutex` vs `std::sync::Mutex`** —— 一致：3 处 `std::sync::Mutex`（1887/2193/2224）都是**短同步操作**（HashMap insert / OnceLock init），与 D4-002 标注的「save_lock 是 std::sync.Mutex 但调方在 tokio 上下文持锁跨 await」是**不同**的 short-lived 场景；没新发现
2. **所有 invoke handler 是否有 `Result<_, String>` 错误返回** —— 4 个无 Result：`quit_app`（用 `app.exit(0)` 一次性）、`get_app_version`（同步读 package_info，无错路径）、`rebuild_tray`（其他内部 helper），均合理；无新发现
3. **panic 是否被 catch_unwind 包裹** —— Tauri 2 不自动 catch，**这是已知风险**（D8-006 是同类），但全代码 `grep -n "unwrap()|expect("` 只剩 test-only，生产路径已用 `unwrap_or_else(|e| e.into_inner())` 处理 mutex poison；`save_lock` 改 tokio::sync::Mutex 是 D4-002 改造，本轮不重复
4. **config.rs 启动加载 config.json / keys.json 失败是否降级到默认** —— D4-001~009 已修
5. **poller.rs App 退出时 per-provider task 是否 cancel** —— D5-101 已修（SHUTDOWN + drain）
6. **poller_backoff.rs 30 分钟上限是否正确实现** —— D5-066 / D5-038 / D5-074 已修；`MAX_BACKOFF_SECS = 1800` 已 hard-code + test 守门
7. **tauri.conf.json CSP / capabilities / allowlist / wildcard** —— CSP 严（`default-src 'self'; img-src 'self' data:` 等），无 wildcard；capabilities 按 window 拆分（default / settings-extra / anysearch-login / stepfun-login / xiaomi-login），process:default 只给 settings；`bundle.targets = ["nsis", "dmg"]`（与 release.yml 不一致是 D8-002）
8. **Cargo.toml `[features]`** —— `default = ["custom-protocol"]`，custom-protocol 是 Tauri 推荐 feature，无 unsafe-opfs 之类危险默认
9. **vite.config.ts `assetsInlineLimit: 0`** —— 有效（v0.2.0 修 SVG logo broken icon 落地）；`modulePreload.polyfill: false` 跨平台一致性 fix；rollupOptions 双 entry（main / settings）解决 settings.html 404
10. **tsconfig.json strict / moduleResolution** —— `strict: true` / `noUnusedLocals: true` / `moduleResolution: "bundler"`，配置正确
11. **release.yml + ci.yml action 全部钉 SHA** —— 已确认（注释里写明每个 action 的 SHA 对应版本，ci.yml 顶部列出 8 个 SHA）
12. **i18n locales `t!()` key 存在** —— 后端 171 个 t!() key 100% 在 en.json 里（已 diff）；**前端 432 个 t() 调 4 个缺（D8-001）**
13. **error 消息是否泄漏密钥 / URL token / 内部路径** —— 已审过 `error.xiaomi.cookie_format_invalid` 等用 `%{ch}` 占位符（不泄漏 token 本身）；`error.common.network` 用 `%{url}` 但截断到 host（logstore.rs:403-441 redact 已落地）
14. **panicking unwrap 全代码 grep** —— 8 处生产路径 unwrap/expect 集中在 `lib.rs:592`（dump CLI tokio runtime 创建，stdout 不致命）、`tray.rs:1180/1187`（load_font 测试）、`anysearch_login.rs:390` / `stepfun_login.rs:270/502` / `xiaomi_login.rs:534`（hardcoded URL parse）、`logstore.rs:136/304`（regex 编译 + 后台线程启动）—— 均在 hardcoded 已知正常输入下，panic 概率 ≈ 0

---

## 修复优先级建议

- **立刻**（release blocker）：D8-001（4 个 missing i18n key）+ D8-002（release.yml MSI 复辟）
- **下次发版前**：D8-003（CI MSVC vs 本地 GNU 决策）
- **v0.2.6 窗口**：D8-004（best_effort 半成品覆盖） + D8-005（chmod 失败 hard error）
- **v0.3 backlog**：D8-006（quit_app 150ms 魔数） + D8-007（missing key dump 入口）

总览：本轮新发现 1 P0 / 1 P1 / 3 P2 / 2 P3 = 7 条，集中在三方面：① 前端 i18n missing key 是用户直接可见的 UX bug 类别（D8-001），跟历史 M9 fix 同模式但没顺手扫一遍；② release.yml 与 tauri.conf.json 的 bundle.targets 不一致 + CI/本地工具链分叉（D8-002 + D8-003），都是「曾经修过的 bug 留下半成品 / 配置漂移」类；③ config.rs / poller.rs 的边界路径（best_effort 半成品 / chmod 失败 / quit 魔数）虽然 D4/D5 大报告已扫过，但还有 3 条 low-prob / high-impact 的尾巴没收。
