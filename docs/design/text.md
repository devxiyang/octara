# 文本、选区与排版

## The engine

文本引擎将内容、字形样式、可用宽度和字体资源转换成可重用的排版结果。窗口要把文字在局部坐标中的范围换算到当前视口；系统输入法还需要屏幕坐标。字节位置、UTF-16 位置和用户看到的字符边界可能不同。用含有 emoji 的字符串检查左右移动、选择和替换范围。[文本引擎](../../crates/octara-text/src/) · [macOS 桥接](../../crates/octara-shell/src/text_input_mac.rs)

渲染文字、选中背景、光标和命中测试应对同一份文本及其布局作答。输入法可能在下一帧前同步询问几何，复核时要注明宿主询问的是草稿还是已提交文本。[文本呈现测试](../../crates/octara/tests/integration/text_presentation.rs) · [系统验证](../../examples/IME.md)

## Markdown octara-kit

Markdown 在组件库把段落、标题和代码块组装成内容。段内链接归文字范围所有；跨段选择由文档选择功能协调。按业务块复用组件可以减少编辑附近以外的重建。定位问题时先分辨源文本变化、块结构变化和仅布局变化。[Markdown 实现](../../crates/octara-kit/src/markdown/) · [文档选择测试](../../crates/octara/tests/integration/selection.rs)
