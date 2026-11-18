# 把原生视图放进窗口

## Two slots, no interleaving (Rune)

当前宿主将原生视图放在绘制表面上方或下方。上方的 Web 视图可以遮住后来绘制的 Octara 内容。`NATIVE_COVERED` 时先核对宿主槽位和覆盖区域，再调整弹层位置、原生视图范围或下层槽位。在 headless 模式可以检验放置请求，最终遮挡和点击归属需要桌面窗口。[原生放置测试](../../crates/octara/tests/integration/native.rs)

## Input, focus, accessibility

为地图、网页等平台对象提供可读名称。`UNLABELED_NATIVE_VIEW` 表示 Octara 的语义树缺少名称。进入和离开原生视图时还要核对焦点，以及宿主实际把点击交给谁。截图报告会说明是否拿到了原生对象的像素。[macOS 适配](../../crates/octara-shell/src/native_mac.rs) · [截图](capture.md)
