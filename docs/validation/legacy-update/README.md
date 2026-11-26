# 已发布 v1.9.9.9 更新器兼容验证

2026-11-26：运行原公开安装包解出的旧 `hufu-server.exe`，不是重新编译的模拟客户端。
SHA256 `2638031e7a1f922ac1b8379e49cf995bced3b29edf243b7b11d86c6a9a6dcc91`。
启动前/结束后检查 HKCU/HKLM 的32/64视图本产品CLSID、TIP、Software\HuFu及运行server均无既有实例。
源审查确认数据、诊断、皮肤和同步配置可随隔离`--data`及环境变量重定向；
原版固定渲染helper的两个回退路径也纳入启动阻断检查。仅复制server与version.txt，
不复制TSF/helper，不使用原提取目录作为运行数据目录。

`tools/test_legacy_update.py` 使用独立目录、随机端口、隔离的LOCALAPPDATA、APPDATA、
USERPROFILE、TEMP、TMP、HOME、HUFU_DATA；HUFU_UPDATE_MANIFEST仅指向本机测试HTTP。
下载文件名带随机UUID，事前检查不存在，允许落到真实KnownFolder或隔离USERPROFILE的
Downloads，仅清理本次自建的两个名称（完整文件与.part）。不改KnownFolder注册表。
此机旧进程实际落点为隔离USERPROFILE\Downloads；真实Downloads没有产生测试文件。
未请求任何open/安装/注册接口，未调用真实更新endpoint；结束只终止本测试拥有的PID。
收据server_exitcode=1来自测试的TerminateProcess，非启动/下载失败。

已完成：

| 检查 | 结果 |
| --- | --- |
| 旧版1.9.9.9发现清单新版99.0.0.1 | 通过 |
| SHA256不一致拒绝落地，删除.part | 通过 |
| 首镜像503，第二镜像成功且哈希一致 | 通过 |
| 恰好67,108,864 bytes（64MiB） | 接受，哈希一致 |
| 67,108,865 bytes | 拒绝并清理 |
| 下载临时文件清理与结束后无本产品注册/进程 | 通过 |

[完整固定用例收据](fixture-report.json)。现场目录：
`_tmp/arm64/legacy-update-fixture-02/`。第一次fixture-01因测试脚本仅检查真实Downloads，
未识别旧版实际使用隔离Downloads而误报第二用例失败；产物哈希实际正确。
已修正为严格接受上述两个明确目录，原失败证据保留，第一次自建下载文件已清理。

最终通用EXE尚未生成，本次不是最终包在线更新验收。最终包准备后复验：

```powershell
python tools\test_legacy_update.py --baseline-app _tmp\arm64\baseline\extracted\app --output <全新绝对隔离目录> --artifact <最终通用EXE绝对路径>
```

所有参数路径请用绝对路径。脚本会重复上述fixture并增加“首镜像失败→下载最终EXE→SHA一致”，
仍不会执行EXE。目标已安装或运行虎符时拒绝启动并保存阻断记录；该情况下需独立测试环境。
