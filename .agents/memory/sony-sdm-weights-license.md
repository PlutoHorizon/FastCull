# Sony SDM / ExRAW HiRes ONNX 权重的许可与格式（2026-11-22 调研）

## 结论（一句话）

Sony IEDT 的 SDM（SuperDemosaic）/Extended Hi-Res ONNX 权重**没有任何单独的许可**，
它们被 IEDT EULA 的 "SOFTWARE" 定义一并吞掉（"any data files … provided by SONY …
through … SONY's websites or other relevant online means"），而 EULA 明确禁止
copy/redistribute/reverse-engineer/derivative works，且授权仅限个人家用或企业内部使用；
因此**在第三方应用里复用或分发这些权重 = 无许可**，唯一安全路径是"驱动 IEDT 出片"或
改用可再分发的开源权重（PMRID Apache-2.0 / NAFNet MIT / darktable RawNIND GPL-3.0）。

## EULA 原文（primary source，已安装产品内）

- 路径：`/Applications/Imaging Edge Desktop.app/Contents/Resources/EULA/EULA_int-en.txt`
  （国际英文，16924 B，md5 26cfc6cbe79303a8a17510c9cdfca2b7）、`EULA_us.txt`（美国版，
  管辖法 Delaware + AAA 仲裁），另有 ~50 个语言版本。首行 "Last Updated: 2026-01"。
  macOS IEDT 1.2.02.080500 / Viewer 4.1.00.03062 / Edit 4.x。
- Sony **不在网页上单独发布**该 EULA；首次运行由 `EULA_PP_Checker.app` 展示，
  Viewer/Edit 菜单项 "Software License Agreement"（Strings.xml）。
  `https://imagingedge.sony.net/en-us/licenses.html` 只是**官网自身 JS 库**的开源许可，与产品无关。
- 关键条款（原文）：
  - SOFTWARE 定义：含 "any data files, each provided by SONY or the THIRD-PARTY SUPPLIERS
    through the product packaging, SONY's websites or other relevant online means,
    or created by operation of such software"。
  - GRANT OF LICENSE："limited, non-exclusive, non-transferable license to use the SOFTWARE
    solely in connection with your compatible device"；"If you are a consumer, the foregoing
    license is for your personal and domestic use only; if you is a business customer,
    it is for your internal business use only."
  - 禁止项 (1) copy, publish, adapt, redistribute, attempt to derive source code, alter or
    modify … or create any derivative works；(2) "reverse engineer, decompile, or disassemble
    any of the SOFTWARE **except to the extent that such actions cannot be prohibited under
    applicable laws**"；(9) "share, distribute, rent, lease, sublicense, assign, transfer or
    sell the SOFTWARE"。
  - TERMINATION："cease all use, and destroy any copies, of the SOFTWARE"。
  - EULA 全文 grep **不含** model / AI / artificial intelligence / neural / machine learning /
    extract —— 模型只能靠上面的 "data files" 兜住。
- 适用条件：把 SDM 权重抽取出来在第三方产品（尤其商业分发）里用，等于 copy + redistribute
  + 反向工程，落在 (1)(2)(9)；"personal and domestic use only" 也排除商业化。
  唯一可能的口子是 (2) 的法定互操作例外（EU Software Directive / 当地强制法），
  但**互操作例外不豁免再分发**。此为法律解读，非法律意见。

## 没有单独的模型许可 / 下载流程（verified）

- 模型下载提示文案（Viewer `English.lproj/Strings.xml`）：
  "To execute extended RAW processing, you need to download the model files. Download now?
  * Make sure that your Internet is connected." —— 无任何附加条款勾选。
- 下载端点（Viewer 二进制字符串）：
  `https://support.d-imaging.sony.co.jp/disoft_DL/imagingedge_DL/sdm?mdl=<model>`，
  实测 302 → `https://di.update.sony.net/...` CDN。端点可达 ≠ 有许可。
- 二进制里还有 `Strings/ExRAW_ModelFile`、`OnShowModelFilesDownloadsMarqueeDlg`、
  "onnx model: %s"、"onnxruntime-node"、"Failed to run sdm in error id (%d)."

## 第三方项目结论

