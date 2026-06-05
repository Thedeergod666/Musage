# 通过 gh-proxy.com 镜像下载 WinLibs MinGW
$ErrorActionPreference = "Stop"
$proxy = "https://gh-proxy.com/"
$url = "https://github.com/brechtsanders/winlibs_mingw/releases/download/16.1.0posix-14.0.0-ucrt-r2/winlibs-x86_64-posix-seh-gcc-16.1.0-mingw-w64ucrt-14.0.0-r2.zip"
$dst = "C:\tools\winlibs.zip"

if (-not (Test-Path "C:\tools")) { New-Item -ItemType Directory -Path "C:\tools" | Out-Null }
if (Test-Path $dst) { Remove-Item $dst -Force }

Write-Host "[1/3] downloading via $proxy..."
Write-Host "      url: $url"
Write-Host "      dst: $dst"

# 进度回调
$wc = New-Object System.Net.WebClient
$wc.Headers.Add("User-Agent", "PowerShell")
$fullUrl = $proxy + $url

Register-ObjectEvent -InputObject $wc -EventName DownloadProgressChanged -Action {
    $p = $EventArgs
    Write-Progress -Activity "Downloading WinLibs" -Status "$([math]::Round($p.ProgressPercentage,1))%" -PercentComplete $p.ProgressPercentage
} | Out-Null

$wc.DownloadFile($fullUrl, $dst)
Unregister-Event -SourceIdentifier "System.Net.WebClient" -ErrorAction SilentlyContinue

Write-Host "[2/3] downloaded: $((Get-Item $dst).Length / 1MB) MB"
Write-Host "[3/3] extracting to C:\tools\winlibs..."

Expand-Archive -Path $dst -DestinationPath "C:\tools" -Force
$extractedRoot = Get-ChildItem "C:\tools" -Directory | Where-Object { $_.Name -like "mingw64" } | Select-Object -First 1
if ($extractedRoot) {
    Write-Host ""
    Write-Host "[ok] WinLibs extracted to: $($extractedRoot.FullName)"
    Write-Host "[ok] bin path: $($extractedRoot.FullName)\bin"
    Write-Host ""
    Write-Host "Add to PATH (admin PowerShell):"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$($extractedRoot.FullName)\bin;`" + [Environment]::GetEnvironmentVariable('Path'), 'Machine')"
} else {
    Write-Host "[warn] Could not find mingw64 subfolder. Check C:\tools manually."
}
