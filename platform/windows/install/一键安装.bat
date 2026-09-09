@echo off
rem HuFu one-click installer (auto elevation via UAC)
rem [fix 2026-09-11] elevation check switched from 'net session' to 'fltmc'
rem (net session fails for admins on some systems and can cause elevation
rem loops); restructured so %errorlevel% is never read inside a parenthesized
rem block (parse-time expansion bug); installer exit code is now reported.
rem [fix 2026-09-11] file kept pure ASCII (English comments only).

fltmc >nul 2>&1
if %errorlevel% equ 0 goto run

echo Not elevated. Requesting administrator rights...
powershell -NoProfile -Command "try { Start-Process -FilePath '%~f0' -Verb RunAs -ErrorAction Stop } catch { exit 1 }"
if errorlevel 1 goto elevfail
exit /b

:elevfail
echo.
echo Elevation was declined or failed. If you just added your account to the
echo Administrators group, log off and log back in, then double-click again.
pause
exit /b 1

:run
echo [HuFu] Elevated. Installing...
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0install.ps1"
if errorlevel 1 goto instfail
echo.
echo [HuFu] Install finished OK. Press any key to close...
pause >nul
exit /b 0

:instfail
echo.
echo [HuFu] Installer reported an error. Check the messages above.
pause
exit /b 1
