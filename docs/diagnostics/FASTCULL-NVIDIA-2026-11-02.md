# FastCull NVIDIA 专项诊断

日期：2026-11-02（Asia/Shanghai）。用户希望保留 NVIDIA GPU；本轮先区分 GPU、图形 API 与呈现同步因素。

后续更新：用户进一步要求迁移 RAW 编辑和曝光预览，现已完成 DirectX 实现并恢复入口、更新安装。下文“暂时禁用”描述此前的规避阶段；当前实现、EXE 哈希及三格式窗口验收以 [RAW DirectX 迁移记录](RAW-DIRECTX-2026-11-02.md) 为准。

## 对先前结论的限定

先前对照证明 Intel UHD + DirectX 12 可以规避症状，同时改变了适配器和 API，不能据此认定 NVIDIA GPU 本身不可用。原 OpenGL 路径出现“输入命中、内部照片编号推进，但用户可见画面未正常更新”，因此本轮重点是呈现链，仍保留图形后端或应用集成问题的可能性，不直接归责驱动。

## 当前独立验证

新建独立测试程序 `fastcull-nvidia.exe`，默认明确枚举并选择 `vendor=0x10de` 的离散显卡。相同 EXE 可通过进程环境切换：

- `FASTCULL_DIAG_BACKEND=dx12` 或 `gl`。
- `FASTCULL_DIAG_VSYNC=1` 或 `0`；该开关用于后续 OpenGL 对照，DirectX 当前使用其默认呈现设置。
- 导出 NVIDIA 官方文档描述的 `NvOptimusEnablement=1`；PE 导出表已核验。OpenGL 初始化会记录实际 vendor/renderer/version，避免只凭期望配置判断 GPU。

NVIDIA DirectX 窗口已实际启动，日志确认：

```
NVIDIA GeForce RTX 4080 Laptop GPU
vendor=4318, device=10208, device_type=DiscreteGpu
driver=32.0.16.1692, backend=Dx12
```

同批 845 张 ARW 的首张图绘制日志正常。约 3 分钟空闲采样中，进程 CPU 累计用时仅从约 3.38 秒增至 3.52 秒，工作集约 334 MiB、私有提交约 619 MiB，没有持续增长。用户随后确认：**选片和整机都正常**。这些空闲读数不作为活动期性能基准。

## 环境与只读检查

- 实际 NVIDIA 公开驱动版本由 `nvidia-smi` 读取为 616.92，RTX 4080 Laptop 显存总量 12282 MiB。
- 一次空闲读数：GPU 1%、显存 2746 MiB、46°C、4.18 W。此时未出现显存占满或高温现象；没有活动期性能结论。
- Windows 报告版本 10.0.26300；两块显示输出的 WMI 当前刷新率分别约为 NVIDIA 75 Hz、Intel 239 Hz。尚未据此认定跨显卡或混合刷新率是根因。
- 通过 NVIDIA 公开 NVAPI DRS 接口，只读查询 FastCull 应用及当前全局/基础配置：没有找到 `fastcull.exe` 或诊断 EXE 的专用应用配置；所查 OpenGL 呈现、同步、G-SYNC、线程优化等项返回 `NVAPI_SETTING_NOT_FOUND`。这不等同于测得这些选项的有效默认值，也不能认定驱动行为没有其他覆盖。
- 诊断期间没有改动驱动或全局显卡设置。最终安装替换见下文。

## 已完成的 NVIDIA 单变量对照

| 实际后端 | 用户确认 |
| --- | --- |
| NVIDIA RTX 4080 + DirectX 12 | 选片和整机都正常 |
| NVIDIA OpenGL，垂直同步开启 | 选片冻结或整机卡顿 |
| NVIDIA OpenGL，仅关闭垂直同步 | 仍然冻结或卡顿 |

同一 EXE 与同一批照片的结果将问题限定在本机 NVIDIA OpenGL 图形路径；关闭垂直同步不足以规避。尚未证明具体 NVIDIA 驱动内部缺陷或应用集成的某一行代码是根因。用户最后要求“先不要让这个选片器使用 opengl”，因此停止继续运行 OpenGL 对照。

## Windows 实现与安装（本地修订，未发布）

