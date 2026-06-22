//! JSON path 读取 + 数字解析 helpers
//!
//! PR 3 加 [`crate::providers::custom::CustomSource`] 时抽出共享，原本散落在
//! 多个 provider 里的 `parse_f64` / `value.get("a.b.c")` 拼接逻辑。
//!
//! ## 设计原则
//!
//! - **Mini 解析器** —— 只支持 `a.b.c` 和 `a[0]` 两种语法，**不**支持
//!   表达式、函数调用、通配符（避免 ccswitch extractor 的 "用户复制错脚本" 坑）
//! - **零分配迭代** —— `read_path` 返回 `Option<&'a Value>`，生命周期绑在 root
//!   上，调用者不需要 clone
//! - **容错** —— 路径不存在 / 中间节点不是 object / 数字是字符串 → 返 None
//!   而非 panic

use serde_json::Value;

/// 读 JSON path。支持 `a.b.c` 和 `a[0]`，可组合（`data.balance[0].amount`）。
///
/// 返回 `None` 当：
/// - 路径为空
/// - 中间节点不是 object（无法继续 `.field`）
/// - 中间节点不是 array（无法继续 `[idx]`）
/// - 最终节点不存在
///
/// **容错**：前导 `.`（如 `.data`）被静默忽略，等价于 `data`。这是
/// **L5 fix（2026-06-19）** 后明确的行为：custom source 用户手填 path 时
/// 经常不自觉加前导点（JSON path 业界用法差异），silent accept 比
/// silent reject 友好。已有测试 [`tests::read_path_leading_dot_tolerated`]
/// 锁定此行为。
///
/// ## Examples
///
/// ```
/// use serde_json::json;
/// let v = json!({"data": {"quota": 100, "tags": ["a", "b"]}});
/// assert_eq!(crate::providers::parse::read_path(&v, "data.quota"), Some(&json!(100)));
/// assert_eq!(crate::providers::parse::read_path(&v, "data.tags[1]"), Some(&json!("b")));
/// assert_eq!(crate::providers::parse::read_path(&v, "missing"), None);
/// assert_eq!(crate::providers::parse::read_path(&v, "data.tags[5]"), None);
/// // 前导 `.` 被静默跳过（容错）
/// assert_eq!(crate::providers::parse::read_path(&v, ".data.quota"), Some(&json!(100)));
/// ```
pub fn read_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let mut current = root;
    let mut chars = path.chars().peekable();

    // 第一段不带前导 `.`
    let mut buf = String::new();
    while let Some(&c) = chars.peek() {
        match c {
            '.' | '[' => break,
            _ => {
                buf.push(c);
                chars.next();
            }
        }
    }
    if !buf.is_empty() {
        current = current.get(&buf)?;
        buf.clear();
    }

    // 后续段：可能是 `.field` / `[idx]` / `.field[idx]`
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                // 段名到下一个 `.` 或 `[` 或结尾
                while let Some(&nc) = chars.peek() {
                    if nc == '.' || nc == '[' { break; }
                    buf.push(nc);
                    chars.next();
                }
                if buf.is_empty() { return None; }
                current = current.get(&buf)?;
                buf.clear();
            }
            '[' => {
                // 数字到 `]`
                let mut idx_str = String::new();
                while let Some(&nc) = chars.peek() {
                    if nc == ']' { break; }
                    idx_str.push(nc);
                    chars.next();
                }
                // 必须以 `]` 收尾
                if chars.next() != Some(']') { return None; }
                let idx: usize = match idx_str.trim().parse() {
                    Ok(n) => n,
                    Err(_) => return None,
                };
                current = current.get(idx)?;
            }
            _ => return None,  // 不应该到这里（段名都在 . / [ 分支里消费）
        }
    }

    Some(current)
}

/// 兼容数字 / 字符串 JSON 表示的 f64 解析。
///
/// - `v` 是 `f64` / `i64` / `u64` → 直接返 Some
/// - `v` 是字符串 → `s.trim().parse()`，失败返 None
/// - `v` 是 null / object / array / bool → None
///
/// 数字字符串容忍：前导 0 / 包含小数点 / 包含指数都接受。
pub fn num_f64(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() { return Some(n); }
    if let Some(n) = v.as_i64() { return Some(n as f64); }
    if let Some(n) = v.as_u64() { return Some(n as f64); }
    if let Some(s) = v.as_str() {
        return s.trim().parse().ok();
    }
    None
}

