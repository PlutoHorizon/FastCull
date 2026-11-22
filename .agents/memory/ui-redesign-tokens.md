# UI 改版：设计令牌与排版

## 2026-11-10 P1 阶段：src/theme.rs 设计令牌

**事实**：`src/theme.rs` 定义了规范第一节的全部令牌（表面色 CANVAS/PANEL/PANEL_RAISED/RAIL/
BORDER/BORDER_STRONG、三级文字色 TEXT/TEXT_SECONDARY/TEXT_MUTED、语义色 ACCENT/ACCENT_SOFT/
SUCCESS/WARNING/DANGER、字号阶 FS_CAPTION 11 / FS_SMALL 12 / FS_BODY 13.5 / FS_LABEL 14 /
FS_TITLE 17 / FS_DISPLAY 24、间距 SP_XS 4 / SP_S 8 / SP_M 12 / SP_L 16、圆角 RADIUS_SM 4 /
RADIUS 8 / RADIUS_LG 12），`theme::apply(ctx)` 负责注册 egui visuals 与 text_styles；
`src/ui.rs` 的 `configure()` 现在只调 `theme::apply(ctx)` 加中文字体加载。

**坑（重要）**：`src/theme.rs` 必须放在**库 crate**（`src/lib.rs` 里 `pub mod theme;`），
不能只挂在二进制 crate。`examples/filter_bench.rs` 用
`#[path = "../src/filter_ui.rs"] mod filter_ui;` 把源码直接编进 example，此时
`crate::theme` 解析到 example 根而失败（E0432）。因此 UI 模块统一用 `use fastcull::theme;`，
`src/focus_overlay.rs` 用全路径 `fastcull::theme::font(...)`。

**适用条件**：任何 `src/*.rs` UI 模块（ui / folder_ui / focus_overlay / raw_editor / filter_ui）
里新增颜色或字号时。

**其余决定**：
- 全项目不再有硬编码字号：`grep -rn "FontId::proportional([0-9]\|\.size([0-9]" src/` 现在只有
  `src/folder_ui.rs` 里那处 `theme::font(size)`（size 来自令牌）。
- 旧常量 `PANEL`/`BG`/`RED` 删除，分别换成 `theme::PANEL`/`theme::CANVAS`/`theme::DANGER`；
  画布底色因此从 rgb(20,22,26) 变深到 rgb(16,18,21)。
- `style.spacing.indent` 保留 14（规范间距阶里没有这一档，改成 16 会动侧栏树缩进，属于改布局）。
- 规范第 2126-2140 行那段"去掉原生滑块外壳"的局部覆盖没有对应令牌，改为调用
  `theme::bare_slider(ui)`，把重复字面值收进主题模块，外观不变。
- 胶片条/网格上的旗标文字按旗标着色：P 用 SUCCESS、X 用 DANGER、其余 TEXT_SECONDARY。

**验证**：`cargo build`、`cargo build --release`、`cargo test`（15 个测试二进制全绿，0 失败）；
`cargo fmt --check` 干净（改前仓库也是干净的，改完必须重新格式化）。
640×480 下工具栏高度实测仍是 96px（断言上限 112px，`src/ui.rs` 的
`minimum_window_with_filters_keeps_toolbar_bounded` 未放宽）。
`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p1 1280x850 .agent-temp/ui-redesign/fixtures
target/release/fastcull features` → main_png=13 raw_png=4；
`python3 scripts/verify_feature_images.py .agent-temp/ui-redesign/cap-p1/main` 退出码 0。
（本机 Pillow 装在 user site，`python3 -I` 隔离模式读不到，跑该脚本不要加 `-I`。）

**更新人**：实现者-p1（ZCode workflow），2026-11-10。

## 2026-11-10 修订：占位符被 override_text_color 钉成正文色

**事实**：`visuals.override_text_color = Some(TEXT)` 会连带把输入框的提示文字（hint）也画成
正文色。egui 0.33.3 的 `TextEdit` 用 `hint_text.into_galley(...)` 排版，plain `&str` 走
`WidgetText::Text` 分支，颜色取 `style.visuals.override_text_color.unwrap_or(PLACEHOLDER)`
（`egui-0.33.3/src/widget_text.rs:747`）；随后 `painter.galley(pos, galley, hint_text_color)`
的第三个参数只是 fallback（`egui-0.33.3/src/painter.rs:562-575`：只有用 PLACEHOLDER 画的
部分才替换），而字形本身已被染成不透明的 TEXT，所以 `visuals.weak_text_color` 设不设都救不回来。

