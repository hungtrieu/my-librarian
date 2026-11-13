<#
.SYNOPSIS
  CI tại máy Windows (E0-02): format, clippy, test, build release, đóng gói .msi/NSIS, lưu bản build vào dist\.

.DESCRIPTION
  Không cần dịch vụ ngoài. Tự chọn toolchain: MSVC nếu có Visual Studio Build Tools, ngược lại GNU (MinGW/WinLibs).
  Log của mỗi lần chạy: target\ci\ci-<thời gian>.log. Mã thoát 0 = xanh, khác 0 = có bước hỏng.

.PARAMETER Quick
  Chỉ format + clippy + test (dùng cho git hook pre-push).

.PARAMETER NoBundle
  Bỏ bước đóng gói bộ cài (vẫn build release).

.PARAMETER Toolchain
  auto (mặc định) | msvc | gnu

.PARAMETER Version
  Số phiên bản ghi vào bộ cài (vd. 0.1.3); bỏ trống thì lấy trong crates\app\tauri.conf.json.
  build-windows.cmd tự truyền số này.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\ci.ps1
  powershell -ExecutionPolicy Bypass -File scripts\ci.ps1 -Quick
#>
param(
    [switch]$Quick,
    [switch]$NoBundle,
    [ValidateSet('auto', 'msvc', 'gnu')][string]$Toolchain = 'auto',
    [string]$Version = '',
    [int]$KeepBuilds = 10
)

$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$logDir = Join-Path $root 'target\ci'
New-Item -ItemType Directory -Force $logDir | Out-Null
$log = Join-Path $logDir "ci-$stamp.log"
$results = New-Object System.Collections.Generic.List[object]

function Write-Log([string]$msg) {
    Write-Host $msg
    Add-Content -Path $log -Value $msg -Encoding utf8
}

# Chạy lệnh qua cmd để gộp stdout/stderr vào log mà không bị PowerShell 5.1 biến stderr thành lỗi.
function Invoke-Step([string]$name, [string]$command) {
    Write-Log ""
    Write-Log "==> $name"
    Write-Log "    $command"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $lines = New-Object System.Collections.Generic.List[string]
    & cmd.exe /d /c "$command 2>&1" | ForEach-Object { Write-Host $_; $lines.Add([string]$_) }
    $code = $LASTEXITCODE
    if ($lines.Count) { Add-Content -Path $log -Value $lines -Encoding utf8 }
    $sw.Stop()
    $results.Add([pscustomobject]@{ Step = $name; Ok = ($code -eq 0); Seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1) })
    if ($code -ne 0) {
        Write-Log "!!  $name THẤT BẠI (mã $code)"
        Write-Summary
        Copy-Item $log (Join-Path $logDir 'latest.log') -Force
        exit $code
    }
}

function Write-Summary {
    Write-Log ""
    Write-Log "Tóm tắt ($Toolchain):"
    foreach ($r in $results) {
        Write-Log ("  {0,-4} {1,-28} {2,7}s" -f ($(if ($r.Ok) { 'OK' } else { 'FAIL' })), $r.Step, $r.Seconds)
    }
    Write-Log "Log: $log"
}

# ---------------------------------------------------------------- chọn toolchain
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Log 'Không tìm thấy cargo. Cài Rust: winget install Rustlang.Rustup'; exit 1 }

function Test-Msvc {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }
    $p = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return [bool]$p
}

function Find-MinGw {
    if (Get-Command gcc -ErrorAction SilentlyContinue) { return $null }
    $pkgs = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    $gcc = Get-ChildItem $pkgs -Directory -Filter 'BrechtSanders.WinLibs*' -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'mingw64\bin\gcc.exe' } | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($gcc) { return Split-Path $gcc }
    foreach ($d in 'C:\msys64\ucrt64\bin', 'C:\msys64\mingw64\bin', 'C:\mingw64\bin') { if (Test-Path "$d\gcc.exe") { return $d } }
    throw 'Không tìm thấy gcc (MinGW). Cài: winget install BrechtSanders.WinLibs.POSIX.UCRT'
}

if ($Toolchain -eq 'auto') { $Toolchain = $(if (Test-Msvc) { 'msvc' } else { 'gnu' }) }
if ($Toolchain -eq 'gnu') {
    $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu'
    $mingw = Find-MinGw
    if ($mingw) { $env:Path = "$mingw;$env:Path" }
    $installed = & rustup toolchain list
    if (-not ($installed -match 'stable-x86_64-pc-windows-gnu')) {
        Invoke-Step 'Cài toolchain GNU' 'rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal -c clippy -c rustfmt'
    }
} else {
    $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc'
}

