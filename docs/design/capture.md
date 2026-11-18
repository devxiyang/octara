# 一张截图能说明什么

## Request

向 Inspector 发送 `{"op":"capture","path":"shot.png"}`。结果包含实际像素来源、渲染方式、是否稳定，以及原生视图使用快照、占位或失败的情况。先看报告，再用图片验证尺寸、位置和外观。请求显示像素后发生回退时，也以实际来源为准。

无窗口参考绘制适合比较场景内容。桌面绘制捕获可以包含受支持的原生视图快照；系统输入法拥有的候选窗需要系统侧观察。`CAPTURE_FAILED` 时检查宿主有无绘制器、目标路径是否可写。窗口在捕获中发生缩放、重建或关闭时，还要核对捕获对应的窗口和尺寸。[桌面实现](../../crates/octara-shell/src/capture.rs) · [捕获测试](../../crates/octara/tests/integration/capture.rs) · [候选窗操作](../../examples/IME.md)