**修复**：`src/theme.rs` 增加 `theme::placeholder(text)`，返回 `RichText::new(text)
.color(TEXT_MUTED)` 组成的 `WidgetText`（RichText 自带颜色优先于 override_text_color，
`widget_text.rs:461-469`）；所有输入框提示改成 `theme::placeholder(...)`：
filter_ui.rs 的“搜索文件名…/开始日期/结束日期（含当天）/最小/最大”、folder_ui.rs 的
“筛选文件夹/文件夹名称/也可粘贴其他目标文件夹路径”、crop_ui.rs 的“预设名称”。
另外 `theme::apply` 增加 `visuals.weak_text_color = Some(TEXT_MUTED)`，让 `ui.weak()`
（计数、辅助说明）也落到规范文字色表上。

**验证**：重新出图后，搜索框内最亮像素实测 rgb(108,115,126)（= TEXT_MUTED）；
改动前同一处是 rgb(226,230,236)，P0 基线是 rgb(218,223,231)——与视觉验收的判断一致。

**坑**：`RichText` 放进 `hint_text()` 后字体不变（仍回落到 TextEdit 的 Body 字体），
但注意 `RichText` 的显式颜色会覆盖 `weak`/`strong` 语义，别用 `.weak()` 代替显式颜色。

**更新人**：实现者-p1（ZCode workflow），2026-11-10。


## 2026-11-10 P2 阶段：控件语义分级与星级升级

**事实**：`src/theme.rs` 新增角色构造 `primary()`（ACCENT 实心 + 白色粗体 + 无描边）、
`secondary()`（抬起灰底 + BORDER 描边，表面色留给主题 visuals 以保留 hover 反应）、
`secondary_selected()`（ACCENT_SOFT 底 + ACCENT 描边）、`ghost()` / `ghost_active()`
（透明底 + 无描边 + TEXT_SECONDARY / ACCENT 字）、`ghost_small()`（侧栏「重置」）。
工具栏按规范分级：拾取 = `primary` + SUCCESS 底、拒绝 = `primary` + DANGER 底（生效态加 2px 白描边）、
未标记 = secondary、单图/网格/对比 = secondary（选中 ACCENT_SOFT + ACCENT 描边）、◀▶ 与对焦点/适配 =
ghost、曝光四键 = secondary、更多 = ghost；`src/focus_overlay.rs` 的「对焦点」、
`src/filter_ui.rs` 的「重置」、顶栏「新窗口」同步改成对应角色。
`rating_control`（src/ui.rs）改成 5 颗实心 ★（已评 ACCENT / 未评 TEXT_MUTED，每颗 ≥22×22）+
右侧 FS_SMALL 读数，删掉单独的「0」按钮，保留「点同一颗星清零」与原悬停提示。
`control_group` 去掉固定宽度改自然宽度，分组之间用自绘 1px 竖线（两侧 8px）代替 `ui.separator()`。

**坑（egui 0.33.3）**：`Button::fill()` / `Button::stroke()` 是**全状态钉死**的
（`egui-0.33.3/src/widgets/button.rs:318-329`：先 `interact_selectable` 取状态 visuals，
再用 fill/stroke 覆盖），所以「同一按钮三态不同底色」做不到。ghost 的做法是
`.frame_when_inactive(false)`：静止时整块 frame 不画（透明），hover/按下时才画，
底色自然取主题的 `widgets.hovered.weak_bg_fill`（SURFACE_HOVER），而不是规范写的 PANEL_RAISED。
另外 `secondary()` 故意**不**钉 fill：钉了就没有 hover 反馈（主题里 inactive.weak_bg_fill=PANEL_RAISED、
hovered.weak_bg_fill=SURFACE_HOVER）。

**坑（布局）**：`ui.horizontal()` 放在 `ui.horizontal_wrapped()` 里**永远不会换行**——
子 ui 申请的是整段剩余宽度，父级的换行判断 `available < child_size` 恒为 false，
分组内容比剩余空间宽时只会被面板裁掉（640×480 时星级读数「0 星」被切掉就是这个问题）。
改成 `ui.horizontal_wrapped()` 更糟：嵌套 wrap 会让内层行高取外层 max_rect 的高度，
工具栏高度实测从 76px 暴涨到 233px，直接打破 ≤112px 的断言。最终做法是自然宽度 +
把星级组收紧（星星间距 0、读数按剩余空间自适应「N 星 / N / 不显示」）。

**证据**：640×480 下工具栏可用宽度 292pt，星级组自然宽 131.5pt（余 9pt）；
`cargo test` 15 个测试二进制全绿，`cargo fmt --check` 干净；
`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p2 1280x850 .agent-temp/ui-redesign/fixtures
target/release/fastcull features` → main_png=13 raw_png=4；
`python3 scripts/verify_feature_images.py .agent-temp/ui-redesign/cap-p2/main` 退出码 0
（exposure_channel_samples=2550，mean_error=0.394）。
注意：`scripts/verify_feature_images.py` 只在 1280×850 下成立，640×480 时画布太小，
采样数只有 36，脚本自带的 `len(errors)>100` 断言必然失败——不是回归。

