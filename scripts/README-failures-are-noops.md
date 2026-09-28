# 失败回滚的两种写法：一种有用，一种是空操作

> 来源：2026-09-28 全量审查。9-04 报告的 M30 / M32 / M33 都标为"已修复"，
> 实际改的东西**不产生任何效果**。这类 bug 比未修的更危险：报告说已修、
> CI 绿、代码里躺着看起来正确的逻辑，但功能是坏的。
>
> **review 时看到 `catch` 里写回旧值，必须确认那个"旧值"是真的旧值。**

## 空操作（错）：在 `change` 回调内部读当前值当"旧值"

```ts
select.addEventListener("change", () => {
  const previous = select.value;   // ❌ change 派发时控件值已经被用户改掉了
  const v = select.value as "cn" | "en";
  if (v !== "cn" && v !== "en") return;
  void setMinimaxRegion(v).catch(() => {
    select.value = previous;       // ❌ previous === v，赋回它本来就有的值
  });
});
```

`change` 事件是在**值已经变了之后**才派发的。所以在回调里读 `select.value`
读到的永远是新值，`previous === v`，`catch` 里的回滚是纯粹的 no-op。

用户看到的是：红色 flash 提示"切换失败"，但**控件仍停在新值**，
而后端仍是旧值 —— 用户以为改了，重开设置面板才发现没生效。

## 有效（对）：闭包记录"最近一次成功值"

```ts
let lastGood = select.value;        // ✅ 初始化为当前（已持久化的）值
select.addEventListener("change", () => {
  const v = select.value as "cn" | "en";
  if (v !== "cn" && v !== "en") return;
  void setMinimaxRegion(v)
    .then(() => { lastGood = v; })          // 成功后才更新
    .catch((e) => {
      select.value = lastGood;              // ✅ 回到最近一次**成功**的值
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
});
```

同样的模式在本项目里的正确实现：
- `settings/app.ts` — `currentStyle` / `currentSource` / `lastGoodTrayColor`
- `settings/floating.ts` — `lastGoodMargin`
- `settings/order.ts` — 在 `mousedown` 时抓 `wasEnabled`（拖拽天然有"按下的那一刻"这个时间锚点）

2026-09-28 已把这 7 处 `source-extras.ts` 的控件 + `floating.ts` 的 3 个
checkbox + `providers.ts` 的 2 处统一改成 `lastGoodX` 模式。

## 同类形状的另外两个坑

### 1. 回滚到"没有对应 option 的值"会把下拉变空白

```ts
// ❌ tray_source 下拉只渲染 11 个固定 option，后端却接受任何能 find_source 的 id
//    catch 里 select.value = currentSource，若该值不在列表中 → selectedIndex = -1
//    下拉变成空白一行
```

**做法**：渲染时若 `currentSource` 不在列表，追加一条 `value=currentSource`
的 option，而不是让回滚去撞不存在的值。

### 2. 校验早退前不回填盘上真值

```ts
// ❌ 填了非法间隔值，只 flash 然后 return
if (!Number.isInteger(n) || n < 10 || n > 86400) {
  flash(...); return;          // 输入框里仍是一个从未生效的数字
}
```

**做法**：`return` 前 `input.value = <盘上的真值>`。

## review 清单

看到一个 `.catch()` 里做 UI 回滚时，问三个问题：

1. 回滚的"旧值"是从**事件派发前**的某个时间锚点取的吗？还是在回调里现读？
2. 回滚目标**一定存在**吗（下拉 option / DOM 节点 / 控件）？
3. 校验失败早退的路径上，控件有被回填成盘上真值吗？

三条任一为否，这个回滚就是空操作或半空操作。

## 自动化

这类 bug **不能**靠"成功路径"的断言测出来（空操作在成功路径下同样通过）。
要测就必须断言**失败路径**：mock IPC 抛错 → 断言控件值确实变了回去。

`src/settings/order.test.ts` 目前只覆盖 order.ts 的纯函数，回滚类逻辑零
自动化覆盖 —— 这是已知的测试缺口，新增这类逻辑时请顺带补一条失败路径断言。
