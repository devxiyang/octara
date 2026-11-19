# 宿主边界

内核以窗口为单位接收输入，向宿主请求窗口操作、原生视图放置和帧提交。宿主把操作系统的消息转换成内核认识的数据，并回报操作结果。这样同一视图可交给确定性测试宿主和桌面事件循环。[平台协议](../../crates/octara/src/platform.rs) · [一致性用例](../../crates/octara/tests/integration/conformance.rs)

| 环境 | 用来回答的问题 |
| --- | --- |
| 无窗口宿主 | 给定事件和时钟，状态、结构、动作与绘制数据如何变化 |
| 桌面窗口 | 系统实际采用了什么窗口尺寸、原生视图是否正确放置、窗口关闭时如何退场 |
| OS 输入或移动模拟器 | 系统快捷键、输入法、软键盘和系统返回是否经过正确的宿主路由 |

桌面和 iOS 模拟器使用同一个 winit 宿主。iOS 的场景代理将 UIKit 窗口接到系统提供的 `UIWindowScene`。窗口的触摸、布局、辅助技术和 Metal 表面使用同一内容视图坐标。应用退出活动状态时，宿主用 `InputEvent::Active(false)` 结束窗口的接触。恢复活动状态后，提交当前帧。[iOS 场景接入](../../crates/octara-shell/src/scene_ios.rs) · [Apple 场景指南](https://developer.apple.com/documentation/uikit/transitioning-to-the-uikit-scene-based-life-cycle)

模拟器通过本机 Unix socket 使用同一 Inspector。构建收据校验当前二进制，系统截图用于核对实际呈现。iOS 场景拥有窗口尺寸，Inspector 的 `size` 请求会被拒绝。[模拟器复现](../../examples/README.md#ios-模拟器面板联动) · [平台进度](../ROADMAP.md)
