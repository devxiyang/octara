# 窗口与命令作用域

一个 App 可以打开多个窗口。主题等共享资源属于 App，每个窗口单独维护挂载树、焦点、选择和几何。关闭窗口会退休其内容和观察目标；App 级工作可继续，最终由宿主关闭 App。[窗口实现](../../crates/octara/src/window.rs) · [多窗口测试](../../crates/octara/tests/integration/windows.rs)

首窗的 `octara_shell::Options::window` 和之后的 `open_window` 都使用 `WindowOptions`。窗口请求可以包含作者期望的逻辑尺寸。宿主打开原生窗口后，把实际逻辑尺寸、缩放和安全区交给内核。根视图首次挂载时就能按这些数据选择布局。无窗口宿主用请求尺寸作为初始视口，缩放取 1。平台后续报告的视口变化仍更新同一个窗口。[开窗协议](../../crates/octara/src/platform.rs) · [桌面与 iOS 宿主](../../crates/octara-shell/src/platform.rs)

设计一项动作时，写清从哪个窗口发起、焦点位于何处、哪个对象可以处理。例如同一文档的“撤销”可以有菜单、快捷键和工具栏三个入口。已关闭的窗口与重新打开的同标题窗口要具有不同目标。三个入口路由到同一文档的完整验收条件见[交互时序](interaction-scenarios.md)。