// ── 单元测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── read_path ──

    #[test]
    fn read_path_top_level() {
        let v = json!({"data": 100});
        assert_eq!(read_path(&v, "data"), Some(&json!(100)));
    }

    #[test]
    fn read_path_nested() {
        let v = json!({"data": {"quota": 100, "used_quota": 50}});
        assert_eq!(read_path(&v, "data.quota"), Some(&json!(100)));
        assert_eq!(read_path(&v, "data.used_quota"), Some(&json!(50)));
    }

    #[test]
    fn read_path_array_index() {
        let v = json!({"data": {"tags": ["a", "b", "c"]}});
        assert_eq!(read_path(&v, "data.tags[0]"), Some(&json!("a")));
        assert_eq!(read_path(&v, "data.tags[2]"), Some(&json!("c")));
    }

    #[test]
    fn read_path_array_then_field() {
        let v = json!({"users": [{"name": "alice"}, {"name": "bob"}]});
        assert_eq!(read_path(&v, "users[1].name"), Some(&json!("bob")));
    }

    #[test]
    fn read_path_missing_returns_none() {
        let v = json!({"data": {"quota": 100}});
        assert_eq!(read_path(&v, "data.missing"), None);
        assert_eq!(read_path(&v, "totally.missing.path"), None);
    }

    #[test]
    fn read_path_index_out_of_bounds() {
        let v = json!({"arr": [1, 2, 3]});
        assert_eq!(read_path(&v, "arr[5]"), None);
    }

    #[test]
    fn read_path_non_object_intermediate() {
        // data 是数字，再 .field 无意义
        let v = json!({"data": 100});
        assert_eq!(read_path(&v, "data.field"), None);
    }

    #[test]
    fn read_path_empty_or_invalid() {
        let v = json!({"data": 100});
        assert_eq!(read_path(&v, ""), None);
        assert_eq!(read_path(&v, "   "), None);
        assert_eq!(read_path(&v, "[abc]"), None);  // 非数字
        assert_eq!(read_path(&v, "data[unclosed"), None);  // 缺 ]
    }

    #[test]
    fn read_path_brackets_with_space() {
        // 宽容：接受 [ 0 ] 这种带空格的写法（JSON path 业界不标准但无害）
        let v = json!({"arr": [10, 20]});
        assert_eq!(read_path(&v, "arr[ 1 ]"), Some(&json!(20)));
    }

    #[test]
    fn read_path_leading_dot_tolerated() {
        // **L5 fix（2026-06-19）**：之前 read_path 对前导 `.` 静默容错
        // （`.data` 跟 `data` 等价），但 API 文档没承诺这个行为。改为：
        //   - 文档明确记录："前导 `.` 被静默跳过"
        //   - 测试锁定现有行为，避免后续无意改动破坏用户手填的 path
        //
        // 不改成 strict（拒绝前导 `.`）的原因：custom source 用户手填 path
        // 时经常不自觉加 `.`（JSON path 业界用法差异），silent accept 比
        // silent reject 友好。
        let v = json!({"data": {"quota": 100}});
        assert_eq!(read_path(&v, ".data.quota"), Some(&json!(100)));
        assert_eq!(read_path(&v, ".data"), Some(&json!({"quota": 100})));
        // 多个前导 `.` 不在当前实现支持范围 (read_path 只剥一个前导 `.`)
        // `..data.quota` → 返 None (实现不解析 `.` 作 segment)
        assert_eq!(read_path(&v, "..data.quota"), None);
    }

    // ── num_f64 ──

    #[test]
    fn num_f64_accepts_int() {
        assert_eq!(num_f64(&json!(100)), Some(100.0));
        assert_eq!(num_f64(&json!(-5)), Some(-5.0));
    }

    #[test]
    fn num_f64_accepts_float() {
        assert_eq!(num_f64(&json!(1.5)), Some(1.5));
        assert_eq!(num_f64(&json!(0.0)), Some(0.0));
    }

    #[test]
    fn num_f64_accepts_string() {
        assert_eq!(num_f64(&json!("100")), Some(100.0));
        assert_eq!(num_f64(&json!("1.5")), Some(1.5));
        assert_eq!(num_f64(&json!("  100  ")), Some(100.0));  // 容 trim
    }

    #[test]
    fn num_f64_rejects_invalid() {
        assert_eq!(num_f64(&json!("abc")), None);
        assert_eq!(num_f64(&json!("")), None);
        assert_eq!(num_f64(&json!(null)), None);
        assert_eq!(num_f64(&json!(true)), None);
        assert_eq!(num_f64(&json!([1, 2])), None);
        assert_eq!(num_f64(&json!({"a": 1})), None);
    }

    #[test]
    fn num_f64_handles_u64_large() {
        // 500000 是 New API 经典 quota 数值（u64）
        assert_eq!(num_f64(&json!(500_000_u64)), Some(500_000.0));
    }
}