**其余决定**：`更多` 菜单改用 `egui::containers::menu::MenuButton::from_button(theme::ghost("更多")).ui(...)`
（egui 源码里 `MenuButton::new(atoms)` 就是 `from_button(Button::new(atoms))`，
`Ui::menu_button` 走的是同一条路径，只是换了按钮外观；kittest 里合成点击测不出弹窗，
egui 自带的 `ui.menu_button` 在同一套合成事件下也弹不出来）。

**更新人**：实现者-p2（ZCode workflow），2026-11-10。

## 2026-11-10 P2 修订：视觉验收未过，星级改芯片 + 曝光读数用 WARNING

**事实（视觉验收反馈）**：第一版星级控件（裸 ★ 字形 + 未评 TEXT_MUTED）被判定为
工具栏第一行视觉权重最低的元素，比视图开关和导航箭头还不显眼；0 星照片上整组星级
没有任何 ACCENT 元素，与分级表「星级 ★×5 = primary（ACCENT 星的填充）」不符；
工具栏曝光读数「+0.00」用了正文白，而令牌表把 WARNING 明确列为曝光读数用途；
且静态截图无法确认每颗星的可点区域 ≥22×22（只看到 12×11.5 逻辑像素的墨迹）。

**修复**：
- 星级改成 5 个 **22×22 圆角芯片**（`min_size(Vec2::splat(22.))` + RADIUS_SM）：
  已评 = 实心 ACCENT 底 + TEXT 字形 ★；未评 = ACCENT_SOFT 底 + 1px ACCENT 描边 +
  TEXT_SECONDARY 字形 ★。这样 0 星时区域里也有精确的 ACCENT 像素，
  芯片轮廓本身把热区尺寸画了出来（截图实测每个芯片 22.0 逻辑像素宽）。
  未评字形由 TEXT_MUTED 提到 TEXT_SECONDARY（放在 ACCENT_SOFT 底上时 TEXT_MUTED 只有 2.3:1）。
- 工具栏曝光读数改成 `ui.monospace(RichText::new(...).color(theme::WARNING))`
  （`Ui::monospace` 收 `impl Into<RichText>`，显式颜色不会被字体族覆盖）。

**证据**：`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p2 1280x850
.agent-temp/ui-redesign/fixtures target/release/fastcull features` → main_png=13 raw_png=4；
`python3 scripts/verify_feature_images.py .agent-temp/ui-redesign/cap-p2/main` 退出码 0。
像素实测（1280×850 的 00.png，2x）：0 星时星级区域 ACCENT 精确像素 1070 个、
ACCENT_SOFT 6953 个；4 星时同一区域 ACCENT 6744 个、TEXT 810 个；
曝光读数「+0.00」区域 WARNING(220,185,110) 精确像素 264 个、TEXT 0 个。
640×480 复测仍为 4 行、星级读数自动收成「0」不裁切。
`cargo test` 15 个测试二进制全绿，`cargo fmt --check` 干净。

**坑**：星级芯片把组宽从 110 涨到 118 逻辑像素（5×22 + 4×2），
640×480 时星级组可用宽 140.5pt，读数因此走紧凑形式「N」而不是「N 星」——
`rating_control` 里按 `ui.available_width()` 自适应，不要写死。

**更新人**：实现者-p2（ZCode workflow），2026-11-10。

## 2026-11-10 P2 补记：验收评语引用的是修复前的截图

**事实**：P2 第二轮修复后，验收评语又重复了第一轮的四条问题（星形 rgb(108,115,126)、
字形 12×11.5、无 ACCENT、曝光读数正文白），但验收方自己在
`.agent-temp/ui-redesign/shots/p2/1280x850/main/00.png`（20:01 生成）里截的图
与修复后的 `cap-p2/main/00.png` **逐字节相同**（`cmp` 一致），
实测该图星级区 ACCENT 精确像素 1120 个、ACCENT_SOFT 7793 个、TEXT_MUTED 0 个，
每个芯片包围盒 22.0×26.0 逻辑像素；曝光读数处 WARNING 264 个像素。
也就是说评语里的数值来自**修复前**那一版截图（第一轮 P2 的裸 ★ 字形版本）。

**坑**：`.agent-temp/ui-redesign/review2/` 里的放大裁图是 P1 阶段（18:40）留下的，
直接拿它评审会看到过期的工具栏。已在 20:04 用验收方自己的 shots/p2 图重出
`review2/{00,02}_toolbar.png`（整行）、`{00,02}_stars.png`（星级 6 倍放大，
能看到 22×22 芯片轮廓）、`{00,02}_expo.png`（曝光读数放大，WARNING 黄色）。

**适用条件**：任何一轮改版结束后，出图前先确认 `review2/` 的裁图与
`shots/<阶段>/` 的整图同源，再交给评审。

**更新人**：实现者-p2（ZCode workflow），2026-11-10。

## 2026-11-10 P3 阶段：顶栏压成单行 56px + 照片加框（圆角/描边/阴影/内边距）

