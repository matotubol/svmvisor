@echo off

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Run.ps1"

set "capture_status=%errorlevel%"
pause
exit /b %capture_status%

