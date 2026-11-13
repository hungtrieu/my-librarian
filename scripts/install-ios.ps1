<#
.SYNOPSIS
  Cài file .ipa 47 Librarian (build ở Mac bằng build-ipad.command --ipa) lên iPad/iPhone cắm cáp vào máy Windows.
  Gọi qua install-ios.cmd ở thư mục gốc (bấm đúp, hoặc kéo-thả file .ipa lên install-ios.cmd).

.DESCRIPTION
  Không ký lại: .ipa đã được Mac ký bằng Personal Team (Apple ID miễn phí) cho các máy đã đăng ký vào team, nên chỉ cần
  chép lên máy. Dữ liệu trong app giữ nguyên (cùng Bundle ID, cùng team).
  Cần app "Apple Devices" (Microsoft Store) hoặc iTunes để Windows nói chuyện được với iPad/iPhone qua USB.
  Công cụ cài: go-ios (MIT, github.com/danielpaulus/go-ios) – lần đầu script tự tải về %LOCALAPPDATA%\47-librarian\go-ios.

  -Ipa <file>    file .ipa cần cài. Bỏ trống: lấy file 47-Librarian-*.ipa mới nhất trong: thư mục ghi ở file .ipa-share
                 (thư mục gốc dự án), $env:LIBRARIAN_IPA_DIR, dist (cả thư mục con), Downloads.
  -Device <UDID> chọn máy khi cắm nhiều iPad/iPhone.
#>
param([Parameter(Position = 0)][string]$Ipa, [string]$Device)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Say([string]$m, [string]$c = 'Cyan') { Write-Host "`n$m" -ForegroundColor $c }
function Fail([string]$m) { Write-Host "`n$m" -ForegroundColor Red; exit 1 }

$GoIosVersion = '1.3.2'

# Chạy ios.exe. PowerShell 5.1 biến mỗi dòng stderr của lệnh ngoài thành lỗi, và với ErrorActionPreference=Stop thì
# dừng luôn script (go-ios in cảnh báo "agent is not running" ra stderr). Nên hạ về Continue trong hàm này.
# -All: trả cả stderr (để in khi lỗi); mặc định chỉ stdout. Mã thoát để ở $script:IosExit.
function Invoke-Ios([string[]]$IosArgs, [switch]$All) {
    $ErrorActionPreference = 'Continue'
    $lines = & $ios @IosArgs 2>&1 | ForEach-Object {
        if ($_ -is [Management.Automation.ErrorRecord]) { if ($All) { $_.Exception.Message } } else { "$_" }
    }
    $script:IosExit = $LASTEXITCODE
    ($lines | Out-String)
}

# --- 1. Driver Apple (usbmuxd của Apple Devices / iTunes)
$svc = Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'Apple Mobile Device*' -or $_.DisplayName -like 'Apple Mobile Device*' } | Select-Object -First 1
# usbmuxd nghe ở cổng 27015 – dùng làm dấu hiệu dự phòng nếu tên dịch vụ khác.
$muxUp = [bool](Get-NetTCPConnection -LocalPort 27015 -State Listen -ErrorAction SilentlyContinue)
if (-not $svc -and -not $muxUp) {
    $hint = ''
    if (Get-AppxPackage 'AppleInc.iCloud' -ErrorAction SilentlyContinue) { $hint = ' (Máy có iCloud nhưng iCloud không kèm driver USB cho iPad/iPhone.)' }
    try { Start-Process 'ms-windows-store://pdp/?ProductId=9NP83LWLPZ9K' } catch {}
    Fail ("Chưa có driver Apple.$hint Cài app 'Apple Devices' từ Microsoft Store (đã mở trang cài; hoặc cài iTunes)," +
        " mở app một lần, cắm iPad/iPhone, bấm 'Tin cậy' trên máy rồi chạy lại.")
}
if ($svc -and $svc.Status -ne 'Running') {
    try { Start-Service $svc.Name } catch { Fail "Dịch vụ '$($svc.DisplayName)' đang tắt. Mở app Apple Devices một lần (hoặc khởi động lại máy) rồi chạy lại." }
}