**事实**：顶栏 `egui::TopBottomPanel::top("hud")`（src/ui.rs）改成**单行 56px**：
frame 内边距 `symmetric(16, 8)` + 一行固定 40px（`ui.allocate_ui_with_layout(Vec2::new(w, 40.),
Layout::left_to_right(Align::Center), …)`），左「新窗口」+ `4/4（共 4）`（FS_BODY + TEXT_SECONDARY），
右「文件名（FS_TITLE 粗体）· EXIF 摘要（FS_SMALL + TEXT_SECONDARY）」同一行；
相机/镜头/拍摄时间仍只在悬停提示里。顶栏与画布之间的 1px BORDER 分隔线是 egui
`TopBottomPanel` 自带的 `show_separator_line`，颜色取 `visuals.widgets.noninteractive.bg_stroke`
（theme::apply 里已设成 1px BORDER），不用自己画。

**照片加框的实现**：`paint_adjusted` 拆成 `photo_layout()`（只算屏幕矩形 + 4 个 uv，
**公式与改版前逐字相同**）与 `paint_photo()`（画 mesh + GPU 曝光预览）。
单图 `central()` 与对比 `compare_canvas()` 都是：画布区域 `rect.shrink(theme::SP_M)`（12px）
当照片的活动区域 → `paint_photo_shadow()`（epaint `Shadow{offset:[0,2], blur:12}`，
必须在照片**之前**画）→ `paint_photo()` → `paint_photo_edge()`（圆角 + 1px BORDER 描边）。
网格/胶片条仍走 `paint_adjusted` 原路径，几何没动；只有格子的圆角从 5 改成
`theme::RADIUS_SM`（4）。

**坑 1（egui 0.33.3）**：egui 不能把带自定义 uv 的贴图网格裁成圆角矩形
（`egui::Image` 的 uv 是 Rect，且旋转时禁用圆角），所以圆角是用「画完照片后，
再用画布色把四个角盖掉」实现的。盖角多边形必须**以方角顶点为第 0 点**，
epaint `fill_closed_path` 是从第 0 点扇形填充的；它只会在 `cw_signed_area < 0` 时整体反转路径
（`epaint-0.33.3/src/tessellator.rs:765-780`），所以镜像的两个角要**反转圆弧点的顺序**，
而不是把顶点挪到末尾——挪走第 0 点会让扇形从圆弧上的点出发，填出穿帮的三角形。

**坑 2（GPU 曝光预览）**：`preview.paint(painter, canvas, rect, …)` 的 `canvas` 必须传
**整块画布**，不能传内缩 12px 的照片区域。传内缩区域时，100% 放大的照片超出该区域的部分
会被 GPU pass 的 viewport 裁掉，只剩没做曝光的 CPU 网格。照片的屏幕矩形本身由
`photo_layout` 用内缩区域算出，mesh 与 GPU quad 用的是同一个矩形，两者仍然重合。

**坑 3（对比联动）**：对比模式里 `compare_sizes[side]`、`fit_zoom` 必须一起换成内缩 12px 后的
区域，否则「适配」时的绘制尺寸与 `zoom_compare()` 里的 `fit[]` 不一致，联动缩放会跳一下。
缩放/平移的**增量公式没动**（`Compare::scale` / `translate` 一字未改）。

**坑 4（像素回归）**：`scripts/verify_feature_images.py` 把「不是画布底色且三通道都在
20~190 之间」的像素当成照片像素。1px BORDER 描边（48,53,60）正好落在这个区间，
而它不随曝光变化 → 每个采到的描边像素贡献约 22.7 的误差。1280×850 下样本多，
mean_error 由 P2 的 0.394 升到 **0.915**（阈值 2.0，仍通过）；
**640×480 下照片本来就小**，描边像素占了有效样本的大半，mean_error 变成 12.055 直接失败
（P2 在 640×480 也失败，但原因是采样数只有 36 < 100 的断言）。规范第五节第 3 条
只要求 1280×850 出图通过，实测通过。

**其余决定**：规范第一节令牌表写「RADIUS_LG=12（照片）」，第三节 3.5 写「6px 圆角」，
自相矛盾；按用户明确要求取 **6px**，为此在 `theme.rs` 另加 `RADIUS_PHOTO = 6.`
（`RADIUS_LG` 保留给以后用），并加 `theme::SHADOW`（黑色 47% 透明）。
EXIF 摘要按规范示例收紧排版：`ISO 2000 · f/4.0 · 1/100s · 70mm`（原来 `1/200 s`、`50 mm`）。

