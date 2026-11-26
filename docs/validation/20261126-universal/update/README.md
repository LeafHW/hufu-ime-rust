# 最终开发验证包：原版更新器实际下载

2026-11-26，使用公开 v1.9.9.9 的原始server二进制实际下载最终通用EXE，未运行安装器。

最终文件现位于 `交付/HuFuSetup-1.9.9.10.exe`；按用户要求整理目录，哈希未变。
下方原始命令与JSON保留测试时路径 `_tmp/arm64/delivery/20261126-arm64-dev/`。
大小39,502,283 bytes，SHA256：
`d46200ef856b2246ec5e3dc1fac7237ae1842ed4b8af678bb4d93046c7657cd1`。

复验发现确切新版1.9.9.10、SHA错误拒绝、首镜像失败回退、64MiB精确边界接受和超限拒绝五项全部通过。
随后首镜像故意503，旧客户端从第二个本地HTTP镜像成功下载最终EXE；下载文件大小与SHA256
均与最终交付文件一致，客户端状态为done、attempt=2。完整结果见[report.json](report.json)。

环境使用`tools/test_legacy_update.py`既定隔离协议：先后均无既有虎符CLSID/TIP/HuFu注册
或运行server，数据/环境变量/端口独立；下载名使用唯一UUID并预检不存在。最终下载仍落在
隔离USERPROFILE的Downloads，完整文件与.part均已清理，自建旧server已结束。
没有调用update/open、注册、卸载或安装；未访问真实更新端点。

实际命令：

```powershell
python tools\test_legacy_update.py --baseline-app _tmp\arm64\baseline\extracted\app --output _tmp\arm64\legacy-update-final-02 --artifact _tmp\arm64\delivery\20261126-arm64-dev\HuFuSetup-1.9.9.10.exe
```

此结果验证旧更新器能获取最终单EXE，不代表该EXE安装、UAC、更新回滚或真实应用输入已验收，
也不改变评分差异未通过正式质量验收的开发包边界。
