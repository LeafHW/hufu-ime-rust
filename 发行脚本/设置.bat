@echo off
rem HuFu 设置：server 未在跑则先拉起，然后开独立窗口（GBK 控制台，双击场景 936 原生）
tasklist /FI "IMAGENAME eq hufu-server.exe" 2>nul | find /I "hufu-server.exe" >nul
if errorlevel 1 (
  start "" /B "%LOCALAPPDATA%\HuFu\hufu-server.exe" --data "%LOCALAPPDATA%\HuFu\数据"
  timeout /t 3 /nobreak >nul
)
set "BROWSER="
if exist "%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe" set "BROWSER=%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe"
if not defined BROWSER if exist "%ProgramFiles%\Microsoft\Edge\Application\msedge.exe" set "BROWSER=%ProgramFiles%\Microsoft\Edge\Application\msedge.exe"
if not defined BROWSER if exist "%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe" set "BROWSER=%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe"
if not defined BROWSER if exist "%ProgramFiles%\Google\Chrome\Application\chrome.exe" set "BROWSER=%ProgramFiles%\Google\Chrome\Application\chrome.exe"
if defined BROWSER (
  start "" "%BROWSER%" --app=http://127.0.0.1:4390/
) else (
  start "" http://127.0.0.1:4390/
)