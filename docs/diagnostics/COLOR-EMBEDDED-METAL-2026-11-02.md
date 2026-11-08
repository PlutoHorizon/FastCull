# RAW 内嵌配置与 macOS Metal 色彩适配

日期：2026-11-02。用户要求优先使用 RAW 文件自身的色彩配置，尽量接近 ACR，并适配 macOS。

2026-11-03 续记：用户打开 Photoshop 后已成功补做 26 组明确版本与配置的 ACR 对照，见 [新对照记录](ACR-COLOR-2026-11-03.md)。下文的 COM 失败和留存样本数值保留为当日历史；用户已确认没有可用于验收的 Mac。

## 选择规则与实现

1. 显式的 `FASTCULL_DCP_PROFILE` 保持最高优先级。
2. XMP 指定配置时，优先寻找名称匹配的 DNG 内嵌配置；没有匹配项时按原有规则寻找本机 ACR DCP。`Embedded` 显式选择内嵌配置。
3. XMP 没有指定配置时，优先采用 DNG 的主内嵌配置，再回退机型对应的本机 DCP，最后使用 LibRaw 相机矩阵。界面显示内嵌来源或实际 DCP 名称。

`src/embedded_profile.rs` 有界 seek 读取 Classic TIFF/DNG 的色彩元数据，不整文件加载传感器载荷。支持主配置、额外扩展配置、双光源 ColorMatrix/ForwardMatrix、AnalogBalance、CameraCalibration 及签名、Hue/Sat、Look、ProfileToneCurve、曝光基线偏移。外部 DCP 与内嵌配置共用同一验证器。附加配置继承拍摄文件的相机校准元数据；缓存复核同一源文件和同一选择规则，管线版本已更新。

DCP 显式曲线改为 Adobe 公开 SDK 定义的自然 C2 三次样条，默认经典 SDR 曲线采用完整 1025 个参考采样，替代原有 11 点近似。两个 ColorMatrix 分别归一化后再插值；ForwardMatrix 也先分别归一化再插值。去掉从 0.8 开始的额外本地指数高光肩，曝光后再作 SDR 裁定，色调由配置曲线提供；负曝光仍可恢复浮点高光。原有线性浮点、DCP 处理和末端 sRGB 编码顺序保留。

ARW/NEF 通常提供相机矩阵、原片白平衡及厂商私有参数，不等于包含完整的 Adobe DCP。内嵌 JPEG 的 Picture Style/Creative Look 不能当作 RAW 像素的通用色彩配置。本地显示不会把“找到相机名称”报告成已复现 ACR 的所有处理。

## macOS

Windows 使用 DirectX 12；macOS 的 Apple Silicon 与 Intel 使用 Metal。两端共享 `src/raw_preview.wgsl` 的曝光、RAW 浮点预览、DCP Look 和显示公式，不提供 OpenGL 回退。macOS 通过 `src/macos_color.rs` 给实际 CAMetalLayer 标记 sRGB，让 ColorSync 执行显示器匹配，避免把 sRGB 数值直接作为宽色域设备 RGB。

构建脚本增加真实 Metal 像素对照及 RAW 窗口检查；检查要求日志中的 Metal 后端和 sRGB 标签，同时确认照片区域非空、调节确实改变像素。无 Metal 适配器的构建机不能据此通过 GPU 验收。当前 Windows 开发机上的交叉编译不能替代 Mac 真机运行。

## 验证记录与边界

本次尝试生成新的明确配置 ACR 参考时，Photoshop COM 返回 `0x80080005`，未成功启动；未用未生成的对照图宣称色彩一致。历史 ACR 图像的配置文件名称没有留档，最多用作固定样本回归。

