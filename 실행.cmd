@echo off
echo Starting KakaoRelay...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\Start.ps1"
if errorlevel 1 (
  echo KakaoRelay failed to start. See the error above.
  echo Log: %LOCALAPPDATA%\KakaoRelay\logs\startup.log
  pause
  exit /b 1
)
