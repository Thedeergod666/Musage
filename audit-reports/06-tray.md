# 托盘 + 动态图标审查报告

**审查域**:`src-tauri/src/tray.rs` (1035 行)

## 概览
- 总 bug 数:13 (CRITICAL 0 / HIGH 2 / MEDIUM 5 / LOW 6)
- 整体健康度评分:6.5/10(核心功能扎实,但 Percent 文本溢出 + 亮色主题支持是明显短板)

## CRITICAL
未发现 CRITICAL 级问题。

## HIGH

### H1. 百分比文本在常用数值下必然被裁切
- **位置**:`src-tauri/src/tray.rs:814`
- **类型**:文本布局 / 边界溢出
- **代码**:
```rust
let scale = PxScale::from(s as f32 * 20.0 / 32.0);
let top = format!("{}%", util_top.round() as i64);
let x = (ICON_SIZE as i32 - w - pad_right).max(1);
draw_text_mut(img, color, x, y, scale, font, text);
```
- **描述**:固定 `scale=20` 无法容纳两位、三位百分比。使用代码实际加载的 Arial Black 实测:
  - `0%`:23.643px
  - `99%`:33.102px
  - `100%`:42.562px
  - 32px 画布扣除右侧 padding 后仅约 30px;`99%` 已触碰并越过右边界,`100%` 会裁掉约 12px,通常连 `%` 都显示不完整。
- **影响**:默认 Percent 样式在绝大多数正常用量值下显示残缺,100% 上限状态尤其严重。
- **修法**:先格式化文本,再根据 `imageproc::drawing::text_size` 动态缩小字号,直到 `width <= ICON_SIZE - 2 * padding`;两行分别按真实高度垂直居中,不要硬编码 `y=0/16`;添加 0%/9%/10%/99%/100% 像素包围盒测试。

### H2. 动态图标不适配亮色主题,默认 Percent 可能近乎不可见
- **位置**:`src-tauri/src/tray.rs:223`、`:693`、`:820`
- **类型**:暗色/亮色模式 / 平台适配
- **描述**:Percent 是透明背景上的纯白字,Bars 的已用部分也是纯白;代码既不检测主题,也没有设置 macOS template image。`tray-icon` 默认 `icon_is_template=false`,而且后续 `tray.set_icon(...)` 会继续按非 template 图像更新。
- **影响**:macOS 亮色/浅色菜单栏:Percent 白字几乎消失;Windows 亮色任务栏同样可能失去对比度;Bars 在 100% 时白色填充完全覆盖暗轨道。
- **修法**:macOS 使用 template-compatible alpha mask + `set_icon_with_as_template(..., true)` 原子更新;Windows/Linux 给白色文字和填充加 1-2px 深色描边。

## MEDIUM

### M1. Poller 在托盘通道初始化前启动,首个更新可能永久丢失一轮
- **位置**:`src-tauri/src/lib.rs:200`、`:215`、`src-tauri/src/tray.rs:101`
- **类型**:初始化竞态
- **描述**:`poller::start` 会立即 spawn 全量拉取,但 tray channel 要到后面的 `tray::setup` 才创建。无 credential、新用户或全部 provider 快速失败时,首次刷新可以在 setup 前完成,更新被明确丢弃。
- **影响**:托盘可能继续显示"加载中"图标和旧 tooltip,直到下一次 60 秒轮询。
- **修法**:先执行 `tray::setup`,再启动 poller;channel 未初始化时保存 latest pending update,不直接丢弃。

### M2. 无界队列携带完整 Snapshot,存在明显内存峰值和主线程积压
- **位置**:`src-tauri/src/tray.rs:62`、`:440`、`:538`
- **类型**:资源管理 / 并发队列
- **描述**:每次更新深拷贝完整 `QuotaSnapshot`,其中包含每个 provider 的 `raw` JSON;随后放进无界 channel。
- **影响**:12+ provider 同时到期会突发排入多份完整快照,可能出现数百 MB 级临时内存峰值。
- **修法**:建立只包含 `success/rows/unique_id/display_name/fetched_at` 的紧凑 `TraySnapshot`,排除 `raw`;使用 `watch` channel 或容量为 1 的 bounded channel。

### M3. macOS 26 的 64px Retina 修复只覆盖静态 Logo,动态样式仍是 32px
- **位置**:`src-tauri/src/tray.rs:214`、`:592`
- **类型**:Retina / HiDPI
- **描述**:`tray-base.png` 已是 64×64,但默认 Percent 和 Bars 仍生成 32×32。Retina 菜单栏约需 36 个物理像素,因此动态图标仍会发生 32→36 的上采样。
- **影响**:默认 Percent/Bars 继续存在 macOS 26 hotfix 所针对的模糊、锯齿问题。
- **修法**:macOS 也将动态渲染 source 设为 64×64。

