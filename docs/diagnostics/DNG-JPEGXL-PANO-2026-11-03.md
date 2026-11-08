# ACR 全景 DNG 的 JPEG XL 解码修复

2026-11-03，样本 `DSC06526-Pano.dng`，49,186,790 字节，14173×6402（90,735,546 像素）。原始样本 SHA-256：`c09aeea8c073326137860471a928a3577bd4adda5633f2d24d733f6bbb940bc7`。测试副本和图像只存于本机任务临时目录，不随源码分发。

## 复现与原因

修复前运行 `fastcull --benchmark <样本> 1`，返回 `RAW 解码：Unsupported file format or not RAW file`。同一选片解码接口的探针结果：Thumb 200×84，Fit 256×108，Full 失败。

文件包含普通 JPEG 小缩略图，以及 JPEG XL 压缩的 1024×430 RGB 预览、完整 LinearRaw 瓦片和透明掩膜。原有快速读取只支持普通 JPEG，现有原生库未带 JPEG XL DNG 解码功能。文件还带三个必需的 Stage 2 `MapPolynomial` 指令，省略它们会使全尺寸结果严重偏色。

协议依据：[LibRaw DNG 支持说明](https://github.com/LibRaw/LibRaw/blob/master/README.DNGSDK.txt)、[Adobe DNG SDK 的 JPEG XL 实现](https://android.googlesource.com/platform/external/dng_sdk/+/80a267ed1ac714acff455e85ae28c1732777d5b6/source/dng_jxl.cpp)。实现使用 `jxl-oxide` 0.12.6，不引入新的原生 DLL。

## 实现

- `src/dng/jxl.rs`：有界 IFD/瓦片读取、RGB 预览、整数 RAW 临时解压、Stage 2 多项式映射、透明掩膜和进程缓存。
- `src/dng.rs`、`src/arw.rs`、`src/imaging.rs`：标记并读取 JPEG XL 预览，继续保留普通 JPEG 快速路径。
- `src/native_raw.rs`：原生解码/显影使用只读临时副本；解压副本的写句柄先关闭，兼容 LibRaw 在 Windows 的文件共享模式。
- `src/main.rs`：工作线程结束后释放解压缓存。
- `examples/dng_probe.rs`：通过实际选片解码接口检查三个级别，断言 Full 与原尺寸一致，输出有限尺寸的视觉参考。

没有把 256/1024 像素小图放大冒充全尺寸。读取不修改原始照片或其 XMP；窗口交互测试在私有照片副本上执行。

## 验证记录

可重建的完整构建、日志、探针、照片副本和截图位于 `.agent-temp/2026-11-03-pano-dng/`。单元回归使用上游公开的最小 JPEG XL 示例数据，不包含用户照片；覆盖大小端、仅有 JPEG XL 的预览、实际解码入口、边缘瓦片、样本值和元数据保留、缓存复用、取消、坏偏移，以及多项式的通道/区域/步距/顺序。

最终路径 `lrplugins/FastCull.lrplugin/fastcull.exe` 已更新，版本仍为 2.9.0，未发布新标签或公开版本。原 EXE 与许可声明备份于 `backups/2026-11-03-pano-dng-174059/`。新 EXE SHA-256：`7b8d5d8636c4256d9e5a4b24322766e35ff0bedaaebe39264ce7e04da64238a1`；DLL 未变。

- 现有测试套件复跑 280 项通过、3 项忽略；新增四项 JPEG XL/多项式回归包含在内。首次执行遇到临时文件访问拒绝和沙箱不允许建立目录联接，沙箱外完整复跑通过，未为此改变保存逻辑。
- Clippy 全 target、禁止警告检查通过；`git diff --check` 通过。
- 原图三个级别均可解码；适屏读取 1024×430 的 ACR RGB 预览，完整解码输出 14173×6402。实际安装路径的一次完整 CPU 解码为 12.571 秒，含临时解压、必要映射、显影与透明掩膜，不含 GPU 上传。单次、文件缓存已预热，不是稳定性能承诺。
- 真实窗口在 NVIDIA DirectX 12 上完成适屏、100% 数据加载、网格、评分/标记/旋转与退出测试，`full=1 fit=1 error=None`。另用 Windows 桌面工具检查最终 EXE 的单图适屏及显示 `100%` 的实际画面，确认加载提示消失并显示细节。
- 普通 ARW、NEF 和原有 PureRAW DNG 的安装路径全尺寸解码也通过，分别为 4128×6192、4256×2832、6192×4128。
- 原始全景 DNG 的 SHA-256 验证前后相同；所有交互保存都在私有副本上，正常退出的解压缓存为空。

本轮只验证 Windows。100% 使用既有本地显影，和内嵌 ACR 预览仍有亮度、颜色差异；此修复不实现完整 Adobe 显影算法，也不新增 macOS 真机验收。
