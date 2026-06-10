## 安装说明

### macOS
下载 `Musage_*_aarch64.dmg`（Apple Silicon）或 `Musage_*_x64.dmg`（Intel），双击挂载后拖入 Applications。

> 首次打开如遇「未识别开发者」，右键 → 打开 → 打开。

### Windows
下载 `Musage_*_x64-setup.exe`（推荐，~5 MB）走 NSIS 安装器，或 `.msi` 走 WiX。

> 首次运行如遇 SmartScreen，点 **更多信息** → **仍要运行**。

## 校验

每个产物对应 `.sha256` 文件：
```bash
shasum -a 256 -c Musage_*.sha256
```

## 已知问题

参见 [Issues](https://github.com/${{ github.repository }}/issues)。
