#!/usr/bin/env python3
"""check-i18n-callsite-keys.py — 校验 t!() / t() 调用点引用的 key 真实存在

为什么需要这个脚本
------------------
已有的 `scripts/validate-i18n-keys.sh` 检查的是 **en.json ↔ zh-CN.json 之间的
对称性**（A 有 B 也要有）。但它挡不住本轮审查发现的这一类问题：

    `t!("login.anysearch.timeout", secs = ...)` 引用的 key **两份 locale 都没
    收录** —— 对称性检查照样是绿的，但 rust-i18n 3 找不到 key 时会返回
    `format!("{locale}.{key}")` 字面量且**不做命名参数替换**，用户看到的是
    一串 "zh-CN.login.anysearch.timeout"。

2026-09-28 全量审查在两个独立域各自命中了同一类 bug（providers/域的
`error.common.api_error` + 登录域的 `login.anysearch.timeout`），说明这不是
偶发而是结构性的检查缺口。本脚本补上 **调用点 → locale 存在性** 这一维。

用法
----
    python3 scripts/check-i18n-callsite-keys.py
    退出码 0 = 全部命中；1 = 有 key 缺失（打印可读诊断）

设计要点
--------
- 先剥注释再扫，否则 docstring / 行尾注释里的示例 key 会误报
- Rust locale 是嵌套 JSON（需展平成点路径），前端 locale 本身就是扁平 key
- Rust `t!("k", name = v)` 与 TS `t("k", {...})` 都能识别
- 动态 key（`t!(format!(...))` / `t(variable)`）无法静态判定，跳过并计入统计
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

RED = "\033[0;31m"
YELLOW = "\033[1;33m"
GREEN = "\033[0;32m"
NC = "\033[0m"

# ── 注释剥离 ────────────────────────────────────────────────────────
# 先扫字符串字面量占位，再剥注释，避免把 URL 里的 "//" 当注释起点。
_STR_RE = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'')


def strip_comments(src: str) -> str:
    """把字符串字面量替换成等长占位后剥掉 // 与 /* */ 注释。"""
    placeholders: list[str] = []

    def _stash(m: re.Match) -> str:
        placeholders.append(m.group(0))
        return f"\x00{len(placeholders) - 1}\x00"

    src = _STR_RE.sub(_stash, src)
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    src = re.sub(r"//[^\n]*", "", src)
    src = re.sub(r"\x00(\d+)\x00", lambda m: placeholders[int(m.group(1))], src)
    return src


# ── 调用点扫描 ───────────────────────────────────────────────────────
# Rust: t!("key", name = value) / t!(r#"key"#, ...)
# 原始字符串前缀是「可选的 r + 若干 #」，注意 `(r#*)` 是错的 —— `r` 在那里是
# 必选（`#*` 只是零或多），会把不带前缀的 t!("k") 全部漏掉（实测 0 命中）。
RUST_CALL_RE = re.compile(r'\bt!\(\s*(r#+)?("(?:\\.|[^"\\])*")')
# TS:   t("key", {...})            t 前面不能紧跟字母/数字（排除别的标识符）
TS_CALL_RE = re.compile(r'(?<![\w.$])t\(\s*("(?:\\.|[^"\\])*")')

# t!(var) / t(someFn()) 之类动态 key —— 记录数量，避免"扫不到"被误当成全绿
RUST_DYNAMIC_RE = re.compile(r'\bt!\(\s*(?!\s*(?:r#+)?")')
TS_DYNAMIC_RE = re.compile(r'(?<![\w.$])t\(\s*(?!["])')


