#!/usr/bin/env bash
# Musage dev environment loader (Git Bash)
# Use:  source dev-env.sh
export PATH="/d/Develop/node20:/c/Users/33348/.cargo/bin:$PATH"
export npm_config_registry=https://registry.npmmirror.com
export CARGO_HOME="$USERPROFILE/.cargo"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
echo "[dev-env] PATH loaded"
which node pnpm cargo rustc
