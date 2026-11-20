# 当前工作清单

这里记录仍需决策或取得证据的工作。通过的实现细节随代码和回归测试维护。

## 当前阶段

用复合应用中遇到的实际问题决定公开 API 和 Agent 反馈的调整。资料审阅队列已完成首轮实窗闭环，定位编辑器与读取回执的用法已更新。[本轮证据](design/interaction-scenarios.md#资料审阅队列)

## 内核重构

- [x] 统一弹层的绘制顺序、交互范围和关闭仲裁。分别验证逻辑焦点路径、逆序打开的同级弹层、外点关闭的原目标校验，以及窗口内绘制提示框的焦点恢复。[弹层契约](design/ui-primitives.md#5-overlays) · [弹层回归](../crates/octara/tests/integration/overlay.rs)
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
- [x] 在已解锁、可见的 macOS 窗口验证 `display` 像素来源，核对原生视图报告和实际屏幕尺寸。覆盖 WKWebView 页面切换、窗口缩放、绘制帧版本和锁屏回退。[截图实测记录](design/capture.md#macos-实窗记录)

## 从上轮接续的工程问题

- [ ] 核对可编辑文本框的辅助技术选区与文字范围坐标。macOS 实窗已获得 AX 字段边界，`AXSelectedTextRange` 和 `AXBoundsForRange` 返回 `kAXErrorNoValue`。[受控文本框实测](design/input.md#受控文本框实测)
- [ ] 复核工作区已有的挂载中断改动。三个已知触发点是子作用域首次 watch 关窗、替换分支时 cleanup 关窗、移除列表行时 cleanup 关窗。核对后继工厂、绑定写入、释放计数和首帧提交；另行记录真实屏幕结果。[窗口](../crates/octara/tests/integration/windows.rs) · [分支](../crates/octara/tests/integration/scopes.rs) · [列表行](../crates/octara/tests/integration/collection_rows.rs)
- [ ] 横图换竖图、空图变非空及旧绘制列表重画时，核对布局尺寸和像素版本。单张图片过大与多张图耗尽 GPU 容量分别复现。[图片测试](../crates/octara/tests/integration/image.rs)
- [ ] 记录 Windows 实时检查通道和移动宿主的支持范围。没有平台输入或屏幕证据的项目继续保留待验收状态。

过去的决策过程可从 Git 历史查询，例如 `git log --all -- docs/design`。当前设计、实现边界和待办各有一个入口。[架构](ARCHITECTURE.md) · [实现边界](ROADMAP.md)
