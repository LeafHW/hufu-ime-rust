@echo off
rem Pure-ASCII skeleton: all CJK text lives in install.ps1 (UTF-8 BOM).
rem Immune to console codepage (936/65001) confusion.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
pause