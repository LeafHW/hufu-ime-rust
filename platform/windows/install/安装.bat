@echo off
rem HuFu 虎符输入法安装（自动请求管理员权限：机器级注册需要，
rem 一次 UAC 完成全部安装；server 等普通权限部分由脚本自动降权处理）
rem 双击本文件 → 管理员账户静默提权或 UAC 点「是」→ 完成后按任意键退出

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo 检测到未提权，正在请求管理员权限…
    powershell -NoProfile -Command "try { Start-Process -FilePath '%~f0' -Verb RunAs -ErrorAction Stop } catch { Write-Host ('提权失败: ' + $_.Exception.Message); exit 1 }"
    if %errorlevel% neq 0 (
        echo.
        echo 提权被拒/失败。若你刚把本账户加入管理员：请 注销 - 重新登录 后再双击本文件。
        echo 普通权限也可安装每用户部分：直接运行 install.ps1（机器级注册需管理员一次）。
    )
    exit /b
)

echo [HuFu] 已获管理员权限，安装中…
cd /d "%USERPROFILE%"
powershell -ExecutionPolicy Bypass -NoProfile -File "%~dp0install.ps1"
echo.
echo [HuFu] 安装流程结束。按任意键关闭…
pause >nul