**证据**：`cargo build`、`cargo test` 全部测试二进制全绿（70 + 125 + 其余，0 失败），
`cargo fmt --check` 干净；
`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p3 1280x850 .agent-temp/ui-redesign/fixtures
target/release/fastcull features` → main_png=13 raw_png=4；
`python3 scripts/verify_feature_images.py .agent-temp/ui-redesign/cap-p3/main` 退出码 0
（exposure_channel_samples=2592，mean_error=0.915，两个对比画布都可见）。
像素实测（cap-p3/main/05.png，2x）：顶栏 PANEL 从 y=0 到 y=109，y=110-111 是 BORDER 分隔线，
y=112 起是画布 → 顶栏正好 56 逻辑像素；照片左边框在 x=1212，照片左边与画布区左沿之间
留 12 逻辑像素；照片圆角实测半径 12 物理像素（6 逻辑像素）✓。
640×480 复测：顶栏仍单行，工具栏 3 行不重叠（`minimum_window_with_filters_keeps_toolbar_bounded` 未放宽）。
注意本机 Pillow 装在 user site，`python3 -I` 读不到，跑该脚本不要加 `-I`。

**新增回归测试**：`src/ui.rs` 的 `focus_overlay_projects_onto_the_padded_photo_rect`——用 `photo_layout` 算出内缩 12px 后的照片矩形，断言照片留在内缩区域内、限制轴上正好留 12px，并断言 `fastcull::photo_focus::project()` 把焦点 uv 投到**照片自己的屏幕矩形**上（不是画布）。

## 2026-11-10 P3 修订：640×480 工具栏溢出与网格格子被压扁

**事实（视觉验收反馈）**：640×480 的网格态截图里，工具栏第三行右边缘把曝光控件
（−⅓ / +⅓ / +1 / +0.00）裁掉了；网格格子被压成约 45 逻辑像素宽、220 高的窄条，
缩略图缩成小色块，文件名（"a-balloo"）和星级被切掉。两者在 P2 就存在，不是 P3 引入的。

**坑（egui 0.33.3，嵌套组无法换行）**：`ui.horizontal()` 申请的是**整段剩余宽度**
（`horizontal_with_main_wrap_dyn` → `initial_size = vec2(available_size_before_wrap().x, …)`），
而父级换行判断是 `available_size.x < child_size.x`（`layout.rs:547`），
所以 `available < available` 恒为 false → **嵌套组永远不会换行**；
内容比剩余宽度宽时不是换到下一行，而是被组的 max_rect 裁掉。
父级光标只按子 ui 的 **min_rect**（内容实际宽度）前进（`ui.rs:2515` scope_dyn），
所以组后面的控件仍能跟在同一行。

**做法**：先把这一组拿到屏幕外量一次宽度，再按这个宽度摆放：

```rust
fn measure_group(ctx, spacing, id, build) -> f32 {
    // Ui::new + max_rect 放在 (-20000,-20000)，clip_rect 也在那里 → 画出来也看不见
    let mut ui = egui::Ui::new(ctx.clone(), Id::new(id),
        egui::UiBuilder::new().max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .style(ctx.style()));
    *ui.spacing_mut() = spacing.clone();
    build(&mut ui);
    ui.min_rect().width() + 1.
}
```

再用 `ui.allocate_ui_with_layout(Vec2::new(measured_width, interact_size.y), …)` 摆放：
这时 desired size = 实测宽度 → 父级 `available < 实测宽度` 成立 → 整组换到下一行 ✓。
组里的点击处理要拆出来（`fn exposure_controls(&self, …) -> Vec<Response>` 只建控件，
点击由调用方处理），量宽的那一趟才能用同一个 &self 构造器。
曝光组前面的 1px 分隔线也放进组里，组换行时分隔线跟着走，不会留下悬空的竖线。

**网格**：列数改成跟随可用宽度 `cols = (available/140).floor().clamp(1,6)`，
格子高 `= (width*1.37).clamp(150,220)`，行高 = 格高 + 4（滚动偏移也要用同一个行高）。
1280×850 下 available≈992 → 仍是 6 列、格高 220（与改版前逐像素一致）；
640×480 下 available≈313 → 2 列、格宽 152、格高 209，缩略图和文件名都完整。

**证据**：`cargo test` 15 个测试二进制全绿（含 `minimum_window_with_filters_keeps_toolbar_bounded`，
工具栏在 640×480 下 4 行、高度仍在 112px 断言内），`cargo fmt --check` 干净；
`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p3-640 640x480
.agent-temp/ui-redesign/fixtures target/release/fastcull features` → main_png=13 raw_png=4，
实测网格态 640×480 第三行只剩「单图 E 网格 G 对比 C | ◀ ▶」、第四行完整显示
「| 曝光 −1 −⅓ +⅓ +1 +0.00」，网格 2 列、文件名 "a-balloons.jpg" 完整；
`bash scripts/ui_capture.sh .agent-temp/ui-redesign/cap-p3 1280x850 … features` →
`python3 scripts/verify_feature_images.py .agent-temp/ui-redesign/cap-p3/main` 退出码 0
（exposure_channel_samples=2592，mean_error=0.915，与本轮改动前一致）。
640×480 下该脚本仍然失败（n=54 < 100、mean_error=12.055）：原因是 1px BORDER 描边像素
被脚本当成"应随曝光变亮的照片像素"（P2 在 640×480 也失败，采样数只有 36）。

