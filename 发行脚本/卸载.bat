@echo off
rem Per-user uninstall (no UAC). HKLM leftovers (if any elevated install was used) are harmless.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1" -NoHKLM
cd /d "%USERPROFILE%"
pause