# --- 2. go-ios
$toolDir = Join-Path $env:LOCALAPPDATA "47-librarian\go-ios\$GoIosVersion"
$ios = Join-Path $toolDir 'ios.exe'
if (-not (Test-Path $ios)) {
    Say "Tải công cụ cài go-ios $GoIosVersion (một lần, ~150 MB gói npm)"
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) { Fail 'Thiếu Node.js/npm (cài từ https://nodejs.org) – cần để tải go-ios.' }
    $dl = Join-Path $env:TEMP "go-ios-$GoIosVersion-$PID"
    New-Item -ItemType Directory -Force $dl | Out-Null
    # npm pack chỉ tải gói, không chạy script cài đặt của gói.
    Push-Location $dl
    try { npm pack "go-ios@$GoIosVersion" --silent | Out-Null } finally { Pop-Location }
    $tgz = Get-ChildItem $dl -Filter 'go-ios-*.tgz' | Select-Object -First 1
    if (-not $tgz) { Fail 'Không tải được go-ios (npm pack lỗi – kiểm mạng).' }
    tar -xzf $tgz.FullName -C $dl 'package/dist/go-ios-windows-amd64_windows_amd64/ios.exe'
    if ($LASTEXITCODE -ne 0) { Fail 'Không giải nén được go-ios.' }
    New-Item -ItemType Directory -Force $toolDir | Out-Null
    Move-Item (Join-Path $dl 'package\dist\go-ios-windows-amd64_windows_amd64\ios.exe') $ios -Force
    Remove-Item $dl -Recurse -Force -ErrorAction SilentlyContinue
}

# --- 3. Chọn file .ipa
if (-not $Ipa) {
    $dirs = @()
    if (Test-Path '.ipa-share') { $dirs += (Get-Content '.ipa-share' -TotalCount 1).Trim() }
    if ($env:LIBRARIAN_IPA_DIR) { $dirs += $env:LIBRARIAN_IPA_DIR }
    $dirs += (Join-Path $root 'dist'), (Join-Path $env:USERPROFILE 'Downloads')
    $found = foreach ($d in $dirs) {
        if ($d -and (Test-Path $d)) { Get-ChildItem $d -Filter '47-Librarian-*.ipa' -Recurse -Depth 2 -File -ErrorAction SilentlyContinue }
    }
    $pick = $found | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $pick) {
        Fail ("Không thấy file 47-Librarian-*.ipa. Kéo-thả file .ipa lên install-ios.cmd, hoặc ghi đường dẫn thư mục chứa" +
            " .ipa (vd. thư mục Google Drive/OneDrive) vào file .ipa-share ở thư mục dự án. Đã tìm trong:`n  " + ($dirs -join "`n  "))
    }
    $Ipa = $pick.FullName
}
if (-not (Test-Path $Ipa)) { Fail "Không có file: $Ipa" }
$Ipa = (Resolve-Path $Ipa).Path
Say "File: $Ipa  ($([math]::Round((Get-Item $Ipa).Length / 1MB)) MB, tải về lúc $((Get-Item $Ipa).LastWriteTime))"