**更新人**：实现者-p3（ZCode workflow），2026-11-10。

## 2026-11-10 修订：P2/P3/P4 手动收尾（人工改版，非工作流）

**事实**：P0–P3 由工作流执行，P4 及两处评审未通过项由人工补完。

- **星级控件**：未评的星改为 `PANEL_RAISED` 底 + `TEXT_MUTED` 字 + `BORDER` 描边；已评的星是 `ACCENT` 底 + 白字。
  之前未评的星也是 `ACCENT_SOFT` 底 + `ACCENT` 描边，0 星时看起来像五个已选中的方块。
- **拾取/拒绝**：新增 `theme::flag_action`，静置时是同色系的暗色底 + 彩色字，
  只有真正打上的那个旗标才是实心 + 白圈。两个旗标互斥，常亮实心会让人以为三个旗标都设了。
- **色块组换行**：色块是嵌套 group，egui 不会给嵌套 group 换行，640px 窗口下五个色块被裁掉两个。
  改用与曝光组相同的 `measure_group` + `control_group_sized` 组合，测量后再按自身宽度放置。
- **状态提示底衬**：新增 `status_plate()`，压在照片上的提示文字改成半透明黑圆角底 + 内边距。
- **导航图标**：`RAW 编辑` 原本画的是折线图（像统计面板），改成相框 + 圆点 + 山峰的图形。
- **窄窗口侧栏**：窗口宽度 < 780px 时侧栏默认宽度 260 → 200，否则 640px 窗口里侧栏占 40% 宽度。
- **筛选侧栏摘要**：`filter_ui.rs` 新增 `facet_summary()`，折叠态的筛选项在标题后用强调色显示当前条件
  （如「星级 3 星」「ISO 100–800」）。egui 0.33 的 `CollapsingHeader` 没有自定义标题接口，
  只能用 `egui::text::LayoutJob` 拼两段不同颜色的文字。

**坑**：`egui::CollapsingHeader` 在 0.33 没有 `show_header_ui`，`show_header` 是 `CollapsingState` 的方法；
`RichText` 也没有 `append`，多段文字要用 `egui::text::LayoutJob` + `RichText::append_to`。
`egui::LayoutJob` 的路径是 `egui::text::LayoutJob`，不在 `egui::epaint` 下。

**顺带修掉的 clippy 警告**：`photo_layout` 原来 8 个参数（超过 clippy 的 7 个上限），
把 `zoom`/`pan`/`ppp` 收成一个 `PhotoView` 结构体，降到 6 个。

**验证**：`cargo build --release`、`cargo test`（15 个测试二进制全绿）、`cargo clippy --release` 无警告；
`bash scripts/ui_capture.sh <out> 1280x850 <fixtures> target/release/fastcull features` → main_png=13；
`python3 scripts/verify_feature_images.py <out>/main` 退出码 0（2592 采样点，平均误差 0.915）。
640x480 下工具栏实测 95px，仍低于 `minimum_window_with_filters_keeps_toolbar_bounded` 的 112px 上限。

**更新人**：ZCode 人工改版，2026-11-10。

## 2026-11-10 修订：大幅视觉重构（人工改版）

**事实**：在令牌体系之上做了第二轮视觉重构，重点是让照片成为主角、让控件有层级。

- **画布渐变**：`CANVAS_TOP rgb(19,21,25)` → `CANVAS_BOTTOM rgb(7,8,10)`，20 条横带插值。
  `paint_canvas()` 绘制，`canvas_color_at()` 供圆角补画取色。
- **照片双层阴影**：宽的环境阴影（blur 36, offset 10）+ 紧的接触阴影（blur 9, offset 2）。
  单层阴影看起来像灰边而不是景深。
- **顶栏重排**：64px 三栏 —— 左「＋ 新窗口 + 计数（当前张数用 ACCENT 强调）」、
  中「文件名居中，FS_HERO 19px 加粗」、右「拍摄参数摘要」。
- **导航栏**：图标放在 36px 圆角方块上，选中时填 ACCENT_SOFT；图标描边 1.9px；
  未选中标签用 TEXT_SECONDARY（TEXT_MUTED 太暗）。
- **筛选侧栏**：星级/旗标/颜色标签三个高频项改为**芯片行**（`quick_chips()`），
  计数以更小的暗色字跟在标签后；其余八项仍是复选框列表，且保持折叠时的懒加载。
- **胶片条**：独立表面色 `PANEL_SUNK`，当前格 2px ACCENT 边框。
- 控件高度 26 → 28px；主题圆角 RADIUS 8。

**坑（重要）**：
1. `egui::Frame::new()` 的默认 `inner_margin` 是 8，不是 0。CentralPanel 用默认 Frame 时，
   画布渐变铺不到窗口边缘，会在四周留一圈扁平的 frame 色（像素回归因此误判画布边界）。
   画布面板必须显式 `inner_margin(Margin::ZERO)`。
