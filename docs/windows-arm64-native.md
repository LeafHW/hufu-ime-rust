# Windows ARM64：原生评分与渲染助手

## 锁定来源和许可证

`native/sentence/sources.lock.json` 是原生评分的唯一源码锁。包装层取自虎爪已提交的
`d3a723c0f6727407696bb5281924dc6dd2855815`，由该提交的 README 声明 GPL-3.0-only，
原始 LICENSE 和 README 随源码保存。llama.cpp 固定
`9b05354ec6fb58b4e665e9a39ebc40285c015638`，归档 SHA256
`3757add5ecea48727edf81b56d3e233eb82332d30ac475f23aa3efec4c647d1c`，保留 MIT 许可证。
不读取虎爪活动工作树，不调用其部署脚本。

历史 SHARED 配置参考 `f38e813c7f7878b7776803a50b8fb4807013896a`。
历史包装层与当前源码哈希不同；当前版本添加可取消评分 API、将默认候选数/context/batch
常量提为宏（默认仍 5/512/512），原 tcs_score 使用空取消回调。历史未确认许可证的源码
没有复制作为构建输入，也没有假定当前许可证追溯覆盖历史。锁中保留历史文件哈希便于复核。
当前原始 CMake 只作来源记录，顶层 CMake 机械恢复 DLL SHARED 目标及原 C ABI：
`tcs_create_from_file`、`tcs_score`、`tcs_destroy`。

## 构建接口

Windows Python 3、Visual Studio C++/ARM64、CMake、ClangCL 必须可用：

```powershell
native\sentence\build.ps1 -Architecture x64 -BuildRoot C:\Workspace\hufu-build-tools\sentence -LlamaArchive C:\Workspace\hufu-build-tools\downloads\llama-9b05354.tar.gz
native\sentence\build.ps1 -Architecture arm64 -BuildRoot C:\Workspace\hufu-build-tools\sentence -LlamaArchive C:\Workspace\hufu-build-tools\downloads\llama-9b05354.tar.gz
```

输出 `<BuildRoot>\<architecture>\Release\TigerClaw.Sentence.Native.dll` 和独立
`hufu-native-score-probe.exe`；`native-build.json` 记录源码锁和 DLL 哈希，但仅表示已构建。
默认 VS 18 2026；其它安装可指定 `-Generator`/`-CMake`/`-Python`，并可指定 `-Jobs`。
`prepare.py` 检查包装层、许可证、下载归档及已解压每个文件，拒绝修改过或多文件缓存。
x64 DLL 使用 MSVC，ARM64 使用 ClangCL/MSVC ABI（锁定 ggml 明确拒绝 ARM MSVC cl）。
两者静态 CRT、CPU only、GGML_NATIVE=OFF，不继承构建机专用指令集。
Rust 应用仍按既有 x64 GNU/新增 aarch64-pc-windows-msvc 构建；C ABI DLL不要求客户端同工具链。
发布时保留上述源码锁、许可证、可获取的对应源码和构建说明。

## 隔离验证与边界（2026-11-26）

公开安装包只解包，未执行。评分基线 DLL SHA256：
`e41677d27d3d4f804e8dd7d5e6183d14d355c94c4308a0793e0676c02e7c0a50`。
只读 Q8 模型（GGUF v3，310 tensors，639446720 bytes）SHA256：
`ebb25a17e79b1f43834410fb711ac3dc985364eb875b45914181f55b9993f2d0`。
脚本 `compare_scores.py` 独立加载两套 DLL、每组五候选，经相同模型实际评分，保存
输入哈希、TSV、进程日志、绝对误差及排序结果。不会启动输入法后台或注册 TSF。