# --- 4. Đọc provisioning profile trong .ipa: hạn ký và các máy được phép cài
Add-Type -AssemblyName System.IO.Compression.FileSystem
$expires = $null; $udids = @()
$zip = [IO.Compression.ZipFile]::OpenRead($Ipa)
try {
    $entry = $zip.Entries | Where-Object { $_.FullName -match '^Payload/[^/]+\.app/embedded\.mobileprovision$' } | Select-Object -First 1
    if (-not $entry) { Fail 'File .ipa không có embedded.mobileprovision – không phải bản đã ký cho máy cụ thể.' }
    $ms = New-Object IO.MemoryStream
    $s = $entry.Open(); try { $s.CopyTo($ms) } finally { $s.Dispose() }
    # Profile là plist XML bọc trong chữ ký CMS – đọc thẳng phần chữ.
    $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
    $m = [regex]::Match($text, '<key>ExpirationDate</key>\s*<date>([^<]+)</date>')
    if ($m.Success) { $expires = [datetime]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal -bor [Globalization.DateTimeStyles]::AssumeUniversal) }
    $m = [regex]::Match($text, '<key>ProvisionedDevices</key>\s*<array>(.*?)</array>', 'Singleline')
    if ($m.Success) { $udids = [regex]::Matches($m.Groups[1].Value, '<string>([^<]+)</string>') | ForEach-Object { $_.Groups[1].Value.ToUpper() } }
} finally { $zip.Dispose() }
if ($expires) {
    $left = ($expires - [datetime]::UtcNow).TotalDays
    if ($left -le 0) { Fail "Bản này đã hết hạn ký ($($expires.ToLocalTime())). Build lại trên Mac: ./build-ipad.command --ipa" }
    Write-Host ("Hạn ký: {0:dd/MM/yyyy HH:mm} (còn {1:N1} ngày). App ngừng mở được sau thời điểm này." -f $expires.ToLocalTime(), $left)
    if ($left -lt 2) { Write-Host 'Sắp hết hạn – nên build bản mới trên Mac.' -ForegroundColor Yellow }
}

# --- 5. Chọn máy
$listJson = Invoke-Ios @('list')
$list = @()
try {
    $o = $listJson | ConvertFrom-Json
    foreach ($d in @($o.deviceList)) { if ($d -is [string]) { $list += $d } elseif ($d.Udid) { $list += $d.Udid } elseif ($d.UDID) { $list += $d.UDID } }
} catch {}
if ($list.Count -eq 0) { Fail 'Không thấy iPad/iPhone. Cắm cáp (cáp truyền dữ liệu), mở khoá máy, bấm "Tin cậy" rồi chạy lại.' }
if ($Device) {
    $udid = $list | Where-Object { $_ -ieq $Device } | Select-Object -First 1
    if (-not $udid) { Fail "Không thấy máy $Device. Đang cắm: $($list -join ', ')" }
} elseif ($list.Count -gt 1) {
    $ok = @($list | Where-Object { $udids -contains $_.ToUpper() })
    if ($ok.Count -ne 1) { Fail "Đang cắm nhiều máy: $($list -join ', '). Chọn một: install-ios.cmd -Device <UDID>" }
    $udid = $ok[0]
} else { $udid = $list[0] }

$name = (Invoke-Ios @('devicename', "--udid=$udid")).Trim()
try { $n = $name | ConvertFrom-Json; if ($n.devicename) { $name = $n.devicename } } catch {}
Say "Máy: $name ($udid)"
if ($udids.Count -gt 0 -and $udids -notcontains $udid.ToUpper()) {
    Fail ("Máy này chưa có trong chữ ký của bản build (chưa đăng ký vào Personal Team). Cắm máy này vào Mac build một lần," +
        " chạy build-ipad.command / build-iphone.command (không có --ipa) để đăng ký, rồi build lại .ipa.")
}

# --- 6. Cài
Say 'Cài app (giữ nguyên dữ liệu)…'
$out = Invoke-Ios @('install', "--path=$Ipa", "--udid=$udid") -All
$code = $script:IosExit
if ($code -ne 0 -or $out -match '"level":"(error|fatal)"') {
    Write-Host $out
    Fail ("Cài lỗi. Thử: mở khoá máy và cắm lại cáp; máy bật Chế độ nhà phát triển; còn dung lượng." +
        " Lỗi về chữ ký/profile: build lại .ipa trên Mac.")
}
Say 'XONG – đã cài 47 Librarian.' 'Green'
Write-Host 'Lần đầu cài trên máy này: Cài đặt → Cài đặt chung → VPN & Quản lý thiết bị → tin cậy Apple ID của bạn.'
