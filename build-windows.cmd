@echo off
rem Build bộ cài Windows mới (bấm đúp file này). Chỉ build khi mã nguồn thay đổi; tự tăng phiên bản và chạy test.
rem Build lại dù không đổi gì: build-windows.cmd -Force
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-windows.ps1" %*
set CODE=%ERRORLEVEL%
echo.
pause
exit /b %CODE%
