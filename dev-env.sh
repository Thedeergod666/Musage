#!/usr/bin/env bash
# Musage dev environment loader (Git Bash)
# Use:  source dev-env.sh
#
# mingw64 (WinLibs) 一键装：
#   winget install BrechtSanders.WinLibs.POSIX.UCRT --location "D:\Develop\mingw64"
# 装完后 dlltool 就会在 PATH 里。
[ -d /d/Develop/mingw64/bin ] && MINGW_BIN="/d/Develop/mingw64/bin" || MINGW_BIN=""
export PATH="/d/Develop/node20:/c/Users/33348/.cargo/bin:$MINGW_BIN:$PATH"
export npm_config_registry=https://registry.npmmirror.com
export CARGO_HOME="$USERPROFILE/.cargo"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
echo "[dev-env] PATH loaded"
which node pnpm cargo rustc dlltool
