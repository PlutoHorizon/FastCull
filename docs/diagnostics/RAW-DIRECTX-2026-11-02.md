# RAW 编辑与曝光预览迁移到 DirectX 12

日期：2026-11-02。用户要求保留 NVIDIA，停用本机复现冻结的 OpenGL 路径，随后要求将先前临时禁用的 RAW 编辑和曝光预览迁移到 DirectX。

## 实现

- `src/graphics.rs` 保持 Windows 仅允许 DirectX 12、优先 NVIDIA 的策略。RAW 子窗口、主选片器及启动窗口都通过同一配置创建，不提供 OpenGL 回退。
- `src/raw_preview.rs` 保留共享配方和 CPU 参考公式；旧后端移到 `src/raw_preview_gl.rs`，仅在非 Windows 编译。Windows 使用 `src/raw_preview_wgpu.rs` 和 `src/raw_preview.wgsl`。
- 浮点 RAW 在 GPU 上保持 RGBA32Float，依次处理绝对曝光、白平衡显示增益、七项基础调整、DCP Look、色调曲线、高光肩及 ProPhoto/D50 到 sRGB 变换；显示末端才编码。LibRaw、相机/DCP 前半段处理、灰点采样、缓存和元数据保存协议未改。
- 显式双线性采样不要求显卡具备可选的 float32 硬件过滤特性。DCP 查找表与曲线使用 GPU 只读缓冲；输入尺寸、有限值、表尺寸和缓冲上限在上传前校验。上传完成释放 CPU 场景像素，GPU 资源随帧引用释放。
- JPEG 曝光预览直接引用 egui 已有纹理，解码 sRGB、应用曝光、重新编码；不保留额外全尺寸纹理。每次绘制有独立参数缓冲，支持对比窗口两侧不同参数。新纹理尚未上传时只请求下一帧，不在渲染回调内部重入锁定 egui 渲染器。
- 恢复 RAW 编辑入口和主窗口曝光快调。曝光仍需有效基线；禁用提示不改变原先的 LR 参数等待规则。RAW 子进程保存回执、主窗口锁定与恢复机制继续沿用。
- 源码分发白名单已包含 `.wgsl`，避免源码包遗漏内嵌着色器。

实现依据为当前依赖的 egui-wgpu 0.33.3 原生回调接口：[CallbackTrait](https://docs.rs/egui-wgpu/0.33.3/egui_wgpu/trait.CallbackTrait.html)、[Renderer::texture](https://docs.rs/egui-wgpu/0.33.3/egui_wgpu/struct.Renderer.html#method.texture)。

## 验证记录

- 真机 NVIDIA RTX 4080 Laptop / DirectX 12 的离屏像素对照已通过：60 组 RAW/JPEG 参数，每组 64 像素，外加 UV 翻转；逐通道误差不超过 1.1 个 8 位码值。覆盖 HDR 输入、曝光、白平衡增益、七项基础控制、两种 DCP 查表编码和非恒定曲线。
- 初次着色器验证发现 WGSL 保留字 `meta` 导致管线无效；已修正，并增加创建管线后的验证错误检查，后续真实 GPU 对照通过。仅在项目临时验证过程中发生，未安装该失败版本。
- Apple Silicon 和 Intel macOS 交叉 `cargo check` 均通过；这只证明保留的非 Windows 后端编译通过，不是 macOS 真机 GPU 验收。
- Windows release 常规全套串行测试 266 项通过、3 项忽略；其中新增的真实 GPU 测试已用项目自身 Cargo.lock 单独执行并通过，另两项仍为原有可选测试。Clippy 全目标 `-D warnings`、release 构建均通过。
- 三格式真实窗口：ARW、NEF、DNG 私有副本各完成载入、调整曝光/白平衡/七项控制、全尺寸、保存及关闭，共 12 张截图。主/子窗口日志均为 RTX 4080 / Dx12；RAW 子窗口关闭后主窗口保留，参数旁车逐项读回正确，三份源样本 SHA-256 不变。ARW/NEF 副本本体不变；DNG 副本旁车与内嵌 XMP 一致，压缩图像载荷哈希不变。已实际查看三种格式的渲染截图。
- 原生 Lightroom 会话夹具使用 4 张真实 ARW 私有副本：`error=None`，revision=5、导出 1 行；单张曝光、对比两侧、100% 相对位置联动验证通过。+1 EV 截图抽样 2625 个通道，平均误差 0.389 个 8 位码值，两侧画布均有照片。这里模拟的是协议邮箱，不是真实 Lightroom 宿主端到端验收。
- 原生 Bridge 管道/分离启动夹具通过，4 张照片 `error=None`；裁切原生流程通过，角度、比例、旋转、保存后重读及预览均由应用内测试执行。主窗口曝光、对比和裁切截图已实际查看。测试报告中 0 MiB 峰值表示未取得资源样本，不用于内存结论。
- 一次 RAW 窗口探针错误地预期“保存后”的状态文字仍为“浮点显影”，因此在应用已成功保存后断言失败；修正探针预期后，三格式完整验收通过，未为此修改产品代码。
- 源码清单测试确认 WGSL、Windows 和保留的 GL 源文件全部进入源码包，并继续排除独立 DxO 扩展。

## 当前安装与交付

- 已更新 `lrplugins/FastCull.lrplugin/fastcull.exe` 及依赖许可证，Bridge/Lightroom 入口路径不变；原生 DLL 未修改。
- 独立目录 `dist/diagnostics/2026-11-02-directx-raw/` 包含 EXE、DLL、许可证、第三方声明、说明和 SHA-256 清单，不依赖临时目录。未公开发布或打标签，包版本仍为 2.9.0。
- EXE SHA-256：`bd30059ecc83f55a01122f33a1e6de02366b0ece2f7090062f71d0075e1b30f4`。安装路径与交付 EXE 校验一致。
- 上一个临时禁用 RAW 的 EXE 和旧第三方声明保存在 `backups/2026-11-02-directx-raw-203324/`。通过重命名保留旧文件，原先打开的窗口未强制结束；后续新启动使用本次版本。
- 安装后用独立环境、仅设置日志目录，从实际安装路径只读打开原批次 845 张 ARW：日志确认 NVIDIA RTX 4080 Laptop / Dx12，发现照片约 139.4 ms，首张适屏图日志 46.7 ms，进程存活。一次 5 秒空闲探针 CPU 用时 0 秒，工作集约 322 MiB、私有提交约 617 MiB；这些是单次空闲读数，不能推广为持续编辑性能。
- 安装后进程加载 D3D12，没有 NVIDIA OpenGL 驱动 `nvoglv64.dll`。由于共享 eframe 类型仍编译 glow，系统 `opengl32.dll` 可被链接加载；不表示创建了 GL 上下文。原批次文件名、大小及修改时间前后完全一致。新窗口保留供用户使用；本次实际手动滑块手感与整机体验未另请用户确认。

临时日志、构建缓存、私有照片与截图位于 `.agent-temp/2026-11-02-raw-dx12/`；未自动删除。早先图形故障的证据仍保留在其独立临时目录。

## 色彩边界

本次移植保持 FastCull 的原有本地公式，不承诺与 Lightroom/ACR 像素一致。GPU 对 CPU 的像素对照用于检验迁移一致性；实际照片的相机/DCP 兼容范围仍见 [色彩管线](../COLOR-PIPELINE.md)。