2. `scripts/verify_feature_images.py` 原来靠「取一个全局背景色」定位画布，画布一变渐变就失效。
   现在改成：逐行取右侧边缘色作为该行的画布色（`is_canvas`），并逐行逐列找出照片真实范围；
   同时排除阴影像素（阴影是画布色整体压暗，各通道比例相同）。
3. `egui` 0.33 里 `Color32::lerp_to` 不存在，是 `lerp_to_gamma`；`ui.centered` 不存在，
   是 `ui.centered_and_justified`；`egui::LayoutJob` 的路径是 `egui::text::LayoutJob`。
4. `egui::Button::new(WidgetText)` 的生命周期是 `'static`，helper 的返回类型要写
   `egui::Button<'static>`，写 `<'_` 会报缺少生命周期。

**验证**：`cargo build --release`、`cargo test`（15 个二进制全绿）、`cargo clippy --release` 无警告；
640x480 工具栏断言仍过；`scripts/verify_feature_images.py` 对新版（渐变画布）与旧基线（扁平画布）都通过。

**更新人**：ZCode 人工改版，2026-11-10。

## 2026-11-10 修订：平台字体、星级重画与材质收敛

**事实**：
- 字体改为**平台自有字面**：`platform::latin_font_paths()` / `mono_font_paths()` / `cjk_font_paths()`
  分平台给出（macOS：SFNS.ttf / SFNSMono.ttf / PingFang.ttc；Windows：segoeui.ttf / consola.ttf / msyh.ttc）。
  比例字族顺序为 [latin, cjk]，等宽字族首位插入平台等宽字面。
  之前拉丁文字用的是 egui 自带的 Ubuntu-Light，和中文的苹方/雅黑并排时明显是两套字，这就是"廉价感"的来源。
- 星级改为 `draw_star()` 自绘五角星（凸多边形填充 / 折线描边），**不再有任何方框**；
  已评为 `theme::STAR` 金色 rgb(238,187,78)，未评为主题灰描边，点击区是不可见的 22x22 透明按钮。
  胶片条/网格里的星级同样改成自绘金色星标。
- 按钮去描边：`secondary` / `secondary_selected` / `secondary_rich` 全部改为「填充面 + 无描边」，
  主题里 hovered/active 的 `bg_stroke` 也设为 NONE；圆角统一为 RADIUS 8px。
- 材质分层：画布顶部与工具栏顶部各一条 1px 发丝线（`panel_hairline`），
  顶栏/工具栏的 frame 内边距改为 0，发丝线画在面板内容区上沿。

**坑**：`egui::FontData` 没有公开的已解析字形字段，想判断字体是否可用只能在
`ctx.run()` 之后用 `ctx.fonts_mut(|f| f.has_glyphs(&font_id, "文本"))` 查。
`egui::Button::new(WidgetText)` 的生命周期是 `'static`。
egui 0.33 的 `CollapsingHeader` 没有自定义标题接口（只有 `CollapsingState::show_header`），
多段文字要用 `egui::text::LayoutJob` + `RichText::append_to`。

**验证**：新增 `ui::tests::the_interface_draws_with_the_platform_faces`——
平台字面存在时拉丁字面必须排在比例字族首位，且拉丁与中文字形都必须可绘制。
`cargo test` 15 个二进制全绿；`cargo clippy --release` 无警告；
`scripts/verify_feature_images.py` 通过（60249 采样点，平均误差 0.876）。

**更新人**：ZCode 人工改版，2026-11-10。

## 2026-11-11 修订：主窗口改为浮动圆角卡片

**事实**：
- 主窗口的五个区域（导航栏、顶栏、筛选侧栏、工具栏、胶片条）改成**悬浮在深色底上的圆角卡片**，
  不再是贴到窗口边缘的面板。每张卡片有 10px 外间距（`theme` 里的 `GAP`/`PAD`）和柔和投影
  （`epaint::Shadow`，offset [0,6]、blur 24）。
  实现见 `src/ui.rs` 的 `Card { outer, inner }` + `paint_card()`：`outer` 是卡片到相邻物的间距，
  `inner` 是卡片边缘到内容的内边距，`Card::rect()` 由内容矩形反推卡片矩形。
- 画布底色改为整窗竖向渐变（`CANVAS_TOP` rgb(20,22,27) → `CANVAS_BOTTOM` rgb(8,9,12)），
  在 `ctx.layer_painter(egui::LayerId::background())` 上一次画完；所有面板 frame 设为透明。
  **中间面板必须 `inner_margin(Margin::ZERO)`**，否则 `Frame::new()` 默认 8px 内边距会在窗口边缘留一圈平色。
