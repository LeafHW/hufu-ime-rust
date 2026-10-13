@echo off
rem HuFu 虎符输入法 · 卸载入口（exe 安装时代；本文件 GBK+CRLF，cmd 中文安全）
setlocal
set "UNINS=%~dp0unins000.exe"
if exist "%UNINS%" (
    start "" "%UNINS%"
    exit /b 0
)
echo 未找到卸载程序 unins000.exe，将打开系统应用设置页面……
start "" ms-settings:appsfeatures
echo 请在列表中找到「HuFu 虎符输入法」再点卸载（卸载会保留 码表、码表导出、数据、模型）。
endlocal
pause
