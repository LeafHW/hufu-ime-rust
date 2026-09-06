@echo off
rem HuFu 虎符输入法卸载（一点开即请求管理员权限，一次清光：
rem 注册表+SystemIME+ProgramData+早期版本残留+安装目录）
rem 【防循环双保险】elev 标记：提权自启后直接进主流程，不做二次检测
rem （net session 检测法在部分系统上管理员也失败会导致无限自我重启）

if /i "%~1"=="elev" goto run
powershell -NoProfile -Command "exit [int](-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
if %errorlevel%==0 goto run
echo 检测到未提权，正在请求管理员权限…
powershell -NoProfile -Command "try { Start-Process -FilePath '%~f0' -ArgumentList 'elev' -Verb RunAs -ErrorAction Stop } catch { exit 1 }"
if %errorlevel% neq 0 (
    echo.
    echo 提权被拒/失败。若你刚把本账户加入管理员：请 注销 - 重新登录 后再双击本文件。
)
exit /b

:run
echo [HuFu] 管理员模式，卸载中（含早期版本残留清理）…
cd /d "%USERPROFILE%"
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0uninstall.ps1"
echo.
echo [HuFu] 卸载完成。按任意键关闭…
pause >nul
