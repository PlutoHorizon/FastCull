# FastCull 应用图标

2026-11-08，Codex。以用户选定的 A「光圈相册」为基础，使用内置 imagegen 生成并迭代。

设计：四张同尺寸圆角照片向左上紧密平移叠放；前景为白色光圈相册，中间两层为细线，后层逐层降低对背景的对比度。后层上边和左边的长度按同尺寸照片的平移关系与前景对应。相册边框转角采用圆弧，光圈内部保留六边形结构。绘图主体放大并居中，减少四周黑边；深灰圆角底，圆角外透明。

- `fastcull-master.png`：正式 RGBA 原稿。
- `fastcull.png`：256 px，嵌入主窗口及 RAW 编辑窗口。
- `fastcull.ico`：16、20、24、32、40、48、64、128、256 px，嵌入 Windows EXE。
- `fastcull.icns`：macOS 应用包图标。
- 重新转换：`python scripts/build-icons.py`（需要 Pillow）。程序构建本身无需 Python/Pillow；Windows 使用 Windows SDK 的 `rc.exe` 编译资源，可通过 `RC` 指定路径。

最终图像编辑提示词：

> Uniformly scale up and recenter the entire aperture-photo-stack emblem to occupy approximately 88–90% of the charcoal tile, with minimal safe padding and no clipping. Preserve rounded photo corners, the circular shutter with six blades and central hexagon, four equally sized photo layers, matching rear edge lengths, thin middle strokes and stepped gray fade. Keep the charcoal tile and transparent outer corners unchanged.

Windows 构建自动嵌入 ICO；窗口通过 `graphics::native_options` 使用 PNG。macOS 打包脚本复制 ICNS 并设置 `CFBundleIconFile`；macOS 真机显示需在 Mac 上验收。