- 新增 `src/graphics.rs` 统一窗口配置：Windows 使用 wgpu，后端仅允许 DirectX 12；在支持当前窗口的适配器中优先 NVIDIA 离散 GPU。未配置 OpenGL 失败回退。
- 主选片器、启动壳窗口统一使用该配置。保留显卡最大二维纹理尺寸，以免默认 8192 限制影响较宽照片的 100% 查看。
- RAW 编辑及曝光预览依赖原 OpenGL 管线，当前 Windows 入口暂时禁用并显示说明；RAW 命令行入口也在读取请求前拒绝执行。基础选片、评分、对比及裁切保留。
- 非 Windows 仍使用原图形后端；本次未做 macOS 运行验收。Windows 仍编译原 GL 模块以维持共享类型，但不创建 GL 上下文；`opengl32.dll` 被链接加载不代表正在使用 OpenGL 渲染。
- 已替换 `lrplugins/FastCull.lrplugin/fastcull.exe`，Bridge/Lightroom 注册路径不变。旧文件保存在 `backups/2026-11-02-nvidia-dx12-195629/fastcull.exe`，未删除。
- 可独立运行的交付物：`dist/diagnostics/2026-11-02-nvidia-dx12/`，包含 EXE、同目录原生 DLL、许可证、说明和校验清单，不依赖临时目录。
- 新 EXE SHA-256：`2a58639e194d54422510339462de995b3826e62b3aa5f5125deecc39d532829c`；旧文件为 `cc84f322bb88bf0df9550fe08330caf8b3203df47a3d954209e12f84e94cd461`。

## 验证与边界

- Windows release 全套串行测试：267 项通过，2 项忽略；Clippy 全目标 `-D warnings` 通过，release 构建通过。沙箱下的一项 junction 测试因权限拒绝失败，移到允许环境后该项及全套均通过。
- 打包 EXE 使用 4 张真实 ARW 私有副本，分别完成 Bridge 与 Lightroom 会话原生窗口流程：切换、100%、评分/标记和旋转，均 `error=None`；Lightroom 产生 revision=4、导出 1 行。截图已实际查看。测试报告中的 0 MiB 峰值是未取得资源采样，不能用作内存结论。
- 原 Lightroom 测试夹具缺少已完成的曝光/方向/裁切参数邮箱，因此旋转被产品保护逻辑拦截；修正 `scripts/native_smoke.py` 夹具后通过，未放宽产品保护。
- RAW 命令行拒绝测试退出码为 1，提示 DirectX 暂不支持 RAW 编辑。两项 Windows 回归测试验证 UI 及命令行均不会启动 OpenGL RAW 子窗口。
- 首次安装后探针错误地把测试环境变量设为空串，触发内置测试断言。核对原目录仍为 845 张 ARW、无旁车、无修改；仅产生的截图已移入临时目录。该次探针及其 `installed-directx-health.json` 无效，不作为产品故障或通过证据。
- 随后用 Python 独立环境真正移除全部 `FASTCULL_*` 测试开关，仅设置日志目录；修正探针的 Bridge 清单头，再用安装路径只读打开原批次。启动及后端检查不替代最终安装版的用户手动交互；NVIDIA DirectX 诊断版已完成上述用户交互确认。
- 最终安装版 PID 56992 的日志确认 RTX 4080 / Dx12，原批次发现耗时 156 ms、首张适屏图日志 67.2 ms；运行后进程存活。一次 8 秒空闲采样消耗 CPU 0.015625 秒，工作集约 319 MiB、私有提交约 614 MiB。这里只是一次空闲探针，不能推广为持续选片性能基准。模块中有 D3D12，没有 NVIDIA OpenGL 驱动 `nvoglv64.dll`；原目录文件名、大小、修改时间在探针前后完全一致（845 张）。最终窗口保留供用户使用。

## 证据与参考

- 本轮源码、构建日志、GPU/驱动读取脚本及运行记录：`.agent-temp/2026-11-02-hang/` 下 `nvidia-source/`、`nvapi/`、`nvidia-dx12-v1-logs/`。
- 官方 GPU 选择机制：[NVIDIA Optimus Rendering Policies](https://developer.download.nvidia.com/devzone/devcenter/gamegraphics/files/OptimusRenderingPolicies.pdf)。
- 驱动设置名称、ID 和结构：[NVIDIA NVAPI 官方源码](https://github.com/NVIDIA/nvapi)、[NVAPI 设置参考](https://docs.nvidia.com/nvapi/_nv_api_driver_settings_8h_source.html)。
