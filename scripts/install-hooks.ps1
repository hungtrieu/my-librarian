# Bật git hook của dự án (.githooks/pre-push chạy CI nhanh trước mỗi lần push).
# Gỡ: git config --unset core.hooksPath
Set-Location (Split-Path -Parent $PSScriptRoot)
git config core.hooksPath .githooks
Write-Host "Đã bật hook: $(git config core.hooksPath)"
