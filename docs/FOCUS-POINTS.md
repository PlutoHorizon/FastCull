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

标签依据：[ExifTool Sony](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Sony.pm)、[Nikon](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Nikon.pm)、[Canon](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Canon.pm)。本地实现为只读 Rust 解析器。

维护验证入口：`cargo test photo_focus`、`cargo test --bin fastcull raw_view::tests`、`cargo test --bin fastcull raw_canvas_single_click`。`cargo run --example focus_probe -- <照片路径>` 可只读输出实际记录。