$sha = (& git rev-parse --short HEAD 2>$null)
$dirty = [bool](& git status --porcelain 2>$null)
Write-Log "47 Librarian CI · $stamp · toolchain $Toolchain · commit $sha$(if ($dirty) { ' (có thay đổi chưa commit)' })"
Write-Log (& rustc --version)

# ---------------------------------------------------------------- các bước
Invoke-Step 'Format (cargo fmt --check)' 'cargo fmt --all -- --check'
# crates/ipad chỉ build được cho iOS (dùng API Unix) – build trên Mac bằng build-ipad.command. Ở đây vẫn test
# phần lõi mà app iPad dùng (feature "reader", không có phần chỉ của máy tính).
Invoke-Step 'Clippy (-D warnings)' 'cargo clippy --workspace --exclude librarian-ipad --all-targets --quiet -- -D warnings'
Invoke-Step 'Test' 'cargo test --workspace --exclude librarian-ipad --quiet'
Invoke-Step 'Test phần lõi của iPad' 'cargo test -p librarian-core --no-default-features --features reader --lib --quiet'

if (-not $Quick) {
    Invoke-Step 'Build release' 'cargo build --workspace --exclude librarian-ipad --release --quiet'

    # Phiên bản: tham số -Version, nếu không có thì lấy trong tauri.conf.json. Ghi vào một cấu hình đóng gói tạm
    # trong target\ci (không sửa file nào trong repo).
    if (-not $Version) { $Version = (Get-Content 'crates\app\tauri.conf.json' -Raw | ConvertFrom-Json).version }
    $bundleConf = Get-Content 'crates\app\tauri.bundle.conf.json' -Raw | ConvertFrom-Json
    $bundleConf | Add-Member -NotePropertyName version -NotePropertyValue $Version -Force
    $bundleConfPath = Join-Path $logDir 'bundle.conf.json'
    [IO.File]::WriteAllText($bundleConfPath, ($bundleConf | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))

    $bundled = $false
    if (-not $NoBundle) {
        if (& cargo --list | Select-String -Quiet '^\s+tauri\b') {
            Invoke-Step "Đóng gói $Version (.msi + NSIS)" "cd crates\app && cargo tauri build --bundles msi,nsis --config `"$bundleConfPath`""
            $bundled = $true
        } else {
            Write-Log 'Bỏ qua đóng gói: chưa cài tauri-cli (cargo install tauri-cli --version "^2" --locked)'
        }
    }

    # Lưu bản build: dist\<phiên bản>-<thời gian>-<commit>\
    $out = Join-Path $root "dist\$Version-$stamp-$sha"
    New-Item -ItemType Directory -Force $out | Out-Null
    foreach ($exe in '47-librarian.exe', 'librarian.exe', 'librarian-bg.exe') {
        $p = Join-Path $root "target\release\$exe"
        if (Test-Path $p) { Copy-Item $p $out }
    }
    if ($bundled) {
        Get-ChildItem (Join-Path $root 'target\release\bundle') -Recurse -Include "*_$($Version)_*.msi", "*_$($Version)_*-setup.exe" -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -gt (Get-Date).AddHours(-1) } | Copy-Item -Destination $out
    }
    Write-Log ""
    Write-Log "Bản build: $out"
    Get-ChildItem $out | ForEach-Object { Write-Log ("  {0,-45} {1,10:N0} KB" -f $_.Name, ($_.Length / 1KB)) }
    Set-Content -Path (Join-Path $logDir 'last-dist.txt') -Value $out -Encoding utf8

    # Giữ lại $KeepBuilds bản gần nhất (theo thời gian tạo – tên bắt đầu bằng số phiên bản nên không sắp theo tên).
    Get-ChildItem (Join-Path $root 'dist') -Directory | Where-Object { $_.Name -ne 'ipad' } | Sort-Object CreationTime -Descending | Select-Object -Skip $KeepBuilds |
        ForEach-Object { Remove-Item -Recurse -Force $_.FullName }
}

Write-Summary
Copy-Item $log (Join-Path $logDir 'latest.log') -Force
exit 0
