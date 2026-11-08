# Bridge 快捷键启动延迟诊断与修复

2026-11-06，Codex。本机 Windows / Adobe Bridge 2026 / FastCull 2.9.0。

## 结论与实测

延迟集中在 Bridge `app.system(cmd.exe ... --bridge-open ...)` 的外部命令交接阶段。真实快捷键复现中，Bridge 在按键约 212 ms 后生成照片请求，但 FastCull 进程直到请求生成后 6600 ms 才进入程序；进程进入后的首图耗时为 1137 ms。

同一张 `DSC05538.ARW`、同一台电脑实际按 Ctrl+Alt+K 复测：

| 阶段 | 修复前 | 修复后 |
| --- | ---: | ---: |
| 按键到照片请求 | 212 ms | 299 ms |
| 请求到 FastCull 进程入口 | 6600 ms | 89 ms |
| 请求到首图 | 7830 ms | 986 ms |
| 按键到首图 | 8042 ms | 1285 ms |

这是两次具体启动的测量，不代表所有照片批次的平均速度。今天此前四次历史启动的请求到进程入口为 5393–6231 ms，GUI 入口到首图为 595–811 ms。程序直接 `--help` 启动三次为 139.8–146.2 ms，经 cmd 为 288.4–310.9 ms，支持将主要瓶颈定位到 Bridge 的同步外部调用链；没有进一步分析 Adobe 内部等待机制。

## 修复

- `bridge/FastCull.jsx`：支持新版助手时，将已写完并关闭的照片清单原子重命名为 `.queued`，直接返回 Bridge；保留旧助手及 macOS 的原启动路径。
- `src/bin/fastcull-keys.rs`：复用现有常驻助手，在原 40 ms 循环内接收队列；先认领为 `.starting`，再直接启动同目录 `fastcull.exe --bridge-list-owned`。不使用 shell，不执行请求文件里的命令。
- 独立 `launcher-ready` 心跳区分新旧协议；旧助手不会误接队列。过期任务保留为 `.expired`；进程创建失败保留 `.failed` 并由 Bridge 提示。新队列不会重复处理已认领的任务。
- 主选片器 EXE、RAW 解码和照片评分逻辑未修改。
- `scripts/bridge_startup_report.py` 可重放历史日志，分别报告旧调用链和新队列的进程、首屏、首图耗时，并按阈值失败退出。

## 验证

- 真实旧路径回放：`bridge_startup_report.py --logs <用户 LocalAppData/FastCull/logs> --last 1 --max-handoff-ms 1000`，实际旧请求以 6600 ms 失败。
- 新增脚本回归测试先失败再修复；源码和实际安装脚本均为 21/21 通过，包含旧助手兼容、5000 条路径、Unicode/特殊字符、DNG、文件夹、macOS 和快捷键恢复。
- 助手 Rust 测试 3/3 通过，覆盖队列认领、重复扫描、未写完清单、失败保留及过期/未来任务时间检查。
- `cargo clippy --release --locked --offline --bin fastcull-keys -- -D warnings` 通过；release 助手构建通过。
- 私有原生队列与真实 EXE 集成：请求到进程入口 117 ms，到首图 984 ms；自动窗口报告 `first_draw=true ... error=None`，截图已查看。测试只操作私有 JPEG 样本。
- 实际安装后重启 Bridge，再从照片面板选中原 ARW 并按 Ctrl+Alt+K；日志确认走 `native-queue`，请求到入口 89 ms、首图 986 ms。
- 桌面工具读取 FastCull 窗口的单独授权曾超时；真实 ARW 首图时刻取自应用日志，未将该次桌面截图作为证据。

## 安装与回退

实际更新 `lrplugins/FastCull.lrplugin/fastcull-keys.exe` 以及用户 Adobe/Bridge 2026/Startup Scripts/FastCull.jsx。Bridge 已正常关闭并重新启动，旧助手等待宿主租约结束后退出，新助手 `launcher-ready` 与 `ready: Ctrl+Alt+K` 已核实。未发布新版本。

备份位于 `backups/2026-11-06-bridge-startup/`。`fastcull-keys.exe` 是升级前副本；`fastcull-keys-running.exe` 是为释放正在使用的安装文件名而移动的同一旧版本；`FastCull-Bridge-2026.jsx` 是升级前脚本。实际 Startup Scripts 内也保留 `.20261106-startup.bak`。回退时关闭 Bridge，等旧助手退出，再恢复助手与脚本。

| 文件 | SHA-256 |
| --- | --- |
| 旧助手 | `6D14534EC02CC2A47047A763FB6197A60A42EF5D3FD715EAFE7317EE6716BDBC` |
| 新助手 | `86A34D626DDDE85736C0C43F831B2677B35835A922AF84D0B65059C4AB8D6AB8` |
| 原主选片器（本轮保留） | `7B8D5D8636C4256D9E5A4B24322766E35FF0BEDAAEBE39264CE7E04DA64238A1` |

中间脚本、私有样本、集成结果和截图在 `.agent-temp/2026-11-06-bridge-startup/`；编译临时文件在本次允许的本机可视化工作目录内。正式安装不依赖这些测试目录。未清理历史或用户照片。
