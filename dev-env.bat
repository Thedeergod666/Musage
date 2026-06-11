@echo off
REM Musage dev environment loader
REM Use:  cmd /c "dev-env.bat && pnpm tauri:dev"
REM
REM Loads Node 20 (D:\Develop\node20), cargo (rustup), and optional
REM mingw64 (D:\Develop\mingw64) into PATH for the current cmd session.
REM Sets China-friendly mirrors for npm / crates.io to speed up downloads.

setlocal

set "PATH=D:\Develop\node20;%PATH%"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "D:\Develop\mingw64\bin\dlltool.exe" set "PATH=D:\Develop\mingw64\bin;%PATH%"

set "npm_config_registry=https://registry.npmmirror.com"
set "CARGO_HOME=%USERPROFILE%\.cargo"
set "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
set "RUSTUP_DIST_SERVER=https://rsproxy.cn"
set "RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup"

echo [dev-env] PATH loaded
where node
where pnpm
where cargo
where dlltool

endlocal & (
  set "PATH=D:\Develop\node20;%USERPROFILE%\.cargo\bin;%PATH%"
  set "npm_config_registry=https://registry.npmmirror.com"
  set "CARGO_HOME=%USERPROFILE%\.cargo"
  set "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
  set "RUSTUP_DIST_SERVER=https://rsproxy.cn"
  set "RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup"
)
