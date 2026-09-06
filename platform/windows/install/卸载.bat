@echo off
rem HuFu 虎符输入法卸载（自动请求管理员权限，一次清光：
rem 注册表+SystemIME+ProgramData+早期版本残留+安装目录）
rem 双击本文件 → 管理员账户静默提权或 UAC 点「是」→ 完成后按任意键退出

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo 检测到未提权，正在请求管理员权限…
    powershell -NoProfile -Command "try { Start-Process -FilePath '%~f0' -Verb RunAs -ErrorAction Stop } catch { Write-Host ('提权失败: ' + $_.Exception.Message); exit 1 }"
    if %errorlevel% neq 0 (
        echo.
        echo 提权被拒/失败。若你刚把本账户加入管理员：请 注销 - 重新登录 后再双击本文件。
    )
    exit /b
)

echo [HuFu] 已获管理员权限，卸载中（含早期版本残留清理）…
cd /d "%USERPROFILE%"
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0uninstall.ps1"
echo.
echo [HuFu] 卸载完成。按任意键关闭…
pause >nul
