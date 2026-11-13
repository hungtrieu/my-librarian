<#
.SYNOPSIS
  Build bộ cài Windows chỉ khi mã nguồn thay đổi. Gọi qua build-windows.cmd ở thư mục gốc.

.DESCRIPTION
  - Mã nguồn không đổi kể từ bản build trước: không build gì, chỉ báo và mở thư mục bản mới nhất.
  - Có thay đổi: tự tăng phiên bản (0.1.3 → 0.1.4), chạy kiểm tra + test, build, đóng gói .msi/NSIS vào dist\.
    Hỏng ở bước nào thì dừng, không tăng phiên bản.
  Thêm -Force để build lại dù không có thay đổi.
#>
param([switch]$Force)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }
function Show-Folder([string]$path) {
    if ($path -and (Test-Path $path)) { Start-Process explorer.exe $path }
}

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Say 'Thiếu Node.js (cần để kiểm tra thay đổi). Cài: winget install OpenJS.NodeJS.LTS' Red
    exit 1
}

# Tự sửa định dạng code trước (lỗi định dạng sửa được, không cần dừng build vì nó).
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
& cargo fmt --all 2>$null | Out-Null

$checkArgs = @('scripts\release-state.mjs', 'check', 'windows')
if ($Force) { $checkArgs += '--force' }
$s = (& node @checkArgs) | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $s) { Say 'Không kiểm tra được thay đổi (git có cài chưa?).' Red; exit 1 }

if ($s.action -eq 'none') {
    Say ''
    Say "Không có thay đổi mã nguồn kể từ bản $($s.lastVersion) (build lúc $(([datetime]$s.lastBuiltAt).ToString('dd/MM/yyyy HH:mm'))) – không build gì." Green
    Say "Bộ cài mới nhất: $($s.lastFolder)" Green
    Show-Folder $s.lastFolder
    exit 0
}

Say ''
Say "Build bản Windows $($s.version) – lý do: $($s.reason)." Cyan
Say 'Các bước: kiểm tra định dạng → kiểm lỗi (clippy) → test → build → đóng gói. Mất khoảng 5–10 phút.' Cyan

$lastDist = Join-Path $root 'target\ci\last-dist.txt'
Remove-Item $lastDist -ErrorAction SilentlyContinue
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'ci.ps1') -Version $s.version
$code = $LASTEXITCODE
$out = if (Test-Path $lastDist) { (Get-Content $lastDist -Raw).Trim() } else { '' }
$setup = if ($out) { Get-ChildItem $out -Filter '*-setup.exe' -ErrorAction SilentlyContinue | Select-Object -First 1 } else { $null }

if ($code -ne 0 -or -not $setup) {
    Say ''
    if ($code -ne 0) { Say "BUILD HỎNG – không tạo bộ cài, không tăng phiên bản$(if ($s.lastVersion) { " (bản mới nhất vẫn là $($s.lastVersion))" })." Red }
    else { Say 'Build xong nhưng không có bộ cài (thiếu tauri-cli? cargo install tauri-cli --version "^2" --locked).' Red }
    Say "Xem lỗi trong: $(Join-Path $root 'target\ci\latest.log')" Red
    exit 1
}

& node scripts\release-state.mjs record windows $s.version $out $s.fingerprint
Say ''
Say "XONG – bản Windows $($s.version)" Green
Say "Bộ cài: $($setup.FullName)" Green
Say 'Cài: đóng My Librarian rồi chạy file -setup.exe ở trên (cài đè, giữ nguyên dữ liệu thư viện).' Green
Show-Folder $out
exit 0