- **darktable**：master 源码 grep `superdemosaic|sdm_nn|Extended RAW` = 0 命中，不支持 SDM。
  它的 AI 栈只用自备模型（`data/ai_models.json`：denoise-nind、rawdenoise-nind、
  upscale-realplksr、mask sam2.1/segnext），用户主动才下载；另有
  https://github.com/darktable-org/darktable/wiki/AI-Model-Integration-Policy（只管范围/隐私，
  不含许可条款）。
- **RawTherapee**：dev 分支 grep superdemosaic/sdm_nn/exraw/onnx = 0 命中，不支持。
- **exiftool**：master 无 Sony Extended RAW / ExRAW HiRes 标签，也**不解析 0x789b/0x789c**
  （与你 2026-10-11 的结论一致：exiftool 重写 ARW 会破坏 Viewer 判定）。
- **唯一公开的第三方逆向文档**：`https://github.com/Jannchie/llr/blob/main/docs/sony-edit-internals.md`
  （仓库无 LICENSE 文件）。要点：模型在 `C:\ProgramData\Sony\Imaging Edge\OnnxData\`，
  4 个 hash 命名 .dat 各 ~87.85 MB，共享 magic `1d 62 19 da ee 92 9e 5d`、高熵无明文算子
  → **权重落盘加密、运行时解密**；后端 onnxruntime 1.18(CPU) + DirectML + Vulkan；
  流水线 sdm_pre→sdm_nn→sdm_post(+sdm_noise_add)、bayer-cas、srsw 超分；旁挂
  `drunet.dat`、`srsettings0000.tbl`、`dcv_psf_dst.bin`。其结论是"神经网络那层不可复刻"，
  **全网无人分发这些权重，也无人给出可再分发的许可结论**。

## 输出几何与机型（Sony 官方，已核实）

- 官方页 https://support.d-imaging.sony.co.jp/app/imagingedge/en/instruction/1_6_extended_raw/index.php
  （英文版，实为 /app/imagingedge/l/instruction/… 的英文镜像）：
  "Extended RAW Processing enables you to create a high-grade image from a **single** RAW image
  (ARW)"；"expands the resolution of images, **doubling the number of pixels vertically and
  horizontally (for a fourfold increase in the total number of pixels)**"。
  → Extended Hi-Res = **单帧 2× 线性升采样（宽高各 2×，总像素 4×）**，不是多帧合成。
- Extended NR = 原分辨率 + 降噪。输出格式 AXR / JPEG / TIFF。
- 支持机型（官方 Supported Devices 表 + 官网功能页"支持的相机：α1 II、α9 III、α7R VI、α7 V"）：
  **ILCE-1M2 / ILCE-9M3 / ILCE-7RM6 / ILCE-7M5**（与你 2026-10-11 的 makernote 机型码结论一致）。
- 注意别混淆：相机端 "Pixel Shift Multi Shooting"（4 张/16 张，16 张约 10.72 亿像素）是**另一个**
  功能，Sony 官网把它单列；2020 年 A7R IV 的 "ExRAW HiRes" 是相机端像素位移高分辨率模式，
  与 IEDT 的 AI "Extended Hi-Res" 不是一回事（本次未取到 Sony 一手 ExRAW HiRes 规格页）。

## 本机可验证的格式细节

- `~/Library/Application Support/Imaging Edge/OnnxData/sdmsettings.dat`（7744 B）：
  magic `SDMP`、version=4，ISO 阶梯可见 100/200/400/800/1600/3200/6400/12800/25600。
- `sdmlist.dat`（706 B）：字节级混淆，无明文。
- 你 2026-10-11 的 Ghidra 结论（ARW makernote 0x789b/0x789c 校准数组、0x94 B/SDM-ID×ISO 块）
  本次**未独立复验**；公开资料里也没有 "PTC"（你用的术语）的任何定义，
  ONNX 的输入张量（Bayer 相位 + stddev 相位）与输出名**无公开文档**——权重是加密的。

## 证据

2026-11-22，ZCode。本机 IEDT 安装（`/Applications/Imaging Edge Desktop.app`、
`/Applications/Imaging Edge/{Viewer,Edit}.app`）+ Sony 官方站
（support.d-imaging.sony.co.jp/app/imagingedge/{en/,l/}）+ imagingedge.sony.net +
GitHub raw（Jannchie/llr、darktable-org/darktable、Beep6581/RawTherapee、exiftool/exiftool）。
