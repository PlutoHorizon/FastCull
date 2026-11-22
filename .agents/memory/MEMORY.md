# 项目记忆索引

本目录由 DSH、Codex、ZCode、Claude Code 共用；每条记忆一个文件，本文件只放索引。

格式：- [主题](文件) — 一句话（YYYY-MM-DD，来源）

- [dng-support](dng-support.md) — DNG 快速预览、RGB8 小图、内嵌 XMP 与三张 DxO PureRAW 实测，148 项 Rust 通过（2026-10-23，Codex）
- [raw-editing](raw-editing.md) — Adobe RAW 工作区、完整 LR 配方、曝光/WB、DNG 内嵌保存、184 项 Rust 与真实三格式窗口验证（2026-10-24，Codex）
- [project-overview](project-overview.md) — FastCull 是 Rust + egui 的 RAW/JPEG 选片器，2.7.4，Windows 分发 + macOS 待验收（2026-10-18，DSH）
- [build-and-test](build-and-test.md) — Rust 工具链在 `.agent-temp/2026-10-12-rust/toolchain/`，构建需设 CARGO_HOME/RUSTUP_HOME/TARGET_DIR（2026-10-18，DSH）
- [workspace-acl](workspace-acl.md) — 工作区曾因缺 WRITE_OWNER 导致 pwsh 无法启动，2026-10-18 已用 acl 诊断脚本修复（2026-10-18，DSH）
- [uncommitted-work](uncommitted-work.md) — 原“载入已有裁切”的未提交文件已于 2026-10-18 在 fae9d98 提交（2026-10-18，DSH）
- [performance-tuning](performance-tuning.md) — 2026-10-18 实测基线、ColorImage 零拷贝、FastThumb、12 线程三项改动（2026-10-18，DSH）
- [release-and-macos](release-and-macos.md) — 2.7.4 包校验通过但落后源码；源码包补 macOS 构建文件；Apple 双架构 cross-check 通过（2026-10-18，DSH）
- [macos-objc-encoding](macos-objc-encoding.md) — macOS debug 构建启动 abort 的根因是 msg_send! 编码 `^v` 与 `^{CGColorSpace=}` 不符；改用 objc2-core-graphics 的 CGColorSpace 后 debug/release 一致（2026-11-10，实现者-p0）
- [ui-redesign-tokens](ui-redesign-tokens.md) — 改版 P1/P2：src/theme.rs 设计令牌放在库 crate（examples/filter_bench.rs 用 #[path] 引入 filter_ui.rs，crate::theme 会编译失败）；全项目无硬编码字号；P2 控件语义分级与星级升级，egui Button fill 全状态钉死、horizontal_wrapped 嵌套会爆高度（2026-11-10，实现者-p1/p2）
- [ui-redesign-tokens P3 补记](ui-redesign-tokens.md#2026-11-10-p3-阶段顶栏压成单行-56px--照片加框圆角描边阴影内边距) — 顶栏单行 56px、照片 12px 内边距 + 6px 圆角 + 1px 描边 + 阴影；egui 不能裁圆角贴图要用画布色盖角（扇形填充必须以方角为第 0 点）；GPU 曝光预览的 canvas 必须传整块画布；1px 描边会被像素回归当成照片像素（2026-11-10，实现者-p3）
- [ui-redesign-tokens P6 修订：星星填充是错的](ui-redesign-tokens.md#2026-11-11-修订星星的填充是错的凹多边形不能用-convex_polygon) — 五角星是凹多边形，egui 的 convex_polygon 从第一点扇形三角化会把凹口糊成五片翅膀；改成绕中心的一圈三角形；Shape::line 不闭合，轮廓星顶点缺一条边（2026-11-11，实现者-p6）
- [ui-redesign-tokens P5 修订：侧栏状态栏与胶片条材质](ui-redesign-tokens.md#2026-11-11-修订收侧栏状态栏与胶片条材质统一) — 侧栏底部加状态栏（显示 N/M 张 + 重置 + 拾取/拒绝/评分计数）；缩略图与画布照片同一材质（圆角+描边+投影）；闭包里调 self.method 会整捕获 self 与 rows 冲突；RichText::append_to 要 4 个参数（2026-11-11，实现者-p5）
- [ui-redesign-tokens P4 修订：浮动圆角卡片](ui-redesign-tokens.md#2026-11-11-修订主窗口改为浮动圆角卡片) — 主窗口五个区域改成悬浮圆角卡片（10px 间距 + 阴影 + 顶边受光），画布底色整窗竖向渐变；Frame::new() 默认 inner_margin 是 8；epaint 0.33 无 epaint::Path，圆角弧角度写反会留毛刺（2026-11-11，实现者-p4）
- [ui-redesign-tokens P3 再修订](ui-redesign-tokens.md#2026-11-10-p3-修订640480-工具栏溢出与网格格子被压扁) — egui 嵌套组永远不会换行（申请整段剩余宽度），要先把组拿到屏幕外量宽度再按宽度摆放才会整组换行；网格列数跟随可用宽度（2026-11-10，实现者-p3）

## 归档（迁移自旧 agent 目录，2026-10-17）

- [super-resolution-integration](super-resolution-integration.md) — Sony ExRAW Hi-Res v3.48 ONNX 实测输入 [1,8,H,W]→输出 [1,3,4H,4W]（传感器每维 2×）；手写 WGSL 隐式 GEMM 无 ORT；FastCull 需补 Bayer 入口/σ 先验/memmap/TIFF-DNG 写出；内核可移植约 2–3 周，但权重不可随 MIT 分发（2026-11-22，ZCode）
- [denoise-integration](denoise-integration.md) — DXO PureRAW 降噪不可集成（无 SDK/CLI、EULA 禁逆向与代跑）；自带 ONNX 权重路线 PMRID(Apache-2.0)/NAFNet(MIT)，darktable RawNIND 为 GPL-3.0；降噪只挂全分辨率输出（2026-11-22，ZCode）
- [sony-sdm-weights-license](sony-sdm-weights-license.md) — Sony SDM/Extended Hi-Res ONNX 无单独许可，被 IEDT EULA 的 "data files" 兜住：禁拷贝/再分发/逆向，仅个人或内部商用 → 第三方复用权重无授权；darktable/RawTherapee/exiftool 均不支持；模型落盘加密；Extended Hi-Res = 单帧 2× 线性升采样，仅 α1II/α9III/α7RVI/α7V（2026-11-22，ZCode）

<!-- migrate-agent-memory:2026-10-17 -->

- [photocheck-xmp-feature](_archive/zcode/photocheck-xmp-feature.md) — 来源 zcode，已归档（2026-10-17）
- [rea-mcp-win-env](_archive/zcode/rea-mcp-win-env.md) — 来源 zcode，已归档（2026-10-17）
- [rust-port-plan](_archive/zcode/rust-port-plan.md) — 来源 zcode，已归档（2026-10-17）
- [sony-sdm-exraw-re](_archive/zcode/sony-sdm-exraw-re.md) — 来源 zcode，已归档（2026-10-17）
- [sony-viewer-xmp-rating](_archive/zcode/sony-viewer-xmp-rating.md) — 来源 zcode，已归档（2026-10-17）
- [viewer-raw-speed](_archive/zcode/viewer-raw-speed.md) — 来源 zcode，已归档（2026-10-17）
- [vision-use-flash-subagent](_archive/zcode/vision-use-flash-subagent.md) — 来源 zcode，已归档（2026-10-17）

- [dng-support 补记](dng-support.md#2026-10-24-bridge-实际安装脚本修订) — 本机 Bridge 2025/2026 旧 2.7.4 启动脚本拒绝 DNG；已备份更新为 2.7.7，需重启 Bridge；实际安装脚本各 19 项通过（2026-10-24，Codex）

- [raw-editing 最新修订](raw-editing.md#2026-10-24-最新修订本地-libraw完整基本调节与独立窗口) — 已替换Adobe后端：本地LibRaw/GPU七项调节、灰点吸管、独立窗口、DNG本地内嵌；214项Rust和6组真实窗口验证，现有插件目录exe/DLL已更新；上方Adobe条目为历史（2026-10-24，Codex）

- [公开发布 v2.8.0](release-and-macos.md#2026-10-25-公开发布-v280) — 已公开GitHub仓库并发布8附件；Windows/Apple Silicon/Intel同一标签CI、原生显影与自动RAW窗口均通过，Source/安装注册包与哈希已核对（2026-10-25，Codex）
