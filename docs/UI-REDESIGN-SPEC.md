# FastCull 界面改版规范

本文是 FastCull 原生 GUI（eframe/egui）的改版依据。所有阶段都必须同时满足
[不可破坏的约束](#五不可破坏的约束) 和本阶段自己的验收标准。

改版只动**视觉**，不动**行为**：快捷键、状态机、评分/旗标/颜色标签语义、对比联动、
裁切几何、RAW 显影参数、XMP 读写一律保持原样。

---

## 一、设计令牌

新增 `src/theme.rs`，把当前散落在 `src/ui.rs:15-24` 的 `PANEL`/`BG`/`RED` 三个常量，
以及 `src/ui.rs:3424` `configure()` 里的内联色值，全部收敛成一套令牌。`configure()`
改为调用 `theme::apply(ctx)`。

### 表面色（由深到浅）

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `CANVAS` | `rgb(16,18,21)` | 照片画布底色，比现在的 `BG` 更深，让照片"浮"起来 |
| `PANEL` | `rgb(26,29,34)` | 侧栏、工具栏、胶片条、顶栏 |
| `PANEL_RAISED` | `rgb(34,38,44)` | 控件底、卡片、输入框 |
| `RAIL` | `rgb(21,23,27)` | 左侧导航栏 |
| `BORDER` | `rgb(48,53,60)` | 1px 分隔线、控件描边 |
| `BORDER_STRONG` | `rgb(72,80,90)` | 强描边、选中态外框 |

### 文字色（三级）

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `TEXT` | `rgb(226,230,236)` | 正文、标题 |
| `TEXT_SECONDARY` | `rgb(158,165,176)` | 次要说明、EXIF |
| `TEXT_MUTED` | `rgb(108,115,126)` | 占位符、计数、禁用 |

### 语义色

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `ACCENT` | `rgb(94,156,232)` | 选中、主操作、对焦点 |
| `ACCENT_SOFT` | `rgb(35,56,82)` | 选中态底色 |
| `SUCCESS` | `rgb(104,172,104)` | 拾取 |
| `WARNING` | `rgb(220,185,110)` | 警告、曝光读数 |
| `DANGER` | `rgb(201,79,79)` | 拒绝、错误 |

### 字号阶

替换掉全项目硬编码的 `FontId::proportional(N)`（当前有 11/12/13/14/15/16/17/18/22/24
共 10 种）和 `RichText::size(N)`。

| 令牌 | 字号 | 用途 |
| --- | --- | --- |
| `FS_CAPTION` | 11 | 胶片条缩略图上的旗标/星标 |
| `FS_SMALL` | 12 | 计数、说明、辅助文字 |
| `FS_BODY` | 13.5 | 控件文字（主题默认 Button/Body） |
| `FS_LABEL` | 14 | 正文、列表项 |
| `FS_TITLE` | 17 | 文件名、面板标题 |
| `FS_DISPLAY` | 24 | 弹窗标题 |

### 间距与圆角

`SP_XS=4` `SP_S=8` `SP_M=12` `SP_L=16`；
`RADIUS_SM=4`（输入框、色块）`RADIUS=8`（按钮、卡片）`RADIUS_LG=12`（照片）。

### 主题注册

`theme::apply(ctx)` 里设置：

- `visuals.panel_fill = PANEL`、`window_fill = PANEL_RAISED`、`extreme_bg_color = CANVAS`
- `override_text_color = Some(TEXT)`
- `selection.bg_fill = ACCENT_SOFT`、`selection.stroke = 1px ACCENT`
- `widgets.inactive.bg_fill = PANEL_RAISED`、`bg_stroke = 1px BORDER`
- `widgets.hovered.bg_fill = rgb(46,52,60)`、`bg_stroke = 1px ACCENT`
- `widgets.active.bg_fill = ACCENT_SOFT`、`bg_stroke = 1px ACCENT`
- 四种状态的 `corner_radius = RADIUS_SM`
- `text_styles`：Body = `FS_BODY`，Button = `FS_BODY`，Small = `FS_SMALL`，
  Heading = `FS_TITLE`，Monospace 保持
- `spacing.item_spacing = (8,8)`、`button_padding = (8,4)`、`interact_size.y = 26`

同时把 `src/folder_ui.rs:586-588`、`src/raw_editor.rs:1414-1418`、`src/raw_editor.rs:2126-2140`
三处局部样式覆盖改成引用令牌，去掉重复的字面值。

---

## 二、控件语义分级

**这是本次改版收益最大的一项。** 现在工具栏里 20 个控件全是同一款灰底按钮，评分这种
主决策和"网格 G"这种视图开关长得一模一样。

`theme.rs` 提供三个构造函数：

```rust
pub fn primary(text: &str) -> egui::Button   // 实心强调色，主操作
pub fn secondary(text: &str) -> egui::Button // 抬起的灰底，次操作
pub fn ghost(text: &str) -> egui::Button     // 无底色，轻操作/导航
```

`primary` = `ACCENT` 底 + 白色粗体字 + 无描边；
`secondary` = `PANEL_RAISED` 底 + `BORDER` 描边 + `TEXT` 字；
`ghost` = 透明底 + 无描边 + `TEXT_SECONDARY` 字，hover 时才显出 `PANEL_RAISED` 底。

### 分级表

| 控件 | 角色 | 代码位置 |
| --- | --- | --- |
| 拾取 | `primary`（`SUCCESS` 底） | `src/ui.rs:1374-1392` |
| 拒绝 | `primary`（`DANGER` 底） | `src/ui.rs:1374-1392` |
| 未标记 | `secondary`，选中态用 `ACCENT_SOFT` | `src/ui.rs:1374-1392` |
| 星级 ★×5 | `primary`（`ACCENT` 星的填充） | `rating_control`，`src/ui.rs:2697` |
| 颜色标签 ×5 | `secondary` + 选中加 2px 白描边 | `src/ui.rs:1401-1427` |
| 更多 | `ghost` | `src/ui.rs:1429` |
| 单图 E / 网格 G | `secondary`，选中 `ACCENT_SOFT` + `ACCENT` 描边 | `src/ui.rs:1462-1477` |
| 对比 C | `secondary`，选中同上 | `src/ui.rs:1478-1487` |
| ◀ ▶ | `ghost` | `src/ui.rs:1489-1502` |
| 对焦点 / 适配 | `ghost`，选中态 `ACCENT` 文字 | `src/ui.rs:1504-1515` |
| 曝光 −1 −⅓ +⅓ +1 | `secondary` | `src/ui.rs:1518-1547` |
| 新窗口 | `secondary` | `src/ui.rs:2977-2993` |
| 侧栏「重置」 | `ghost`，小字 | `src/filter_ui.rs:58-68` |

### 星级控件单独升级

`rating_control`（`src/ui.rs:2697`）现在是 5 个 13px 的 `★/☆` 文字按钮加一个 "0" 小按钮。
改成：

- 5 颗星，每颗可点区域 ≥ 22×22，星号用 `FS_BODY`，**已评的星填充 `ACCENT`，未评的星用
  `TEXT_MUTED`**，不要用 `★`/`☆` 两种字符混搭（空心星在暗色下几乎不可见）。
- 数字读数（当前星级）放在星组右侧，用 `FS_SMALL` + `TEXT_SECONDARY`，
  去掉单独的 "0" 按钮（保留"点同一颗星清零"的既有行为）。
- 悬停提示保留。

---

## 三、布局改动

### 3.1 顶栏（`src/ui.rs:2969-3094`）

当前高约 100px，左"新窗口 + 计数"、右"文件名 + EXIF"两行，中间大片空白。

改成**单行、56px 高**：

- 左：`新窗口`（secondary）+ `1/6（共 6）`（`FS_BODY`，计数用 `TEXT_SECONDARY`）
- 右：文件名（`FS_TITLE` 粗体）+ 分隔点 + EXIF 摘要
  （`ISO · f/1.2 · 1/20s · 50mm`，`FS_SMALL`，`TEXT_SECONDARY`），**同一行**
- 相机/镜头/拍摄时间仍然只放在悬停提示里
- 腾出的约 44px 全部还给画布

顶栏与侧栏/画布之间加 1px `BORDER` 分隔线。

### 3.2 左侧导航栏（`src/ui.rs:2900-2968`）

- 4 个图标按钮（`navigation_button`，`src/ui.rs:3336`）统一到 24×24 网格、
  1.75px 描边、圆头线帽。
- **RAW 编辑的图标要能一眼认出**：现在画的是折线图，读起来像"统计"。改成胶片框
  （矩形边框 + 内部一个小圆点/方块）或"方括号内一个圆点"。
- 未选中态图标用 `TEXT_SECONDARY`（现在太暗，几乎看不见），选中态 `ACCENT`，
  选中背景 `ACCENT_SOFT`，左侧 2px `ACCENT` 竖条保持。
- 标签字号统一 `FS_SMALL`。

### 3.3 筛选侧栏（`src/filter_ui.rs:56`）

现在 11 个 `CollapsingHeader` 平铺成一列表单，折叠态完全看不出哪些项当前有值。

- 折叠态标题行改成**自定义行**：左标题 + 右当前值摘要，中间留空。
  例如 `星级` 右边显示 `3 星`、`相机` 右边显示 `Canon R5`。
- **当前有值的项**：标题左侧加 2px `ACCENT` 竖条，摘要用 `TEXT_SECONDARY`。
- 行高统一 30px，展开箭头右对齐。
- 「重置」改为标题行右侧的 `ghost` 小按钮，与标题基线对齐（现在是标题后面一个裸链接）。
- 搜索框、日期范围、数值区间控件保持原有行为，只统一配色与圆角。

### 3.4 文件夹树侧栏（`src/folder_ui.rs:233`）

- 行高保持 24px 不变（拖拽与命中测试依赖它），只改配色：文件夹图标用 `TEXT_SECONDARY`，
  数量用 `TEXT_MUTED`，选中行用 `ACCENT_SOFT` 底 + `ACCENT` 描边。
- 拖拽目标的蓝/红边框沿用现有语义。

### 3.5 画布与照片（`paint_adjusted`，`src/ui.rs:3276`）

**这是"让照片成为主角"的核心改动。** 现在照片是无边距、无圆角、无阴影的硬矩形。

- 画布底色改成 `CANVAS`（比现在更深）。
- 照片四周留 **12px 内边距**（`SP_M`），让照片不贴边。
- 照片加 **6px 圆角**（`RADIUS_LG`）+ 1px `BORDER` 描边 + 向下 2px 的柔和阴影。
- 单图、对比模式（`compare_canvas`，`src/ui.rs:1168`）统一处理。
- 网格模式（`src/ui.rs:1984-2007`）的格子加 4px 圆角，图片区域不再紧贴格子边。

实现上，图片是用 `painter.image()` 直接画的，要做圆角需要：
先画一层圆角矩形作为图片遮罩（`egui::Shape::convex_polygon` 或 `Shape::rect` +
`CornerRadius` 配合 `clip_rect`），或用 `Shape::rect` + 圆角作为裁剪区后再画图。
**必须保证缩放/平移/对焦点叠加层的坐标计算不变**——对焦点是在图片矩形之外单独绘制的
（`src/focus_overlay.rs:69`），加了内边距之后要同步偏移，否则对焦点会错位。

### 3.6 工具栏（`src/ui.rs:1361`）

保持两行结构（640×480 有 ≤112px 的硬约束），只改视觉：

- 分组之间用**自定义 1px 竖线**分隔（当前 `ui.separator()` 在暗色下几乎看不见），
  竖线两侧各留 8px。
- 去掉 `control_group`（`src/ui.rs:2687`）的**固定宽度**，改成自然宽度。
  当前星级组固定 126px、色块组 120px、曝光组 200px，内容不足时留下空洞。
- 工具栏与胶片条之间加 1px `BORDER` 分隔线。
- 窄窗口降级（`src/ui.rs:1363-1367`）保留，但压缩的是间距和内边距，不要把字号缩到 11px 以下。

### 3.7 胶片条（`src/ui.rs:1702`）

- 当前格的边框改成 2px `ACCENT`。
- 缩略图加 4px 圆角。
- 与工具栏之间加 1px 分隔线。

---

## 四、细节打磨

1. **状态提示统一加底衬**：`src/ui.rs:2087-2136` 和 `src/ui.rs:1309-1325` 里的
   "正在加载内嵌 JPEG…"、"预览加载失败…"、"100% 加载失败…"等文字直接压在照片上。
   改成参考 `src/focus_overlay.rs:146-157` 的写法：`from_black_alpha(190)` 圆角底衬 +
   8px 内边距，文字用 `TEXT`（错误用 `DANGER`）。位置不变。
2. **颜色标签按钮**：`src/ui.rs:1401-1427` 现在用 `Button::new("■").fill(color)`，
   选中时是"彩色按钮里再套一个实心方块"。改成自绘 18×18 圆角方块
   （`RADIUS_SM`），选中时加 2px 白色描边并把色块内缩 2px，不再用字符。
3. **网格格子 hover**：`src/ui.rs:1550` 的 `cell()` 加 hover 时的 `ACCENT` 描边。
4. **图标**：见 3.2。

---

## 五、不可破坏的约束

以下每一项都必须仍然成立，违反即视为该阶段失败：

1. **640×480 时工具栏高度 ≤ 112px** —— `src/ui.rs:4224`
   `minimum_window_with_filters_keeps_toolbar_bounded` 断言。改工具栏时必须让这个测试
   继续通过；不能靠放宽断言过关。
2. **`cargo test --lib` 全绿**，包括 `src/ui.rs:3776` 起的全部 egui kittest。
3. **`scripts/verify_feature_images.py` 仍然通过** —— 它靠"推断画布背景色 + 定位真实画布
   边界"来检查曝光 +1EV 后画面变亮、对比模式两个画布都可见。给照片加内边距和圆角会
   影响它对画布边界的推断，**每次改完画布都要跑一遍**。
   出图用 `bash scripts/ui_capture.sh <out> 1280x850 <fixtures> target/release/fastcull features`，
   再跑 `python3 scripts/verify_feature_images.py <out>/main`。
4. **行为不变**：快捷键、评分/旗标/颜色标签的写入与清除语义、对比联动的缩放与平移增量、
   裁切几何、RAW 显影参数与 XMP 写入内容、关闭确认流程，全部保持原样。
5. **对焦点叠加层不错位**：改了照片的内边距/圆角后，`src/focus_overlay.rs` 绘制的对焦点
   框仍然要落在照片里的真实位置。

---

## 六、分阶段与验收

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| **P0** | 修 macOS debug 构建启动崩溃 | debug 二进制能开窗、能出图，不 panic |
| **P1** | 设计令牌与排版统一 | 截图里字号/边框/底色统一；全项目无遗留硬编码字号；`cargo test --lib` 绿 |
| **P2** | 控件语义分级 + 星级升级 | 截图里拾取/拒绝/星级/色块/视图开关层级分明；工具栏分组有分隔 |
| **P3** | 顶栏紧凑化 + 照片加框 | 顶栏单行；照片有圆角、描边、阴影和内边距；对焦点不错位；像素回归通过 |
| **P4** | 细节打磨 | 状态提示有底衬；色块自绘；网格 hover；导航图标可辨 |

每一阶段的视觉验收由截图评审完成，评审标准：

- 控件层级是否一眼可辨（主操作 / 次操作 / 导航）
- 版面是否均衡（没有大片无意义空白，也没有拥挤）
- 文字是否清晰、无截断、无重叠
- 照片是否成为视觉主体
- 两个视口尺寸（1280×850 与 640×480）下都不出现挤压或溢出