- Windows release 全套 270 项通过、3 项忽略；其中真实 GPU 测试另行执行并通过。Windows 与 Apple Silicon 的全目标 Clippy `-D warnings` 通过，release 构建通过。
- NVIDIA RTX 4080 / Dx12：60 组 RAW/JPEG 参数各 64 像素、UV 翻转及 sRGB 目标编码检查通过，逐通道误差不超过 1.1 个 8 位码值。
- DNG 元数据测试覆盖大小端、128 MiB 逻辑文件（只读元数据）、越界及超量数据。另用完整合成 DNG 验证反字节序附加配置、相对偏移和按名选择；主配置与附加配置均成功进入显影，源文件 SHA-256 未变。
- 三格式私有副本原生窗口共 12 张截图：均完成调整、全尺寸、保存和关闭，主窗口保留；父子窗口均为 NVIDIA/Dx12。保存参数逐项一致，DNG 图像载荷哈希不变，源样本 SHA-256 不变。已查看实际窗口，DNG 配置标注为 `Embedded · 内嵌 · ILCE-6700`。
- 主选片器 4 张 ARW 私有副本通过曝光、对比及会话夹具：`error=None`，revision=5、导出 1 行；+1 EV 截图抽样 2625 个通道，平均误差 0.389 个码值。不是 Lightroom 宿主端到端验收；0 MiB 峰值表示未取得资源采样。
- Apple Silicon/Intel 全目标交叉编译通过，Apple Silicon 全目标 Clippy 通过；正式 WGSL 由 Naga 27 转为 Metal 2.0，3 个入口与真实绑定全部成功。尚未运行 Apple Metal 编译器、Mac 真机窗口或本次 Mac 应用包。

### 留存 ACR 样本比较

统一缩放到 192×128 后统计 RGB 平均绝对差，单位为 0–255 码值，没有为每张照片拟合曝光。它不是色卡 ΔE 或普遍一致性证明。

| 样本与设置 | 上一版本 | 本次版本 | 本次实际配置 |
| --- | ---: | ---: | --- |
| Sony ARW，5500 K / Tint 0 / EV 0 | 4.419 | 4.420 | 本机 Adobe Standard |
| Nikon NEF，5500 K / Tint 0 / EV 0 | 1.378 | 1.353 | 本机 Adobe Standard |
| DxO DNG，5500 K / Tint 0 / EV 0 | 3.480 | 3.214 | 文件内嵌配置 |
| DxO DNG，7500 K / Tint 20 / EV 0 | 3.457 | 3.253 | 文件内嵌配置 |
| DxO DNG，5500 K / Tint 0 / EV +1 | 2.956 | 2.020 | 文件内嵌配置 |

DNG 这组结果更接近留存参考；ARW/NEF 变化较小，部分 ARW 暖色/高曝光样本的误差略增约 0.03 个码值。没有因换成 SDK 曲线就达到 ACR 完全一致。选择内嵌配置的依据是文件信息和用户要求，而非按样本误差挑选。完整结果与哈希见 [验证数据](../raw-embedded-color-validation-2026-11-02.json)。

## 安装与交付

已更新 `lrplugins/FastCull.lrplugin/fastcull.exe` 及依赖声明，原生 DLL 未改。新 EXE SHA-256 为 `9e2280794c2496fe71cf3defd6d2f042cbb6035a3f26a57af80ee1e9591a238b`；上一版和声明保存在 `backups/2026-11-02-color-metal-231919/`。保留正在运行的旧窗口，新启动使用更新文件。

Windows 程序位于 `dist/diagnostics/2026-11-02-color-metal/`。同目录的 Source ZIP 包含跨平台源码、LibRaw 源码、构建和 Metal 验收脚本，不含用户照片或 Adobe 相机配置文件。没有公开发布、打标签或伪造 Mac 二进制；版本仍为 2.9.0。

ACR 的当前 ProcessVersion 色调、局部高光/阴影算法、增强 XMP Look、蒙版、去噪和锐化不由 DCP 全部定义。执行相同内嵌配置并不保证逐像素相同。程序仍独立运行，不把 Adobe 软件作为用户显影的必需依赖；官方 SDK 仅用于理解格式与参考数值。

## 依据

- [Adobe DNG SDK 与增强 Profiles SDK](https://www.adobe.com/support/downloads/dng/dng_sdk.html)：本轮只读核对公开的矩阵、曲线与配置格式；经典 SDR 数值的版权及 SDK 许可随源码和软件分发。
- [Apple CAMetalLayer.colorspace](https://developer.apple.com/documentation/quartzcore/cametallayer/colorspace)：未设置该属性时，内容不进行颜色匹配。

临时 SDK、只读对照副本、日志和构建缓存位于 `.agent-temp/2026-11-02-color-metal/`；正式程序不依赖该目录。
