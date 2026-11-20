# 当前工作清单

这里记录仍需决策或取得证据的工作。通过的实现细节随代码和回归测试维护。

## 当前阶段

用复合应用中遇到的实际问题决定公开 API 和 Agent 反馈的调整。资料审阅队列已完成首轮实窗闭环，定位编辑器与读取回执的用法已更新。[本轮证据](design/interaction-scenarios.md#资料审阅队列)

## 内核重构

- [x] 统一弹层的绘制顺序、交互范围和关闭仲裁。分别验证逻辑焦点路径、逆序打开的同级弹层、外点关闭的原目标校验，以及窗口内绘制提示框的焦点恢复。[弹层契约](design/ui-primitives.md#5-overlays) · [弹层回归](../crates/octara/tests/integration/overlay.rs)
- [ ] 推进渲染与作者 wgpu 能力。共享材质与多 pass 画布已接入渲染和绘制捕获。接下来验证 Dialog 转场的背景采样、纹理版本与资源寿命，并核对实屏帧节奏和设备重建。[渲染架构](design/rendering.md)
- [x] 将模态范围应用到 Inspector 动作、系统辅助功能树与辅助技术输入。回归覆盖独立挂载的弹层、标签关系、坐标、迟到选区和恢复焦点。[模态语义](design/ui-primitives.md#5-overlays) · [语义回归](../crates/octara/tests/integration/focus.rs)
- [x] 让虚拟列表按业务 key 揭示一行。用 Chat 长会话列表和持有编辑器焦点的搜索选择器验证，分别核对真实焦点与 `active_descendant`。[复现、实现与实测](research/headless-ui-react.md#虚拟选项与退出时序)
- [x] 分开报告 Inspector 动作投递、窗口布局结算与宿主队列提交。用虚拟行跳转核对布局已结算而宿主仍待提交、随后只读观察提交的过程。[操作回执](design/agent-knowledge.md)
- [x] 统一输入会话与录制。指针续段校验设备、ID 和原始按键，已校验的原生编辑和选区请求进入录制与回放。无窗口回归覆盖设备与按键碰撞、组词和选区的 JSON 回放，以及过期编辑的拒绝。[手势与编辑协议](design/input.md) · [录制回归](../crates/octara/tests/integration/replay.rs)
- [x] 让异步截图使用可核对的提交版本。绘制捕获固定起始帧与像素，视图内容或位置变化会退役旧快照。显示捕获复核提交帧和宿主尺寸。回归覆盖迟到快照、尺寸变化、位置往返与共享图片换帧。[截图协议](design/capture.md) · [捕获回归](../crates/octara/tests/integration/capture.rs)

## 平台验收

- [x] 在 iOS 模拟器核对联动面板的原生触摸、系统中断、迟到位移和恢复呈现。[实测记录](design/interaction-scenarios.md#ios-模拟器实测)
- [x] 在 iOS 模拟器核对普通单行字段的系统拼音、候选确认、取消、焦点退休和键盘边界。[实测记录](../examples/IME.md#ios-拼音实测)
- [x] 在 macOS 同窗复核行内重命名的拼音候选、目录重排、选区和编辑器退休，并与标准 NSTextView 对照候选位置。[实测记录](design/interaction-scenarios.md#macos-实窗记录)
- [x] 在两个同标题的 macOS 实窗核对系统菜单、快捷键、工具栏和 Inspector 的撤销归属与可用性。[实测记录](design/interaction-scenarios.md#两个同标题文档的撤销)
- [x] 在 iOS 模拟器核对相册侧滑返回过半后取消及再次完成，验证页面身份、阅读位置和释放次数。[相册实测](design/interaction-scenarios.md#ios-相册实测)
- [x] 在 iOS 模拟器核对模态前后的原生辅助功能树，用 VoiceOver 手势核对详情内的遍历范围。[辅助功能实测](design/interaction-scenarios.md#模态辅助功能范围)
- [x] 修复软键盘收起时按钮移出原触点导致的点击丢失，并在 iOS 模拟器核对一次触摸关闭和焦点恢复。[触摸实测](design/interaction-scenarios.md#ios-键盘收起与点击资格)
- [x] 在 iOS Dialog 内核对系统拼音提交、虚拟 Combobox 的按 key 揭示与重排、焦点和操作回执。修复弹层避让状态栏、列表裁切及可用空间恢复后的尺寸。[实测记录](design/interaction-scenarios.md#ios-dialog-内的中文虚拟选择器)
- [x] 在 macOS 同窗对照普通编辑器与标准 NSTextView 的辅助功能选区和范围坐标，核对写入回读、字段移动及过期端点拒绝。[实测与回归](design/input.md#辅助功能的文本选区与范围)
- [x] 验收首次 watch、替换分支 cleanup 和移除列表行 cleanup 中途关窗。核对后继工厂和绑定、释放次数、宿主帧及 macOS 原生窗口状态。[实测与回归](design/runtime-contract.md#macos-挂载中断实测)
- [x] 在已解锁、可见的 macOS 窗口验证 `display` 像素来源，核对原生视图报告和实际屏幕尺寸。覆盖 WKWebView 页面切换、窗口缩放、绘制帧版本和锁屏回退。[截图实测记录](design/capture.md#macos-实窗记录)
- [x] 验收横图换竖图、空图变非空和旧绘制列表重画。用 WindowServer 像素复核布局切换，用真实 Metal 分别复现单图超尺寸和图集容量耗尽。[图片实测与回归](design/capture.md#图片尺寸像素与-gpu-图集)
- [x] 记录 Windows 实时通道与移动宿主支持范围。交叉类型检查 Windows 目标，复验 iOS 模拟器的系统触摸、回执和设备画面。[宿主支持范围](design/platform.md)
- [x] 在 iOS 模拟器核对列表行移出裁剪视口后的原生辅助功能命中。覆盖完全裁剪、部分裁剪和反向滚动。[裁剪实测](design/platform.md#ios-辅助功能命中与裁剪)
- [x] 统一多行编辑器的绘制、指针、原生范围与辅助功能几何。覆盖双向选区、LF／CRLF、软折行和内容边缘。[编辑器几何与实测](design/input.md#编辑器的视觉范围)
- [x] 修复 iOS 多行字段的软件键盘 Return 映射。用模拟器的系统键盘核对连续换行、聊天输入提交和原生单行字段回归。[模拟器实测](design/input.md#ios-多行软键盘-return)
- [x] 将 iOS 多行文本会话交给 UIKit，收窄 winit 的宿主职责。在模拟器核对拼音组合、候选提交、取消、焦点交接和跨行范围坐标。[架构边界](ARCHITECTURE.md#平台宿主与-winit-的边界) · [模拟器实测](design/input.md#ios-原生多行编辑)
- [x] 保留 UIKit 同一轮交付内的连续组词边界。覆盖候选提交、下一段取消、文档改写与焦点交接。[交付顺序](design/input.md#ios-连续组词的交付顺序)
- [x] 在 iOS 模拟器用系统拼音和真实触摸完成资料审阅。核对两行意见回读、旧编辑目标退休和待审编辑器隔离。[资料审阅实测](design/interaction-scenarios.md#ios-资料审阅实测)

## 待验收的平台能力

当前优先 macOS、iOS、Linux。可以独立执行的无窗口检查和 iOS 模拟器回归先行。Android 与 Windows 现阶段只要求对应 CI 作业通过。[平台优先级](ROADMAP.md#推进顺序) · [CI 配置](../.github/workflows/ci.yml)

- [ ] 核对资料审阅示例的 iOS 顶部安全区，避免标题与状态栏时钟重叠。[模拟器实测](design/interaction-scenarios.md#ios-资料审阅实测)

### GitHub CI

- [x] 核对 `main` 的 Android 库目标检查与 Windows 桌面矩阵。两者在 `ab8b159` 通过。[CI 结果](https://github.com/devxiyang/octara/actions/runs/44671492733)
- [ ] 复核 macOS Live Inspector 双击的时钟采样。该次作业完成双击后，应用时钟差为 625 ms，脚本计时为 341 ms。[失败步骤](https://github.com/devxiyang/octara/actions/runs/44671492733/job/136073265673#step:9:1)

### 等待验收环境

- [ ] iOS 真机打包签名、安装和实时检查，以及实机系统输入和画面证据。[移动支持范围](design/platform.md#移动宿主)
- [ ] 在 Linux 实窗核对实时 Inspector、系统输入和实屏像素。[桌面支持范围](design/platform.md#桌面实时检查)

### 暂缓的 Android 与 Windows 设备验收

- [ ] Android Activity 宿主、APK 构建安装和实际设备输入、画面验收。[移动支持范围](design/platform.md#移动宿主)
- [ ] Windows 本机实时检查通道，以及 Windows 系统输入与实屏像素的验收。[桌面支持范围](design/platform.md#桌面实时检查)

过去的决策过程可从 Git 历史查询，例如 `git log --all -- docs/design`。当前设计、实现边界和待办各有一个入口。[架构](ARCHITECTURE.md) · [实现边界](ROADMAP.md)
