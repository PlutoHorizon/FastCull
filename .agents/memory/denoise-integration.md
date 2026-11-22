# 降噪集成评估（DXO PureRAW / 自带模型）

事实：DXO PureRAW 的降噪引擎（DeepPRIME 3 / XD3）无法集成进 FastCull——DXO 没有公开 SDK/API/CLI（仅 LR/PS 插件 + Finder 扩展，插件 Lua 已混淆），EULA 2.2(a)(b)(c) 禁止分发、禁止逆向、禁止以服务方式代跑；也不存在可写入 PureRAW 降噪设置的 XMP 键。
适用条件：任何"把 DxO 降噪搬进本软件"的提案；可行替代是自建 ONNX 推理或把 PureRAW 输出 DNG 当普通输入读取。
证据：2026-11-22，https://www.dxo.com/eula/ 、https://support.dxo.com/hc/en-us/articles/4436466219153- 、https://forum.dxo.com/t/cli-automation-support-for-pureraw-workflow-integration/54884 。
更新人和时间：ZCode，2026-11-22。

事实：可自带的降噪权重与许可：PMRID（Bayer 域 ONNX，Apache-2.0，512×512 分块、4 通道打包 RGGB 输入、需增益匹配）、NAFNet（RGB 域，MIT，768×768 分块，SIDD 训练，与 RAW 散粒噪声有域差）、Restormer；darktable 5.6 的 `rawdenoise-nind`（UtNet2/RawNIND）为 GPL-3.0，闭源商业分发有风险。darktable 5.6 已用 ONNX Runtime（macOS CoreML、Windows DirectML）实现 CFA 域 raw denoise。
适用条件：选型与许可判断；MIT/Apache-2.0 可随 MIT 许可的 FastCull 分发，GPL-3.0 权重需先咨询。
证据：2026-11-22，https://github.com/darktable-org/darktable-ai 、https://github.com/MegEngine/PMRID 、https://github.com/megvii-research/NAFNet 、https://docs.darktable.org/usermanual/5.6/en/special-topics/ai/overview/ 。
更新人和时间：ZCode，2026-11-22。

事实：行业惯例是降噪只作用于全分辨率输出/导出，不进选片预览代理（DxO、Capture One、Topaz Photo AI 均如此）；Adobe 侧 AI Denoise 自 ACR 15.3 / LR 6.3 / LRC 12.3 起提供，但没有公开编程接口（Camera Raw SDK 下载页已 404），只能走用户本机 ACR 往返。
适用条件：FastCull 的降噪落点——应挂在 `src/local_raw.rs` 的线性场景（`native_raw::develop_linear` 输出 float32 线性相机 RGB）之后或导出路径，预览网格只做低成本近似。
证据：2026-11-22，https://blog.adobe.com/en/publish/2023/04/18/denoise-demystified 、https://forum.dxo.com/t/using-dxopureraw-from-another-software-other-than-adobe-on1/52448 、本仓库 `src/local_raw.rs`、`docs/COLOR-PIPELINE.md`。
更新人和时间：ZCode，2026-11-22。
