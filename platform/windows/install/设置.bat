@echo off
setlocal
rem HuFu settings: prefer the new WebView2 shell; fallback to Edge /next.
tasklist /FI "IMAGENAME eq hufu-server.exe" 2>nul | find /I "hufu-server.exe" >nul
if errorlevel 1 (
  start "" /B "%~dp0hufu-server.exe"
  timeout /t 2 /nobreak >nul
)
if exist "%~dp0hufu-settings.exe" (
  start "" "%~dp0hufu-settings.exe" --url "http://127.0.0.1:4390/next?shell=1"
  exit /b 0
)
set "BROWSER="
if exist "%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe" set "BROWSER=%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe"
if not defined BROWSER if exist "%ProgramFiles%\Microsoft\Edge\Application\msedge.exe" set "BROWSER=%ProgramFiles%\Microsoft\Edge\Application\msedge.exe"
if not defined BROWSER if exist "%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe" set "BROWSER=%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe"
if not defined BROWSER if exist "%ProgramFiles%\Google\Chrome\Application\chrome.exe" set "BROWSER=%ProgramFiles%\Google\Chrome\Application\chrome.exe"
if defined BROWSER (
  start "" "%BROWSER%" --app=http://127.0.0.1:4390/next?shell=1
) else (
  start "" http://127.0.0.1:4390/next?shell=1
)
