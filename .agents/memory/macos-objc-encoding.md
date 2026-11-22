# macOS / objc2 消息编码

## 2026-11-10 CAMetalLayer.setColorspace: 的 debug 构建 panic

**事实**：macOS 上 debug 构建启动即 abort，release 正常；根因是 `src/macos_color.rs` 的 `tag()`
把 CGColorSpace 以 `*const c_void` 发出（编码 `^v`），而 `-[CAMetalLayer setColorspace:]` 的运行时
签名是 `v24@0:8^{CGColorSpace=}16`。objc2 的签名校验只在 `debug_assertions` 下启用
（`objc2-0.6.5/src/verify.rs`，feature `disable-encoding-assertions` 未开、`relax-void-encoding` 未开），
所以 debug panic、release 跳过校验侥幸可用。

**适用条件**：任何 `msg_send!` 传 CF 类型指针给 Cocoa API 时。

**证据**：clang 探针打印 `setColorspace: -> v24@0:8^{CGColorSpace=}16`；修复前 debug 日志出现
`invalid message send to -[WgpuObserverLayer@... setColorspace:]: expected argument at index 0 to
have type code '^{CGColorSpace=}', but found '^v'`。

**修复**：改用 `objc2_core_graphics::CGColorSpace`（`objc2-core-graphics` 0.3.2，需 feature
`CGColorSpace` + `objc2` + `std`），以 `&CGColorSpace` 作为参数。该类型的 `RefEncode` 编码正是
`^{CGColorSpace=}`，debug 与 release 行为一致。`CFRetained<CGColorSpace>` 的 drop 等价于原来的
`CGColorSpaceRelease`。

**坑**：`objc2-core-graphics` 0.3.2 已在 Cargo.lock 里（arboard / objc2-app-kit 间接引入），
加成直接依赖不会新增 crate。`kCGColorSpaceSRGB` 在 objc2-core-graphics 里是 `&'static CFString`，
对应 CoreGraphics 的 `kCGColorSpaceSRGB` 符号值语义一致。

**验证**：`cargo build` + `scripts/ui_capture.sh <out> 1280x850 <fixtures> target/debug/fastcull default`
→ main_png=5，日志出现 `graphics colorspace=sRGB metal_layers=1`，全日志无 PANIC；
`cargo test` 15 个测试二进制全绿；release features 出图 + `scripts/verify_feature_images.py` 通过。

**更新人**：实现者-p0（ZCode workflow），2026-11-10。
