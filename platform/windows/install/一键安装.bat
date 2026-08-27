@echo off
rem HuFu 虎符输入法一键安装（自动请求管理员权限）
rem 双击本文件 → （管理员账户静默提权或 UAC 点「是」）→ 完成后按任意键退出

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo 检测到未提权，正在请求管理员权限…
    powershell -NoProfile -Command "try { Start-Process -FilePath '%~f0' -Verb RunAs -ErrorAction Stop } catch { Write-Host ('提权失败: ' + $_.Exception.Message); exit 1 }"
    if %errorlevel% neq 0 (
        echo.
        echo 提权被拒/失败。若你刚把本账户加入管理员：请 注销 - 重新登录 后再双击本文件。
        echo （组成员需重登才生效；重登后管理员账户将静默提权，不再弹窗）
    )
    exit /b
)

echo [HuFu] 已获管理员权限，安装中…
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0install.ps1"
echo.
echo [HuFu] 完成。按任意键关闭…
pause >nul
