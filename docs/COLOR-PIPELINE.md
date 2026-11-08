# RAW 色彩管线与校准

2026-10-30 的改动解决了提前编码为 8 位 sRGB、缺少 DCP 外观处理和两套白点标度的问题。已有 ARW/NEF 样本对留存 ACR 参考的误差明显降低；这些样本用于检查实现方向，不代表所有相机、光源与 Lightroom 配方均已匹配。

## 处理顺序

1. LibRaw 解包与去马赛克，输出 16 位线性相机 RGB；按实际白平衡增益归一尺度还原为浮点，保留高光余量。
2. 原片白平衡直接保留相机增益。自定义 K/Tint 使用原生与 Rust 共用的 Robertson 等温线模型，并按机型 DCP 色彩矩阵求相机白点。
3. 选择机型 DCP，在倒数色温上插值；处理匹配签名的校准矩阵、AnalogBalance、ForwardMatrix 或矩阵回退，再转换到 D50 ProPhoto。
4. 缩小预览时按像素覆盖面积聚合线性浮点数据，再进行几何采样和随光源插值的 Hue/Sat 校正，以减少细碎颜色与噪点的混叠；灰点采样保留原有配置处理前的相机数据。平均后再执行非线性色彩处理与 ACR 的末端缩图并不完全等价。
5. GPU 应用绝对曝光、基础色调、DCP Look 与配置色调曲线，再调整自然饱和度/饱和度，转换为 sRGB 并在显示末端编码。曝光之后进行 SDR 范围裁定，不额外插入先前从 0.8 开始的本地指数高光肩；负曝光仍可恢复保留在浮点输入中的高光余量。

HSV 表按饱和度最快存储，色相环绕、三线性插值；sRGB 编码标签作用于明度查表，不先编码三个 RGB 通道。显式 DCP 曲线使用自然 C2 三次样条；没有显式曲线时使用公开 Adobe DNG SDK 经典 SDR 曲线的完整 1025 个采样，并非当代 Lightroom 默认渲染的完整复刻。[Adobe DNG SDK](https://www.adobe.com/support/downloads/dng/dng_sdk.html)

曝光只在显示配方中应用一次。CPU 数值验收与直方图使用同一组显示公式；Windows 窗口由 DirectX 12 / WGSL 执行，macOS 由 Metal 执行同一着色器，其他平台保留 GLSL。线性缓存保留 float32 像素、两级 Look、曲线、曝光基线和相机灰点采样，读取时验证尺寸、有限值和配置文件指纹。当前管线版本为 `scene-prophoto-v3-enhanced-area`，缓存元数据为 V2，避免复用旧显示配方。

## 配置文件与回退

RAW 编辑器提供配置选择；DNG 初次编辑默认 Adobe Color，已明确保存的选择优先。`Embedded` 选择文件主内嵌配置，附加配置按名称选择；ARW/NEF 没有完整内嵌配置时列出本机机型匹配 DCP。下层显影接口在 XMP 未指定配置时仍可选择主内嵌配置。

Adobe Color 从本机安装的增强 XMP 读取：基础 Adobe Standard DCP、额外 HSV LookTable 和主曲线。查找表解码采用有界 ASCII85 / zlib 读取，双 Look 在 CPU、DirectX 与 Metal 共用的显示公式中依次应用；主曲线按 gamma 2.2 工作域与基础曲线合成。该曲线域模型不等于实现 ACR 专有 ProcessVersion 算法。保存使用标准 `crs:Look/Parameters`、表标识和 `Table_...` 数据，避免只改名称却仍渲染 Adobe Standard。未找到资源或不支持的增强配置明确显示实际回退。

搜索范围和环境变量见 [RAW 编辑说明](RAW-EDITING.md)。本机配置文件不随程序分发；没有 DCP 时使用 LibRaw 相机矩阵与本地色调。当前已读取 Classic TIFF/DNG 内嵌的双光源矩阵、校准、Hue/Sat、Look、色调曲线及曝光偏移。BigTIFF、三光源配置、DefaultBlackRender 的全部处理、任意增强 XMP/RGB LUT、蒙版及当代 Adobe 局部/基础算法仍有缺口，不能宣称与 ACR 完全一致。历史记录见 [内嵌配置与 Metal](diagnostics/COLOR-EMBEDDED-METAL-2026-11-02.md)，本轮见 [配置选择与 Adobe Color](diagnostics/RAW-PROFILE-SELECTOR-2026-11-03.md)。

## 验收范围

2026-11-03 已补齐 Photoshop 27.11.0 / ACR 18.7 / ProcessVersion 11.0 的 26 组明确配置对照，逐项核对 ACR 实际配置与参数；包括 DNG Embedded、Sony Camera ST、Nikon Camera Standard。仍有显影差异，对比度调节后差异扩大；详见 [新 ACR 对照记录](diagnostics/ACR-COLOR-2026-11-03.md)。以下早期留存参考的版本不明限制仍适用于历史数值。

使用 Sony ILCE-6700 ARW、Nikon D3S NEF 与 DxO 输出 DNG，在 5500 K / Tint 0 和 7500 K / Tint 20 下，与 2026-10-24 留存的真实 ACR sRGB 输出按相同统计方法对照。参考图当时的准确配置文件名称未记录，不能用该对照证明指定 DCP 与 Lightroom 像素等价。没有按单张照片拟合曝光补偿。

统计采用统一 192×128 采样图，RGB MAE 单位为 0–255 码值；亮度偏差为有效像素的线性亮度比 log2 中位数。MAE 同时受到亮度、颜色、采样和细节影响，不是感知色差 ΔE。完整数值见 [校准验证 JSON](raw-color-calibration-validation-2026-10-30.json)，改动前的单因素调查见 [调查记录](RAW-COLOR-INVESTIGATION-2026-10-30.md)。

2026-10-30 的色彩校准只运行离线测试与数值分析，检查原 RAW 哈希、缓存逐位往返、曝光方向、白点求解、取消恢复和旧接口兼容；当时 GLSL 仅离线编译/链接，未做真实 GPU 窗口验收。2026-11-02 的 DirectX 迁移另完成 RTX 4080 真机 GPU 对 CPU 像素对照及三格式原生窗口检查，见 [迁移记录](diagnostics/RAW-DIRECTX-2026-11-02.md)。这些验证不等同于 Lightroom 色彩一致性或用户手动滑块体验验收；macOS 尚未实机验证。

下一步精细校准应固定相机、光源、LR 版本、ProcessVersion、配置文件及所有滑块，取得用户实际 ARW/NEF 的同尺寸 sRGB 导出；以灰卡/色卡和独立场景分别检查中性轴、肤色、高饱和度及高光，再用未参与调参的照片验收。显示器色彩管理与画面统计应分别验证，避免把屏幕外观差异混入 RAW 算法修正。