### M4. Windows tooltip 超过 127 个 UTF-16 单元后会被静默截断
- **位置**:`src-tauri/src/tray.rs:882`
- **类型**:平台限制 / 多 provider 展示
- **描述**:Windows `NOTIFYICONDATAW.szTip` 只有 128 个 UTF-16 单元。12+ provider 必然远超上限。
- **影响**:Windows 只能看到前几个 provider,emoji surrogate pair 还可能切开。
- **修法**:Windows 单独生成紧凑 tooltip,保证正文最多 127 UTF-16 单元,按 Unicode scalar 边界截断。

### M5. Percent 分支没有复用 Bars 的 0-100 边界钳制
- **位置**:`src-tauri/src/tray.rs:696`、`:822`
- **类型**:进度计算边界
- **描述**:Bars 正确 clamp,但 Percent 直接格式化原始值。MiniMax 旧 count schema 在 remaining > total 或负值时可计算出负百分比或超过 100%。
- **影响**:可能显示 `-25%`、`999%`;极端浮点值转 i64 后生成超长文本,触发 H1 严重裁切。
- **修法**:抽出统一 `sanitize_percent`,先处理 `is_finite`,再 clamp 到 `0.0..=100.0`。

## LOW

### L1. 左键 Down 状态未消费,快速右键仍可能误认合成 Left Up
- **位置**:`src-tauri/src/tray.rs:343`
- **类型**:事件状态机
- **描述**:真实左键 Up 处理后,时间戳仍保留 500ms。macOS `performClick` 合成的 Left Up 仍会匹配这个旧 Down。
- **修法**:Up 时用 `swap(0, Ordering::SeqCst)` 消费 Down,使用单调时钟。

### L2. MiniMax 并列时没有实现注释承诺的"小 instance_index 优先"
- **位置**:`src-tauri/src/tray.rs:638`
- **类型**:多实例优先级
- **描述**:比较器只比较 5h;相等时返回 `Equal`,`Iterator::max_by` 会保留后出现的元素。
- **修法**:显式以 `(five_hour_util, Reverse(instance_index))` 比较。

### L3. "Win-only"强制置顶项在 macOS/Linux 上仍以禁用项出现
- **位置**:`src-tauri/src/tray.rs:410`
- **类型**:平台菜单
- **描述**:`cfg!` 只设置 enabled,不会移除菜单项。
- **修法**:用 `#[cfg(target_os = "windows")]` 构造不同的 items 列表。

### L4. 自定义 `assets/font.ttf` 在发布包中实际不可用
- **位置**:`src-tauri/src/tray.rs:145`
- **类型**:字体资源加载
- **描述**:`CARGO_MANIFEST_DIR` 是编译机器上的绝对源码路径。CI 构建后的安装包仍尝试读取 CI workspace 路径,用户机器上不存在。
- **修法**:固定字体用 `include_bytes!`;可替换字体加入 Tauri resources。

### L5. 切换语言只重建菜单,现有 tooltip 不会立即更新
- **位置**:`src-tauri/src/lib.rs:163`、`src-tauri/src/tray.rs:489`
- **类型**:托盘状态同步
- **修法**:locale 变化时同时使用当前内存 snapshot 重发 tooltip,或新增轻量 `RefreshTooltip` request。

### L6. 显隐操作和异步派发对调用方报告"假成功"
- **位置**:`src-tauri/src/tray.rs:231`、`:520`
- **类型**:错误处理
- **描述**:窗口 API 错误全部吞掉;`dispatch_tray_request` 即使 channel 未初始化或 receiver 已退出,仍返回 `Ok(())`。
- **修法**:抽取统一 `toggle_floating_window` 并记录每个失败;让 `dispatch_tray_request` 返回 `Result`。

## 已核对项

- `Icon` 替换由 `tray-icon` 持有并释放旧平台句柄,未发现每次重绘导致的确定性句柄泄漏。
- PNG 解码产生的临时 RGBA buffer 会正常释放;主要资源风险来自无界队列中的完整 snapshot,而非 PNG。
- macOS 26 静态 `tray-base.png` 64×64 hotfix 本身有效;问题仅是动态 Percent/Bars 没有同步升级到 64px。
- Bars 已正确处理 0%、100% 和超过 100% 的宽度钳制。
- 12+ provider 的动态图标优先级目前是"仅 MiniMax,取 5h 最高实例";除并列 tie-break 外未按 bug 处理。
