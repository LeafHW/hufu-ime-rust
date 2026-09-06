@echo off
rem Installs per-user + one UAC for machine-level registration (HKLM + msctf).
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
cd /d "%USERPROFILE%"
pause