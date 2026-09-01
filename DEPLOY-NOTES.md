# DLL 部署铁律（实测确认）

## 真实加载路径

TSF COM 注册的 InprocServer32 指向系统目录，宿主应用实际加载：

- **64 位 DLL**：`C:\Windows\SystemIME\HuFu\hufu_tsf.dll`
- **32 位 DLL**：`C:\Windows\SysWOW64\SystemIME\HuFu\hufu_tsf32.dll`

`D:\HUFJ\...\hufu_tsf.dll` 只是安装目录副本，替换它**不会被任何进程加载**。
2024-09-01 曾因此整轮部署无效（行尾跟随优化用户报"还是跟之前一样"），
用 `Get-Process -Id <pid> -Module` 才定位真身。

## 权限与提权

两个系统目录都需要管理员。非管理员会话用：

```powershell
Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','E:\DSH-KF\elevate-deploy.ps1' -Wait
```

脚本要点：纯 ASCII 内容与路径、改名腾位保留 .bak1、结果写标志文件回传验证。
UAC 弹窗由用户点确认，属 Windows 授权，非沙箱 escalation。

## 生效条件

1. 换 DLL 后重启 ctfmon。
2. 已加载旧 DLL 的宿主进程（跟打器等）也必须重启。
3. 验证：`Get-Process -Id <pid> -Module | Where-Object ModuleName -match hufu`
   看 FileName 是否指向 SystemIME 真身。
