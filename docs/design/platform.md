# 宿主边界

内核以窗口为单位接收输入，向宿主请求窗口操作、原生视图放置和帧提交。宿主把操作系统的消息转换成内核认识的数据，并回报操作结果。这样同一视图可交给确定性测试宿主和桌面事件循环。[平台协议](../../crates/octara/src/platform.rs) · [一致性用例](../../crates/octara/tests/integration/conformance.rs)

| 环境 | 用来回答的问题 |
| --- | --- |
| 无窗口宿主 | 给定事件和时钟，状态、结构、动作与绘制数据如何变化 |
| 桌面窗口 | 系统实际采用了什么窗口尺寸、原生视图是否正确放置、窗口关闭时如何退场 |
| OS 输入或移动模拟器 | 系统快捷键、输入法、软键盘和系统返回是否经过正确的宿主路由 |

## 桌面实时检查

macOS 和 Linux 的实时 Inspector 使用本机 Unix domain socket。`octara launch` 记录构建收据，`inspect --connect` 和 `run --connect` 连接正在运行的窗口。macOS 有实窗验证。Linux 的 CI 提供构建、测试与无窗口截图覆盖。[连接与构建收据](agent-context.md) · [CI 配置](../../.github/workflows/ci.yml)

Windows 的窗口宿主走 winit。`octara check` 和无 `--connect` 的 `inspect`、`run` 通过无窗口宿主验证语义、脚本和参考绘制，`check` 可以显式选用 Windows 平台约定。当前实时客户端和宿主监听器都受 Unix 条件限制。Windows 上 `launch` 在启动前拒绝，`inspect --connect` 和 `run --connect` 在连接处拒绝。给图形应用设置 `OCTARA_INSPECT` 会使宿主报告启动错误。[无窗口入口](../../crates/octara-cli/src/main.rs) · [启动入口](../../crates/octara-cli/src/launch.rs) · [实时客户端](../../crates/octara-cli/src/live.rs) · [宿主监听](../../crates/octara-shell/src/inspect.rs)

在 Mac 上用已安装的 Rust 1.100.0 和缓存交叉类型检查 `x86_64-pc-windows-msvc`，CLI、Shell、示例和其余 Rust 测试目标通过。检查排除了 Chat。全工作区的交叉检查在 Chat 所用的 `aws-lc-sys` 停下，C 交叉编译缺少 MSVC 和 Windows SDK 头文件。该结果属于 `cargo check`，证据范围是 Windows 目标的 Rust 类型检查。

GitHub CI 的 Windows 作业曾在 `b117aee` 通过实窗一致性测试。该测试用程序发出的框架输入、窗口和帧计数核对通用约定。`bfcc5e3` 的 Windows 作业构建成功，随后 CLI 的源路径断言因 Windows 分隔符失败，实窗步骤被跳过。断言现将预期路径统一为 `/`，以匹配 CLI 的源码位置格式。修正后的 Windows 作业结果待新一轮 CI 核对。Windows 的系统输入、实屏像素和实时检查仍需各自的验收证据。[实窗一致性代码](../../crates/octara/src/conformance.rs) · [通过的 Windows 作业](https://github.com/devxiyang/octara/actions/runs/43982629738/job/133702689191) · [较新的 Windows 作业](https://github.com/devxiyang/octara/actions/runs/44554228138/job/135665821758)

## 移动宿主

iOS 的 winit 宿主通过场景代理将 UIKit 窗口接到系统提供的 `UIWindowScene`。触摸、布局、辅助功能和 Metal 使用同一个内容视图坐标。退出活动状态时结束接触，恢复活动状态时提交当前帧。当前示例入口使用 Apple Silicon 模拟器的 `aarch64-apple-ios-sim` 目标。[iOS 场景接入](../../crates/octara-shell/src/scene_ios.rs) · [模拟器复现](../../examples/README.md#ios-模拟器面板联动)

iPhone 17e 模拟器上的 iOS 27.2 已验证系统触摸、中断恢复、UIKit 侧滑返回的取消与完成、模态 VoiceOver 范围和普通单行输入框的系统拼音。嵌套 Dialog 里的虚拟选择器也验证了系统拼音与确认键，列表导航和重排从 Inspector 驱动。[触摸与恢复](interaction-scenarios.md#ios-模拟器实测) · [侧滑返回](interaction-scenarios.md#ios-相册实测) · [VoiceOver](interaction-scenarios.md#模态辅助功能范围) · [单行拼音](../../examples/IME.md#ios-拼音实测) · [虚拟选择器](interaction-scenarios.md#ios-dialog-内的中文虚拟选择器)

模拟器通过 Mac 本机的 Unix socket 使用 Inspector，构建收据核对运行二进制。设备画面由 `simctl io … screenshot` 取得，Inspector 还可返回离屏 `painted` 捕获。Inspector 的 `display` 像素后端当前由 macOS 提供。iOS 场景拥有窗口尺寸，Inspector 的 `size` 会被拒绝。[模拟器打包脚本](../../examples/ios_panel.py) · [捕获实现](../../crates/octara-shell/src/capture.rs) · [尺寸入口](../../crates/octara-shell/src/resize.rs)

在 `a3c10b2` 的 iPhone 17e 模拟器复验中，原生触摸拖动 96 点，面板扩展 12 点，列表滚动 84 点。列表 `target` 保持，宿主提交计数从 6 增至 9。构建收据匹配，设备截图为 `1170×2532` 像素。向 Inspector 请求 `display` 时读回 `pixels=painted`、`renderer=gpu` 及 macOS 专有的显示捕获回退原因。另一次采样中，辅助功能命中仍返回已滚到裁剪视口上方的列表行，设备画面该处显示地图，这个命中边界留待复核。[面板复现](../../examples/README.md#ios-模拟器面板联动) · [工作清单](../TODO.md)

iOS 真机仍缺仓库内的打包签名、安装、Mac 到设备的实时检查通道及实机输入与画面证据。Android 已有触摸平台约定用于无窗口检查，Activity 宿主入口、APK 构建安装和系统输入与画面验收仍待完成。[平台约定](../../crates/octara/src/app.rs) · [宿主启动](../../crates/octara-shell/src/lib.rs) · [验收清单](../TODO.md)

Android 与 Windows 现阶段以 CI 通过作为验收条件。CI 只在推送到 `main` 或 PR 以 `main` 为目标分支时触发。Android 作业交叉类型检查内核和 Kit 的 `aarch64-linux-android` 库目标，工作区测试覆盖 Android 触摸约定和 Windows 指针约定。Windows 沿用桌面矩阵的构建、工作区测试及窗口一致性作业。设备输入和实屏验收另列待办。[CI 配置](../../.github/workflows/ci.yml) · [平台约定回归](../../crates/octara/tests/integration/inspect.rs) · [平台顺序](../ROADMAP.md#推进顺序)