已构建 x64 DLL SHA256 `fd5cfbba681d0d5c55d82a5f5efee9bdeccd73c96558f4cb5fe8c2302301bc7e`；
最初25及扩展125个固定候选与发布 DLL 的每个 double 完全相同。此结果支持当前包装层/ABI/模型的等价性，
不是原发布 DLL 完整来源证明，也不能推广到未测模型和任意输入。
ARM64 原生 DLL SHA256 `374f9847c5bf0a79f79fa274b699a37c273cc3aad75dfc0e04dc3553396f43bd`。
初始25候选最大绝对误差0.3615707766378904，五组排序相同；扩展125候选、25组时
Top1 25/25一致，完整排序24/25一致，第25组第2/3项互换。严格诊断阈值 abs1e-4/rel1e-6
未通过，不能报告数值完全等价；该阈值是本次诊断阈值，并非上游承诺的跨架构精度契约。
原始证据 `_tmp/arm64/native-scores/published-vs-x64`、`published-vs-arm64`、
`published-vs-arm64-extended`。后续应结合CPU kernel消融定位及实际候选质量确定验收边界。

Rust rerank ARM64 release 单元测试5/5通过。debug模式受上游gemm-common0.19未使用FP16
内联汇编缺target_feature影响，报fullfp16；release优化不生成该未使用代码并通过。
没有为避错全局要求ARM FP16 CPU特性。生产使用release；debug构建缺口仍需上游修复。

## 渲染组件定位

`engine/crates/hufu-server/src/render_paths.rs` 首先接受成对明确覆盖
`HUFU_RENDER_HELPER`/`HUFU_RENDER_DLL`，供隔离验证使用。正常运行优先后台同目录
`package-architecture.json` schema1，按 `render-helper`/`tsf-native` 唯一角色选择，
校验架构、相对路径、目录边界与SHA256；清单损坏/缺项直接失败，不选择旧DLL。
无清单旧版才允许同目录组件、HKCU/HKLM native64 CLSID InprocServer32所指的
SystemIME代次，最后兼容 `%SystemRoot%\SystemIME\HuFu`；不猜最新目录或固定C盘。
一次预览使用同一次解析得到的helper/DLL对，不缓存旧代次。
已通过ARM64 release路径逃逸/清单角色与哈希测试；这不是实际皮肤渲染或安装后的宿主验收。

### CPU kernel消融

额外诊断构建保持锁定源码原样：ARM C/C++ flags取消`__ARM_NEON`/`__ARM_NEON__`，
同时`GGML_CPU_REPACK=OFF`（C++保留`/EHsc`），输出独立`arm64-scalar`。
该变体125候选相对发布DLL最大绝对差1.6467622351817397，Top1 24/25、全排序23/25；
不能将其作为数值改进发布。完整原始得分与输入哈希已保留在
`native/sentence/tests/evidence/`，包括失败结果。
锁定上游有架构独立量化路径：ARM `ggml/src/ggml-cpu/arch/arm/quants.c:41` Q8_0
计算倒数`1/(amax/127)`并使用NEON nearest-even；x86 `arch/x86/quants.c:302`
计算`127/amax`并使用AVX nearest-even；scalar参考路径使用`roundf`。
这些公式与归约顺序并不保证每个float/量化值相同；仅源码差异不能单独证明全部分数误差的原因。

第二消融构建x64关闭GGML_AVX/AVX2/FMA/F16C/SSE42/BMI2/CPU_REPACK。
其与ARM禁NEON变体比较125候选，最大绝对差0.36499032855250846、Top1 23/25、
完整排序22/25；未消除跨架构误差，不能归因成单独某一个NEON kernel错误。
默认ARM路径在本候选集反而保持全部Top1且仅一组低分候选换位；保留默认优化路径，
不将消融变体投入包内，也不将尚未完全定位的误差描述为已证明无影响。
x64标准重建与原发布的扩展125候选差分仍全部double一致。
最终渲染路径测试（包含坏schema/架构/版本、重复角色、哈希损坏和逃逸）2/2通过。

这些证据不包含正式安装、活动配置、后台端口、真实应用输入、卸载或原生渲染验收。
