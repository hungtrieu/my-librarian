<#
.SYNOPSIS
  Build APK Android (debug) trên Windows chỉ khi mã nguồn thay đổi. Gọi qua build-android.cmd ở thư mục gốc.

.DESCRIPTION
  Tương đương build-android.command (Mac). Tự tìm JDK / Android SDK / NDK ở chỗ cài mặc định.
  Có điện thoại cắm cáp (adb) thì cài luôn. APK lưu ở dist\android\<phiên bản>-…\.
  -Force: build lại dù không đổi gì.  -Version 0.1.4: ép số phiên bản (khi điện thoại đang cài bản mới hơn: INSTALL_FAILED_VERSION_DOWNGRADE).  LIBRARIAN_ANDROID_TARGETS="aarch64 armv7": kiến trúc CPU (mặc định cả hai).
  LIBRARIAN_ANDROID_DEVICE=<serial>: chọn máy khi có nhiều thiết bị.
#>
param([switch]$Force, [string]$Version)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Say([string]$m, [string]$c = 'Cyan') { Write-Host "`n$m" -ForegroundColor $c }
function Fail([string]$m) { Write-Host "`n$m" -ForegroundColor Red; exit 1 }

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
foreach ($c in 'node', 'cargo', 'cargo-tauri') { if (-not (Get-Command $c -ErrorAction SilentlyContinue)) { Fail "Thiếu $c (tauri-cli: cargo install tauri-cli --version '^2' --locked)." } }

# --- Android SDK / NDK / JDK
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
if (-not (Test-Path (Join-Path $env:ANDROID_HOME 'platforms'))) { Fail "Không thấy Android SDK ở $env:ANDROID_HOME (xem docs\ANDROID.md)." }
if (-not $env:NDK_HOME -or -not (Test-Path $env:NDK_HOME)) {
    $ndk = Get-ChildItem (Join-Path $env:ANDROID_HOME 'ndk') -Directory -ErrorAction SilentlyContinue | Sort-Object { [version]($_.Name -replace '[^\d.].*$', '') } | Select-Object -Last 1
    if (-not $ndk) { Fail 'Thiếu Android NDK: sdkmanager "ndk;27.1.12297006"' }
    $env:NDK_HOME = $ndk.FullName
}
$env:ANDROID_NDK_HOME = $env:NDK_HOME
if (-not $env:JAVA_HOME -or -not (Test-Path (Join-Path $env:JAVA_HOME 'bin\java.exe'))) {
    $cands = @('C:\Program Files\Eclipse Adoptium', 'C:\Program Files\Java', 'C:\Program Files\Android\Android Studio\jbr') |
        Where-Object { Test-Path $_ } | ForEach-Object { if (Test-Path (Join-Path $_ 'bin\java.exe')) { Get-Item $_ } else { Get-ChildItem $_ -Directory } } |
        Where-Object { Test-Path (Join-Path $_.FullName 'bin\java.exe') } | Sort-Object Name | Select-Object -Last 1
    if (-not $cands) { Fail 'Thiếu JDK 17+: winget install EclipseAdoptium.Temurin.17.JDK' }
    $env:JAVA_HOME = $cands.FullName
}
$env:Path = "$env:JAVA_HOME\bin;$env:Path"
$adb = Join-Path $env:ANDROID_HOME 'platform-tools\adb.exe'

$targets = if ($env:LIBRARIAN_ANDROID_TARGETS) { $env:LIBRARIAN_ANDROID_TARGETS -split '\s+' } else { @('aarch64', 'armv7') }
$triples = @{ aarch64 = 'aarch64-linux-android'; armv7 = 'armv7-linux-androideabi'; x86_64 = 'x86_64-linux-android'; i686 = 'i686-linux-android' }
foreach ($t in $targets) {
    if (-not $triples.ContainsKey($t)) { Fail "Không biết kiến trúc '$t' (aarch64, armv7, x86_64, i686)." }
    & rustup target add $triples[$t] | Out-Null
}

function Get-Serial {
    if (-not (Test-Path $adb)) { return $null }
    if ($env:LIBRARIAN_ANDROID_DEVICE) { return $env:LIBRARIAN_ANDROID_DEVICE }
    $line = & $adb devices | Select-Object -Skip 1 | Where-Object { $_ -match '\sdevice$' } | Select-Object -First 1
    if ($line) { return ($line -replace '\s+device$', '') }
    return $null
}
# Phiên bản app đang cài trên điện thoại (null nếu chưa cài / không có máy).
function Get-InstalledVersion {
    $serial = Get-Serial
    if (-not $serial) { return $null }
    $out = & $adb -s $serial shell dumpsys package vn.hungtrieu.mylibrarian.android 2>$null | Select-String 'versionName=(\d+\.\d+\.\d+)' | Select-Object -First 1
    if ($out) { return $out.Matches[0].Groups[1].Value }
    return $null
}
function Cmp-Ver([string]$a, [string]$b) { return ([version]$a).CompareTo([version]$b) }
function Next-Ver([string]$v) { $p = $v -split '\.'; return "$($p[0]).$($p[1]).$([int]$p[2] + 1)" }

