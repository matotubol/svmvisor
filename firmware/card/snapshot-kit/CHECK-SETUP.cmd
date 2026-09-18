@echo off

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Run.ps1" -CheckOnly

set "capture_status=%errorlevel%"
pause
exit /b %capture_status%

