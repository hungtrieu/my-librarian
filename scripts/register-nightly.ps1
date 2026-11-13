<#
.SYNOPSIS
  Đăng ký tác vụ Task Scheduler chạy CI đầy đủ mỗi đêm (build + đóng gói vào dist\).
  Gỡ: powershell -File scripts\register-nightly.ps1 -Remove
#>
param([string]$At = '02:00', [switch]$Remove)
$name = '47 Librarian - nightly CI'
if ($Remove) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "Đã gỡ tác vụ '$name'"
    return
}
$root = Split-Path -Parent $PSScriptRoot
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$root\scripts\ci.ps1`"" -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -Daily -At $At
# StartWhenAvailable: máy tắt đúng giờ thì chạy bù khi bật lại.
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -DontStopIfGoingOnBatteries -AllowStartIfOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 2)
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Settings $settings -Description 'CI tại máy cho 47 Librarian (scripts\ci.ps1)' -Force | Out-Null
Write-Host "Đã đăng ký '$name' lúc $At hằng ngày. Kết quả: target\ci\latest.log, bản build: dist\"