function Install-Apk([string]$apk) {
    $serial = Get-Serial
    if (-not $serial) { Write-Host 'Không thấy điện thoại nào qua adb – chưa cài. Chép APK sang điện thoại, hoặc cắm cáp (bật Gỡ lỗi USB) rồi chạy lại.'; return }
    Say "Cài lên $serial"
    $prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue' # adb ghi lỗi ra stderr: không để PowerShell coi là ngoại lệ
    $log = & $adb -s $serial install -r $apk 2>&1 | Out-String
    $code = $LASTEXITCODE; $ErrorActionPreference = $prev
    Write-Host $log
    if ($code -eq 0 -and $log -match 'Success') { Say "Đã cài lên $serial." 'Green'; return }
    if ($log -match 'USER_RESTRICTED|INSTALL_FAILED_USER_RESTRICTED') {
        # Máy không cho cài qua USB (vd. Redmi không có SIM không bật được "Cài đặt qua USB"): đẩy APK vào Download, mở file trên máy để cài.
        $name = Split-Path $apk -Leaf
        $ErrorActionPreference = 'Continue'
        & $adb -s $serial push $apk "/sdcard/Download/$name" 2>&1 | Out-Null
        $pushed = $LASTEXITCODE
        # File chép qua adb chưa được Android lập chỉ mục → app Tệp không thấy: yêu cầu quét file.
        if ($pushed -eq 0) { & $adb -s $serial shell content call --uri content://media/external/file --method scan_file --arg "/sdcard/Download/$name" 2>&1 | Out-Null }
        $ErrorActionPreference = $prev
        if ($pushed -eq 0) {
            Say "Máy chặn cài qua USB. Đã chép APK vào điện thoại: Download/$name" 'Yellow'
            Write-Host 'Mở app Tệp (Files) → Download → bấm file APK để cài (cho phép "cài ứng dụng không rõ nguồn gốc" nếu được hỏi).'
        } else { Write-Host "Máy chặn cài qua USB. Chép tay file này sang điện thoại rồi mở để cài: $apk" }
        return
    }
    if ($log -match 'INSTALL_FAILED_UPDATE_INCOMPATIBLE') { Fail 'Chữ ký không khớp bản đang cài (bản cũ ký bằng khoá khác crates\ipad\android\debug.keystore): gỡ app cũ trên điện thoại rồi cài lại một lần (mất sách đã tải). Từ đó mọi máy build dùng chung khoá này.' }
    Fail 'CÀI HỎNG – xem docs\ANDROID.md.'
}

$checkArgs = @('scripts\release-state.mjs', 'check', 'android')
if ($Force -or $Version) { $checkArgs += '--force' }
$s = (& node @checkArgs) | ConvertFrom-Json
if (-not $s) { Fail 'Không kiểm tra được thay đổi (git?).' }
if ($Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail '-Version phải dạng 0.1.4' }
    $s.version = $Version
} else {
    # Điện thoại đang cài bản mới hơn bản sắp cài (vd. build trước đó từ máy khác) → Android từ chối "hạ cấp": build bản cao hơn.
    $inst = Get-InstalledVersion
    $planned = if ($s.action -eq 'none') { $s.lastVersion } else { $s.version }
    if ($inst -and (Cmp-Ver $inst $planned) -ge 0) {
        Write-Host "Điện thoại đang cài bản $inst (>= $planned) – build bản $(Next-Ver $inst)." -ForegroundColor Yellow
        $s = (& node scripts\release-state.mjs check android --force) | ConvertFrom-Json
        $s.version = Next-Ver $inst
    }
}
if ($s.action -eq 'none') {
    Say "Không có thay đổi mã nguồn kể từ bản $($s.lastVersion) – không build gì. Dùng -Force để build lại." 'Green'
    Write-Host "Bản mới nhất: $($s.lastFolder)"
    $old = Get-ChildItem $s.lastFolder -Filter *.apk -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($old) { Install-Apk $old.FullName }
    exit 0
}

Say "Build bản Android $($s.version) – lý do: $($s.reason)"
Say 'Test phần đọc sách dùng chung'
& cargo test -p librarian-core --no-default-features --features reader --lib --quiet
if ($LASTEXITCODE -ne 0) { Fail 'TEST HỎNG – không build, phiên bản giữ nguyên.' }

Say 'Ghép giao diện'
& node scripts\build-ipad-ui.mjs
if ($LASTEXITCODE -ne 0) { Fail 'Ghép giao diện hỏng.' }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("librarian-android-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$verFile = Join-Path $tmp 'version.json'
[IO.File]::WriteAllText($verFile, "{`"version`":`"$($s.version)`"}`n")

Push-Location crates\ipad
try {
    if (-not (Test-Path gen\android)) {
        Say 'Khởi tạo dự án Android (lần đầu)'
        & cargo tauri android init --ci
        if ($LASTEXITCODE -ne 0) { Fail 'Không khởi tạo được dự án Android.' }
    }
    & node (Join-Path $root 'scripts\android-signing.mjs')
    if ($LASTEXITCODE -ne 0) { Fail 'Không cài được khoá ký chung.' }
    $env:CARGO_PROFILE_DEV_OPT_LEVEL = '2'; $env:CARGO_PROFILE_DEV_DEBUG = '0'
    Say "Build APK (kiến trúc: $($targets -join ' ')) – lần đầu mất 10–20 phút"
    $start = Get-Date
    & cargo tauri android build --debug --apk --ci --target @targets --config $verFile
    if ($LASTEXITCODE -ne 0) { Fail 'BUILD HỎNG – phiên bản giữ nguyên.' }
    $apkFile = Get-ChildItem gen\android\app\build\outputs\apk -Recurse -Filter *.apk | Where-Object { $_.LastWriteTime -gt $start } | Select-Object -First 1
    if (-not $apkFile) { Fail 'Build không ra file APK mới.' }
} finally { Pop-Location }

$out = Join-Path $root ("dist\android\{0}-{1}" -f $s.version, (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $out | Out-Null
$dest = Join-Path $out "My-Librarian-$($s.version).apk"
Copy-Item $apkFile.FullName $dest
& node scripts\release-state.mjs record android $s.version $out $s.fingerprint
Say "XONG – bản Android $($s.version): $dest" 'Green'
Install-Apk $dest
