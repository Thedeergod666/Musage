#!/usr/bin/env bash
# check-i18n-callsite-keys.sh — 跨平台包装：跑 check-i18n-callsite-keys.py
#
# CI 的 frontend job 是 ubuntu / macos / windows 三平台矩阵，Windows runner
# 上 `python` 与 `python3` 哪个存在不确定。本包装按 python3 → python 顺序
# 探测，找不到就明确报错（而不是让 CI 抛一个看不懂的 "command not found"）。
#
# 用法: bash scripts/check-i18n-callsite-keys.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

PY=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1; then
    PY="$cand"
    break
  fi
done

if [ -z "$PY" ]; then
  echo "❌ 找不到 python3 / python，无法运行 i18n key 存在性检查。"
  echo "   本脚本只需标准库（json / re / pathlib），任意 Python 3.7+ 均可。"
  exit 1
fi

exec "$PY" "$SCRIPT_DIR/check-i18n-callsite-keys.py"
