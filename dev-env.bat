@echo off
REM Musage dev environment loader
REM Use:  cmd.exe /c "dev-env.bat && tauri dev"
REM  Or:  cmd /c "dev-env.bat && cargo build"
setlocal

set "PATH=D:\Develop\node20;D:\Users\33348\.cargo\bin;%PATH%"

REM npm/pnpm to use Chinese mirror for speed
set "npm_config_registry=https://registry.npmmirror.com"

REM Rust: use rsproxy.cn mirror for crates
set "CARGO_HOME=%USERPROFILE%\.cargo"
set "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
set "RUSTUP_DIST_SERVER=https://rsproxy.cn"
set "RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup"

echo [dev-env] PATH loaded
echo [dev-env] node:  %SystemDrive%%SystemRoot%\..\..\..\D\Develop\node20\node.exe
where node 2>nul
where pnpm 2>nul
where cargo 2>nul
where rustc 2>nul

endlocal & (
  set "PATH=D:\Develop\node20;D:\Users\33348\.cargo\bin;%PATH%"
  set "npm_config_registry=https://registry.npmmirror.com"
  set "CARGO_HOME=%USERPROFILE%\.cargo"
  set "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
  set "RUSTUP_DIST_SERVER=https://rsproxy.cn"
  set "RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup"
)
