#!/usr/bin/env bash
# validate-i18n-keys.sh — 交叉验证前后端 locale JSON 的 key 完整性
#
# 用法: bash scripts/validate-i18n-keys.sh
# 非零退出 = key 不一致
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

failures=0

extract_keys() {
  jq -r 'paths(scalars) | map(if type == "number" then "\(.)" else . end) | join(".")' "$1" | sort
}

check_pair() {
  local en="$1"
  local zh="$2"
  local label="$3"

  local en_keys
  local zh_keys
  en_keys=$(mktemp)
  zh_keys=$(mktemp)
  trap "rm -f $en_keys $zh_keys" EXIT

  extract_keys "$en" > "$en_keys"
  extract_keys "$zh" > "$zh_keys"

  local missing_in_zh
  local missing_in_en
  missing_in_zh=$(comm -23 "$en_keys" "$zh_keys")
  missing_in_en=$(comm -13 "$en_keys" "$zh_keys")

  if [ -n "$missing_in_zh" ]; then
    echo -e "${RED}[${label}] en.json keys missing in zh-CN.json:${NC}"
    echo "$missing_in_zh"
    failures=$((failures + 1))
  fi
  if [ -n "$missing_in_en" ]; then
    echo -e "${RED}[${label}] zh-CN.json keys missing in en.json:${NC}"
    echo "$missing_in_en"
    failures=$((failures + 1))
  fi
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# 后端 locale
check_pair \
  "$PROJECT_DIR/src-tauri/locales/en.json" \
  "$PROJECT_DIR/src-tauri/locales/zh-CN.json" \
  "backend"

# 前端 locale
check_pair \
  "$PROJECT_DIR/src/i18n/en.json" \
  "$PROJECT_DIR/src/i18n/zh-CN.json" \
  "frontend"

if [ "$failures" -eq 0 ]; then
  echo -e "${GREEN}All i18n key sets consistent.${NC}"
  exit 0
else
  echo -e "${RED}${failures} key mismatch(es) found. Fix before merging.${NC}"
  exit 1
fi