def _scan_tree(
    path: Path,
    glob: str,
    call_re: re.Pattern,
    dynamic_re: re.Pattern,
    key_group: int,
) -> tuple[set[str], int, list[str]]:
    """扫目录树下所有匹配文件，返回 (用到的 key 集合, 动态 key 次数, 错误列表)。"""
    keys: set[str] = set()
    dynamic = 0
    errs: list[str] = []
    if not path.exists():
        return keys, 0, [f"扫描目标不存在: {path}"]
    for f in sorted(path.rglob(glob)):
        try:
            src = strip_comments(f.read_text(encoding="utf-8"))
        except Exception as e:  # pragma: no cover
            errs.append(f"读取失败 {f}: {e}")
            continue
        for m in call_re.finditer(src):
            try:
                keys.add(json.loads(m.group(key_group)))
            except json.JSONDecodeError:
                continue
        # 动态 key：调用了 t!()/t() 但第一个实参不是字符串字面量
        dynamic += len(dynamic_re.findall(src))
    return keys, dynamic, errs


# ── locale 加载 ──────────────────────────────────────────────────────
def flatten(obj, prefix: str = "") -> set[str]:
    out: set[str] = set()
    if isinstance(obj, dict):
        for k, v in obj.items():
            out |= flatten(v, f"{prefix}.{k}" if prefix else k)
    else:
        out.add(prefix)
    return out


def load_locale(p: Path, nested: bool) -> set[str]:
    data = json.loads(p.read_text(encoding="utf-8"))
    return flatten(data) if nested else set(data.keys())


def main() -> int:
    failures = 0

    rust_locales = {
        "en": ROOT / "src-tauri/locales/en.json",
        "zh-CN": ROOT / "src-tauri/locales/zh-CN.json",
    }
    ts_locales = {
        "en": ROOT / "src/i18n/en.json",
        "zh-CN": ROOT / "src/i18n/zh-CN.json",
    }

    # ── 后端：Rust src/ 下的 t!() ──
    rust_keys, rust_dyn, errs = _scan_tree(
        ROOT / "src-tauri/src", "*.rs", RUST_CALL_RE, RUST_DYNAMIC_RE, key_group=2
    )
    for e in errs:
        print(f"{RED}{e}{NC}")
        failures += 1
    for loc, p in rust_locales.items():
        have = load_locale(p, nested=True)
        missing = sorted(rust_keys - have)
        if missing:
            failures += 1
            print(f"{RED}[backend/{loc}] src-tauri/locales/{loc}.json 缺少 {len(missing)} 个被 t!() 引用的 key:{NC}")
            for k in missing:
                print(f"    {k}")
        else:
            print(f"{GREEN}[backend/{loc}] {len(rust_keys)} 个 t!() key 全部存在{NC}")

    # ── 前端：src/ 下的 t() ──
    ts_keys, ts_dyn, errs = _scan_tree(
        ROOT / "src", "*.ts", TS_CALL_RE, TS_DYNAMIC_RE, key_group=1
    )
    for e in errs:
        print(f"{RED}{e}{NC}")
        failures += 1
    for loc, p in ts_locales.items():
        # src/i18n/*.json 也是嵌套结构（credentials / settings / error …），
        # 与后端 locale 一样需要展平成点路径，不是取顶层 key。
        have = load_locale(p, nested=True)
        missing = sorted(ts_keys - have)
        if missing:
            failures += 1
            print(f"{RED}[frontend/{loc}] src/i18n/{loc}.json 缺少 {len(missing)} 个被 t() 引用的 key:{NC}")
            for k in missing:
                print(f"    {k}")
        else:
            print(f"{GREEN}[frontend/{loc}] {len(ts_keys)} 个 t() key 全部存在{NC}")

    dyn_total = rust_dyn + ts_dyn
    if dyn_total:
        print(
            f"{YELLOW}提示: {dyn_total} 处 t!()/t() 使用动态 key（变量或函数调用），"
            f"静态检查覆盖不到 —— 若新增了动态 key，请手工确认其取值在 locale 里存在。{NC}"
        )

    if failures:
        print(f"\n{RED}{failures} 处 key 缺失。rust-i18n 找不到 key 时会返回字面量 "
              f"'<locale>.<key>' 且不做参数替换，用户直接看到 key 串。{NC}")
        return 1
    print(f"\n{GREEN}全部 t!()/t() 调用点的 key 均存在于对应 locale。{NC}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
