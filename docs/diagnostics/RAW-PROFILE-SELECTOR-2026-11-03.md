# RAW 配置选择与 Adobe Color（2026-11-03）

用户要求继续优化 RAW 色彩处理，在编辑界面选择文件实际包含的配置，并让 DNG 默认使用 Adobe Color。

## 实现

RAW 编辑器在调整区顶部增加“配置文件”下拉框。后台枚举文件的主/附加内嵌配置，并以“文件内嵌”标明来源；同时列出本机与机型匹配的 DCP。ARW/NEF 的机内风格元数据不等于完整 DCP，不将本机 Camera Matching 文件冒充 RAW 自带配置。

DNG 没有明确保存的旁车选择时，以本机 Adobe Color 建立编辑草稿；已有自定义 Look 或已保存的选择优先。只有评分/曝光的旁车不阻止默认选择。旧文件若仅保存 Adobe Color 名称而没有实际表数据，首次编辑会补齐完整配方，保存后不再重复补写。选择后重新显影；保存和关闭自动保存均持久化该选择，回执传回主选片器。切换普通 DCP 时移除旧增强 Look 和配置摘要，其他显影、评分、裁切和未知元数据保留。

Adobe Color 使用本机安装的 `Adobe Color.xmp` 和机型 Adobe Standard DCP：额外 HSV LookTable 通过有界 ASCII85 / zlib 解码，验证维数、数值及灰轴；两级 Look 在 CPU 与 GPU 中顺序执行。增强主曲线按 gamma 2.2 工作域与基础曲线合成。查找表和曲线加入缓存指纹，浮点缓存升级 V2，管线名称为 `scene-prophoto-v3-enhanced-area`。本机 Adobe 配置不随软件分发；缺少资源时明确报告实际回退。

缩小预览时采用线性浮点面积聚合，保留负值与 HDR 均值，避免仅取四个点产生混叠；全尺寸路径不受该缩放影响。该处理发生在后续非线性 Look/色调之前，与 ACR 最终显示图的缩图顺序不完全等价。

## ACR 配方核对

Photoshop 27.11 / ACR 18.7、PV 11.0 的 DNG 私有副本对照显示：只写 `CameraProfile="Adobe Color"`，或只写 Look 名称/UUID，输出与 Adobe Standard 完全相同；不能据此声称采用 Adobe Color。写入完整 `Look/Parameters`、LookTable 引用、主曲线及顶层表数据后，实际输出发生变化。

生产 XMP 保存代码生成的配方已交给 ACR 复读；有效文档中记录 Adobe Color 与 `E1095149FDB39D7A057BAB208837E2E1` 表。其 ACR 输出与独立构造的完整配置参考逐通道一致（平均差 0.0）。这验证保存格式及配置的实际采用，不是本地显影与 ACR 逐像素一致的证明。

对同一 ACR Adobe Color 参考，旧 Adobe Standard 基础输出的该 DNG 样本 MAE 为 5.739，本次实际 Adobe Color 为 3.734（统一 192×128 RGB，单位为 0–255 码值）。这是固定样本的联合结果，不是所有相机的提升比例。保留的 Adobe Standard 三格式样本中，面积聚合后的 MAE 略增约 0.07–0.19 个码值；本轮不把抗混叠处理宣称为普遍改善 ACR 色彩误差。数值同时包含采样、亮度及颜色差异，不是感知色差 ΔE，没有针对照片拟合曝光或颜色补偿。

## 验证状态

release 全套 276 项通过、3 项忽略；其中真实 GPU 用例另行执行并通过。Windows 全目标 Clippy `-D warnings`、release 构建、分发清单检查通过。已通过真实指针选择下拉菜单的单元验证、面积采样 HDR/取消验证、双 Look 缓存往返及增强表合成样本验证；旧 XMP 命名空间兼容回归已修复。

NVIDIA RTX 4080 Laptop / Dx12 实测 78 组颜色参数各 64 像素，加上镜像 UV 与 sRGB 目标检查，CPU 与 GPU 逐通道误差不超过 1.1 个 8 位码值。四组原生双窗口完成调整、全尺寸和保存，共 16 张截图：ARW Camera ST、NEF Camera Standard、DNG Adobe Color、DNG Embedded。源样本未变，DNG 图像载荷未变，父窗口保留，父/子窗口均为 NVIDIA/Dx12。已查看各组实际截图。安装路径又单独完成一组 DNG Adobe Color 保存与回传检查（4 张截图）。完整记录见 [验证数据](../raw-profile-validation-2026-11-03.json)。

Apple Silicon / Intel 的编译检查、Apple Silicon Clippy 和 WGSL 到 Metal 2.0 的三个实际绑定入口转换通过。用户没有 Mac，本轮没有 Mac 真机或显示管理验收。

## 安装与交付

已更新插件目录安装版 `lrplugins/FastCull.lrplugin/fastcull.exe`，SHA-256 为 `37494eb69d7ccf84ca343745108a0de163bd63685acb27124ec622f3bb12a06b`。原生 DLL 未变；旧 EXE 和声明保存在 `backups/2026-11-03-profiles-122033/`。程序与跨平台源码包整理在 `dist/diagnostics/2026-11-03-profiles/`。本轮没有公开发布、打标签或生成未经真机验收的 Mac 二进制。

参考图、完整 XMP、运行日志和私有 RAW 副本保存在 `.agent-temp/2026-11-03-profiles/` 对应的本轮临时目录中，不作为程序运行依赖、不随源码包分发。

## 格式依据

- [Adobe DNG / Enhanced Profiles SDK](https://www.adobe.com/support/downloads/dng/dng_sdk.html)：LookTable、增强配置及公开流格式。
- [Adobe Camera Raw 配置说明](https://helpx.adobe.com/camera-raw/desktop/edit-and-enhance-images/tone-and-color/adjust-color-rendering-camera-camera.html)：Adobe Raw 与 Camera Matching 配置的用途。

支持范围为当前 Adobe Color 的 HSV LookTable 与主曲线，并未实现任意 RGB LUT、HDR 增强配置、蒙版或 Adobe 专有局部/基础显影算法。
