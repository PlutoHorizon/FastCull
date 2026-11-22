# 超分（2× Hi-Res 类）集成评估（2026-11-22）

## 参考对象

事实：GitHub `y-g-jiang/Sony-ExRaw-For-All-Cam` 是 Sony "ExRAW Hi-Res v3.48" 的第三方实现——单个 69 MB ONNX（SHA256 43fd0e45…b94324ab22），输入 `[1,8,H,W]`（4 路 Bayer 相位信号 + 4 路同位 stddev），输出 `[1,3,4H,4W]`；图含 Conv×53 / ConvTranspose×5 / Relu×24 / Add×28（另有 Constant×24、Mul×24 单位残差）。打包输入取自 `raw[y::2,x::2]`，即半分辨率打包，因此最终输出 = **传感器每维 2×、总像素 4×**。
适用条件：讨论"把该模型搬进 FastCull"时的几何与算子事实（本次用纯 Python protobuf 解析器实测 ONNX 输入输出形状与算子计数）。
证据：2026-11-22，克隆到 /tmp 读 models/exraw_hires_v3.48.onnx，自写解析器输出；MODEL_INFO.txt、wgpu_model.py。
更新人和时间：ZCode，2026-11-22。

## 技术可行性

事实：该实现不用 ONNX Runtime/PyTorch，而是把卷积写成手写 WGSL 隐式 GEMM（16×16 workgroup、显式寄存器分块、≤16 KiB shared memory、FP32），并在 GPU 上跑固定的 158 节点图；分块 tile=512（max_storage_buffer_binding_size<256 MB 时 352），halo=112，打包像素 core=288，输出按 `halo*4` 裁切；输出经 memmap 落盘，再写 32-bit float 线性 TIFF 或 16-bit 线性 DNG。
适用条件：判断 FastCull 能否内嵌该内核——FastCull 已依赖 wgpu 27（Metal/DX12）+ wgsl，设备来自 `cc.wgpu_render_state`，但项目至今**没有任何 compute pipeline**（`create_compute_pipeline` 零命中），本项将是首个 compute 管线。
证据：2026-11-22，repo/wgpu_model.py、repo/engine.py（infer/packed/encode_export）、src/graphics.rs、src/raw_preview_wgpu.rs。
更新人和时间：ZCode，2026-11-22。

## 集成 FastCull 的四个改造点

事实：(1) **Bayer 入口缺失**——FastCull 的 LibRaw 桥（native/raw_bridge.cpp）走 `unpack()+dcraw_process()` 直接输出 8-bit/16-bit 去马赛克结果，从不暴露 mosaic；超分需要新增 `fc_raw_bayer`（可见区 mosaic + black/white per channel + CFA pattern + WB/matrix）。(2) **噪声先验**——模型要 4 路每像素 stddev，参考实现用 Sony PTC（自带的只是 Panasonic S5 ISO100 显式先验，S5II 用近似），其他相机只能手填 alpha/beta/c 或自行估计。(3) **内存**——输出 2×：45 MP 机身 → 192 MP ×3ch×f32 ≈ 2.3 GB，必须 memmap + 分块。(4) **输出格式**——FastCull 无 TIFF writer，JPEG 写出仅 Windows 可用（native_raw.rs:292 write_jpeg=None）；可先用 png crate 16-bit RGB，或新增 TIFF/DNG writer。
适用条件：任何"在 FastCull 里加 AI 超分"的方案设计；落点建议遵循行业惯例（DxO/Topaz/darktable 均为"只挂全分辨率导出，不进选片预览"），且 `src/raw_view.rs` 视图坐标与预览分辨率解耦，2× 产物可整体替换 full 纹理而不动缩放/裁剪数学。
证据：2026-11-22，native/raw_bridge.cpp、src/native_raw.rs、src/raw_view.rs、src/local_raw.rs、docs/COLOR-PIPELINE.md。
更新人和时间：ZCode，2026-11-22。

## 结论

事实：技术路径清晰、单人约 2–3 周（内核移植 3–5 天 + Bayer/σ 2–3 天 + 分块内存 2 天 + 输出格式 2–3 天 + UI 2 天 + 校验 2 天），但**权重不可随 FastCull 分发**——见 sony-sdm-weights-license.md；参考仓库把权重直接提交进公开 GitHub 仓库，与 README"为合规请自行提取"自相矛盾，且其 ONNX 来源未与本机 IEDT 产物核验一致（IEDT 本地 SDM 权重是 4 个加密 .dat，流水线为 sdm_pre→sdm_nn→sdm_post，与本仓库的单一 158 节点图形态不符）。
适用条件：决定 FastCull 超分路线时的取舍——推荐"自备可再分发权重（RealPLKSR Apache-2.0 / darktable 同款）做通用 2×"为主线，Sony 权重只做本机自备的实验开关，且必须先向用户核实来源与 EULA 立场。
证据：2026-11-22，本文件前述各条 + https://github.com/Jannchie/llr/blob/main/docs/sony-edit-internals.md + IEDT EULA 原文。
更新人和时间：ZCode，2026-11-22。