- 卡片顶部加了一条**受光边缘**（`paint_card_rim_light()`，颜色 `SURFACE_HI` rgb(45,51,60)）：
  同一填充色 + 等宽描边只会像"画出来的方框"，顶边受光才像实体。
  epaint 0.33 没有 `epaint::Path`，`PathShape.points` 是 `Vec<Pos2>` 多段线，用 `egui::Shape::line()` 画。
  圆弧角度：左上角从 180° 扫到 270°、右上角从 270° 扫到 360°，角度写反会让圆角变成折线并留下钩状毛刺。
- 卡片对齐规则：顶栏卡片左边与筛选侧栏卡片左边重合；导航栏卡片与顶栏卡片同时从顶部 10px 开始。

**坑**：`egui::Frame::new()` 默认 `inner_margin` 是 8，不是 0。

**验证**：`cargo test` 15 个二进制全绿；`cargo clippy --release --all-targets` 无警告；
`scripts/verify_feature_images.py` 通过（53589 采样点，平均误差 0.916）；640x480 最小窗口布局不塌。

**更新人**：ZCode 人工改版，2026-11-11。

## 2026-11-11 修订：收侧栏（状态栏）与胶片条材质统一

**事实**：
- 筛选侧栏底部新增**状态栏**（`SIDEBAR_FOOTER = 56`）：一条发丝线 + 「显示 N / M 张」+ 右对齐的「重置」
  + 一行彩色计数「拾取 n · 拒绝 n · 评分 n」（复用 SUCCESS / DANGER / STAR 三种语义色）。
  侧栏卡片用 `ui.scope_builder(UiBuilder::new().max_rect(rect), …)` 切成上（可滚动的筛选树）和下（状态栏）两块，
  空白不再留着。「重置」从筛选树标题行移到这里（`filter_ui.rs` 的 `FilterEditor::show` 不再画按钮）。
- 胶片条缩略图改成和画布照片**同一套材质**：圆角 6（`RADIUS_PHOTO`）+ 1px `LINE` 描边 + 柔和投影。
  胶片条里缩略图不再垫 `SURFACE` 底板（直接站在卡片上），网格里才垫 `SURFACE_2` 面板放文件名。
  选中环、投影、圆角盖角都跟随**照片本身**而不是外框盒子，所以竖构图放在横向格子里环仍然贴着照片。
- `paint_photo_edge` 拆成通用函数 `round_image_corners(painter, rect, radius, background)`：
  `background` 闭包按角返回 `Option<Color32>`，返回 `None` 就不盖那个角（画布上的照片靠这个保留贴边的方角）。

**坑**：
- egui 0.33 的 `RichText::append_to` 要四个参数：`(job, &Style, FontSelection, Align)`。
- `Ui::allocate_ui_at_rect` 已废弃，用 `ui.scope_builder(UiBuilder::new().max_rect(rect), …)`。
- **闭包捕获**：在面板闭包里调 `self.some_method(&mut self)` 会把整个 `self` 捕获进去，
  和外面借 `self.paths` 造出来的 `rows` 冲突（E0500）。解法：把要用的数据在闭包外算好，
  状态栏做成**自由函数**只收数据。
- 想在画图前知道照片实际落在哪，得直接调 `photo_layout()` 拿 `layout.rect`，
  再用 `paint_photo()` 画；`paint_image` / `paint_adjusted` 只是包装，画完才拿不到位置，已删除。

**验证**：`cargo test` 15 个二进制全绿；`cargo clippy --release --all-targets` 无警告；
`scripts/verify_feature_images.py` 通过（53589 采样点，平均误差 0.916）；640x480 布局不塌。

**更新人**：ZCode 人工改版，2026-11-11。

## 2026-11-11 修订：星星的填充是错的（凹多边形不能用 convex_polygon）

**事实**：
- `draw_star()` 之前用 `egui::Shape::convex_polygon(points, …)` 填充五角星，**形状是错的**：
  五角星是**凹多边形**，而 epaint 的 `fill_closed_path` 是从第一个点**扇形三角化**
  （`idx_inner + 2*(i-1), idx_inner, idx_inner + 2*i`），扇心在外尖上，
  五个凹口会被横跨的三角形糊成五片"翅膀"，点亮时形状像被打翻的。
  现在改成**绕中心的一圈三角形**：中心点 + 10 个星角点，`mesh.indices` 里连 10 个 (center, i, i+1)。
- 未点亮的轮廓星之前也少一条边：`egui::Shape::line` 的 `PathShape` 是 `closed: false`，
  从最后一个内角回到第一个外尖的那条边没有画出来，顶点处有个缺口。
  现在把首点追加到末尾闭合。
- **坑**：egui 里凡是凹多边形都不能用 `convex_polygon` 填充；
  epaint 没有耳切三角化，只有扇形三角化。圆角照片的"盖角"用的是同一个 API，
  那里能work是因为扇形区域是凸的四分之一圆。

**验证**：`cargo test` 15 个二进制全绿；`cargo clippy --release --all-targets` 无警告；
`scripts/verify_feature_images.py` 通过（53589 采样点，平均误差 0.916）。

**更新人**：ZCode 人工改版，2026-11-11。
