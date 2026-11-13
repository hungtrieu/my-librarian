@echo off
rem Cài file .ipa (build ở Mac bằng build-ipad.command --ipa) lên iPad/iPhone cắm cáp vào máy Windows.
rem Bấm đúp (tự lấy .ipa mới nhất) hoặc kéo-thả file .ipa lên file này. Nhiều máy: install-ios.cmd -Device <UDID>
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\install-ios.ps1" %*
set CODE=%ERRORLEVEL%
echo.
pause
exit /b %CODE%
