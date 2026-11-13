@echo off
rem Build APK Android trên Windows (bấm đúp). Chỉ build khi mã nguồn thay đổi. Build lại: build-android.cmd -Force
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-android.ps1" %*
set CODE=%ERRORLEVEL%
echo.
pause
exit /b %CODE%
