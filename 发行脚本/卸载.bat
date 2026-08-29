@echo off
rem Pure-ASCII skeleton: all CJK text lives in uninstall.ps1 (UTF-8 BOM).
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1"
pause