# 照片对焦点与对焦框

选片单图和对比界面的工具栏、RAW 编辑右侧面板提供“对焦点”切换按钮，快捷键为 O。开启时按钮显示强调色，关闭时恢复默认颜色。默认关闭；只在开启且照片显示时按需后台读取，不调用 ExifTool，不改照片或旁车。

绿色小标记表示相机只记录了一个位置；有真实区域宽高时绘制绿色框。标记不会被当成相机记录的 AF 框尺寸。叠加位置跟随 EXIF 的八种方向、XMP 裁切与拉直、缩放和拖动；裁切外的标记不显示。RAW 裁切工具内暂不叠加，应用裁切后恢复。

支持的记录：

| 数据 | 显示方式 |
| --- | --- |
| Sony MakerNote 0x202a，版本 1 | 优先显示相机相位对焦点，可为多个 |
| Sony FocusLocation 0x2027 | 没有可用相位坐标时显示回放位置，并提示它可能是默认中心 |
| Nikon AFInfo2 0100 / 0101 的反差对焦坐标 | 对焦区域 |
| Nikon AFInfo2 0300 / 0301 / 0400 / 0401 / 0402，坐标有效标记为 1 | 对焦区域 |
| Canon AFInfo2 0x26 | 仅显示合焦位掩码中有效的框 |

支持上述记录的原始 TIFF RAW 和 JPEG EXIF。DNG 只有仍带可解析的标准 MakerNote 时才可能有坐标；转换、全景、去噪或导出可能移除原始记录。没有坐标、仅记录对焦点编号的布局、未知版本或损坏元数据均提示“无可用对焦坐标（未记录或暂不支持）”，不从照片内容猜测。Sony 回放位置不能作为合焦成功的证明。

解析器使用定位读取，预算 2 MiB；限制目录数量、条目数量、坐标数量和长度。原始图像数据不会因读取对焦信息而解码。

## 100% 切图时自动定位

选片工作台左侧边栏底部的“设置”菜单提供“100% 切图时自动定位对焦点”，默认关闭，修改后自动保存。颜色外观（浅色、深色、跟随系统）也在此菜单中。

开启后，在单图 100% 或对比画面恰为 100% 时切到本次尚未浏览的照片，会将可见对焦点的中心移到画布中央；多个点取裁切范围内各点中心的平均位置，靠近照片边缘时限制移动以避免留白。对比视图只定位新切换的画面，参考照片保留原位。未记录、暂不支持或对焦点全部位于裁切外时，沿用切换前的位置。

本次已在单图或对比画面浏览的照片不再触发自动定位；网格缩略图和后台预读不计为浏览。已保存的每张照片放大位置优先恢复，浏览记录随本次选片窗口结束而清空。手动拖动、切换倍率或进入裁切会取消尚未完成的定位。此功能无需开启“对焦点”叠加显示，也不写入照片或旁车文件。

标签依据：[ExifTool Sony](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Sony.pm)、[Nikon](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Nikon.pm)、[Canon](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Canon.pm)。本地实现为只读 Rust 解析器。

维护验证入口：`cargo test photo_focus`、`cargo test --bin fastcull raw_view::tests`、`cargo test --bin fastcull raw_canvas_single_click`。`cargo run --example focus_probe -- <照片路径>` 可只读输出实际记录。
