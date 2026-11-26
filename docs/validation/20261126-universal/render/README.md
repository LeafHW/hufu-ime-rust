# ARM64 原生渲染助手与 ARM64X facade 实际出图

2026-11-26，对最终候选载荷 `payload-95b54d2fba2246e98c97fe7f937d54e8/arm64`
执行隔离验证。清单SHA256 `941c26356ebbd932e69040ad9a35b1c517acc44c5114b304786ba377a217d812`。
逐文件核验载荷hash后复制到 `中文路径-虎符验证`；真实运行复制后的ARM64 server、
ARM64 render-helper，经ARM64X facade调用 `hufu_test_skin_dump` 并同步输出BMP。
所有二进制哈希见 [report.json](report.json)。

启动前与结束后检查均无本产品既有注册或运行server。数据、配置、皮肤缓存、
LOCALAPPDATA/APPDATA/USERPROFILE/TEMP/TMP/HOME及HUFU变量完全隔离，HTTP随机端口。
TSF的既有固定命名管道不能重命名，所以测试只在无虎符安装/运行实例时允许启动。
helper readback模式不显示候选窗。未调用注册、安装、卸载或更新open接口。
测试结束终止自建server，来源载荷与SSF哈希保持。

`pkg-source/数据/皮肤`只有JSON。此次SSF输入为最终server实际内嵌的
`engine/crates/hufu-server/official-ssf/`四款源文件，只读复制到测试数据目录。

| 皮肤 | BMP尺寸 | 不同像素颜色数 | helper退出码 |
| --- | --- | --- | --- |
| [虎娘-绯绒小恶魔](虎娘-绯绒小恶魔.bmp) | 385×292 | 21110 | 0 |
| [虎娘-莓糖小恶魔](虎娘-莓糖小恶魔.bmp) | 369×313 | 17809 | 0 |
| [蜜桃虎娘](蜜桃虎娘.bmp) | 281×336 | 6120 | 0 |
| [鲸鱼娘](鲸鱼娘.bmp) | 276×329 | 9871 | 0 |

四份BMP签名、头部、尺寸、像素长度和非单色内容检查通过。这证明最终原生helper能从中文路径
加载facade并完成D3D/候选窗readback，不等同真实应用TSF键入或安装后验收。
LoadLibraryW已存在于395e0ec基线，本轮没有把它记为新增Unicode修复。

复验工具 `tools/test_render_helper.py`：

```powershell
python tools\test_render_helper.py --payload <最终arm64载荷> --resources <pkg-source> --ssf-source engine\crates\hufu-server\official-ssf --output <新的隔离目录>
```

原始现场与日志 `_tmp/arm64/render-final-01/`。评分质量仍有另文记录的跨架构差异，
本渲染结果不改变“首轮仅开发验证包”的交付边界。

## 与最终交付包绑定

最终载荷为`payload-e080f8d8385448229d9fe2f9a4e33973`。
再次逐文件实读hash核对最终ARM64载荷，并与上述已渲染载荷比较：唯一变化是
`hufu-setup-engine.exe`；server、helper、facade、两实现DLL及其它资源全部字节相同，
因此无需重复本渲染。最终ARM64清单SHA256：
`35747341ed57f8d7104317092b0cb8eff6fb25b87e7fc14b5ab222a34e807308`。
对应最终`HuFuSetup-1.9.9.10.exe` SHA256：
`d46200ef856b2246ec5e3dc1fac7237ae1842ed4b8af678bb4d93046c7657cd1`。
原渲染报告保留当时载荷hash，不用新清单覆盖旧证据。
