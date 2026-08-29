@echo off
rem Per-user install (no UAC, no admin needed). Output stays in this window.
rem Machine-wide HKLM layer is optional: powershell -File install.ps1 -HKLM
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -NoHKLM
cd /d "%USERPROFILE%"
pause