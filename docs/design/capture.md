# 一张截图能说明什么

## Request

向 Inspector 发送 `{"op":"capture","path":"shot.png"}`。结果包含实际像素来源、渲染方式、是否稳定，以及原生视图使用快照、占位或失败的情况。先看报告，再用图片验证尺寸、位置和外观。请求显示像素后发生回退时，也以实际来源为准。

无窗口参考绘制适合比较场景内容。桌面绘制捕获可以包含受支持的原生视图快照；系统输入法拥有的候选窗需要系统侧观察。`CAPTURE_FAILED` 时检查宿主有无绘制器、目标路径是否可写。窗口在捕获中发生缩放、重建或关闭时，还要核对捕获对应的窗口和尺寸。[桌面实现](../../crates/octara-shell/src/capture.rs) · [捕获测试](../../crates/octara/tests/integration/capture.rs) · [候选窗操作](../../examples/IME.md)

## 捕获对应的帧

`painted` 在请求原生快照前取样已结算的窗口帧。窗口尺寸、绘制位置、普通图片与画布中的图片像素、原生视图报告都随该帧固定。原生视图提交内容或位置变化时，旧请求失效。报告将该视图标为 `failed` 并保留原帧的占位，其他有效快照仍可使用。窗口缩放时，画面保留请求开始的尺寸，等待中或已收集的原生快照会退役。窗口重建和关闭会终止捕获。

`display` 在成功提交绘制后等待窗口服务器采样，采样前后复核应用帧和宿主尺寸。应用帧变化时，在原等待预算内重新取样。发生回退时，报告记录实际使用的绘制捕获和原因。屏幕像素来自窗口服务器，关联的应用帧用来核对提交与几何。系统合成器的呈现时刻、原生视图自己的动画与加载进度，需要从实际像素和宿主状态观察。 [绘制与报告](../../crates/octara/src/app/presentation.rs) · [重采样](../../crates/octara-shell/src/capture.rs)

## macOS 实窗记录

在 macOS 27.2（arm64）用 `octara launch` 启动 `window showcase web`，以构建收据核对 Inspector 执行目标。两个页面来自本地内联 HTML。同一个 WKWebView 挂载实例从 First page 切到 Second page 后，GPU 绘制捕获均报告 `native.shown = snapshot`。在原生视图矩形 `[53, 83, 360, 160]` 内各取一个背景像素，得到 `#6366f1` 和 `#ec4899`。窗口从 `800×600` 缩到 `700×590` 后，Inspector 的宿主尺寸、捕获报告和 PNG 尺寸一致，原生视图位置和第二页颜色保持一致。 [Web 例程](../../examples/src/showcase/web.rs) · [构建收据](agent-context.md)

另一个真实窗口的受控验证先发出 WKWebView 快照请求，同一次窗口命令随后更新 HTML 和视图高度。开始捕获时有一个等待中的原生请求。提交新状态后，等待数归零，旧视图报告为 `failed`，原因为已提交内容或位置变化。旧 PNG 仍画起始帧的红色标记 `#e11d48`，原生占位保留 `[16, 64, 320, 112]`。同一挂载实例的实时矩形已变为 `[16, 64, 320, 160]`，新捕获显示绿色标记 `#22c55e` 并得到原生快照。无窗口回归另外直接注入迟到回复，验证它无法填入被退役的请求。 [捕获回归](../../crates/octara/tests/integration/capture.rs)

首次验收时系统处于锁屏状态，窗口报告 `occluded`，呈现计数为零。请求 `display` 时返回 `painted`，并报告当前帧无法呈现。

解锁后重新启动同一示例。窗口可见、无遮挡且已呈现。First page、Second page 和缩放后三次捕获都返回 `pixels = display`、`renderer = display`、`native.shown = live`。窗口的背板缩放为 `2×`，前两张 PNG 为 `1600×1200`，窗口缩到逻辑尺寸 `700×590` 后得到 `1400×1180`，与宿主报告的像素尺寸一致。三张图的原生视图矩形都与 Inspector 的 `[53, 83, 360, 160]` 一致，采到的背景色依次为 `#6366f1`、`#ec4899` 和 `#ec4899`。 [平台验收清单](../TODO.md)

## 图片尺寸、像素与 GPU 图集

窗口每轮结算布局时按图片身份集中取样。固有尺寸、`object_fit`、画布和绘制列表都使用该样本。尺寸变化会重新计算依赖固有尺寸的布局。等尺寸换帧只更新绘制，布局和视图可以复用。隐藏节点会释放失效的绘制缓存。`display_list()` 读取已准备好的帧。在无窗口测试里，直接调用 `Image::set` 后可用 `App::settle()` 准备下一帧。保留的绘制列表和已开始的 `painted` 捕获随原帧保存像素，GPU 图集使用图片身份和取样版本查找像素。[图片回归](../../crates/octara/tests/integration/image.rs) · [截图回归](../../crates/octara/tests/integration/capture.rs)

Rune 分开记录图像源和固有比例，并覆盖空图与横竖图切换。Exact 在接受同一张位图时一起发布像素与固有尺寸，旧场景在缓存回收后仍可用其快照绘制。[Rune 尺寸测试](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/tests/picture_scale.rs#L280-L357) · [Exact 发布尺寸](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/image.rs#L712-L729) · [Exact 保留像素测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/canvas/vello/src/image_tests.rs#L28-L84)

macOS 27.2 的可见窗口用同一挂载目标依次切换图片。共享源从 `120×60` 变为 `60×120`，固有尺寸随之更新，固定宽 80 的图片从 `80×40` 变为 `80×160`。另一张图从 `0×0` 变为 `64×32`。WindowServer 的 `display` 捕获在 `2×` 输出中核对了新占用区域的像素以及旧横图区域露出的背景。无窗口回归核对普通节点和画布使用同版本，并复读保留的旧列表。

图片图集目前为 `2048×2048`。Metal 验收中，`2048×1` 可绘制，`2049×1` 被略过。修复前，超尺寸图片触发图集重新布局，还可能使已经有位置的合法图片消失。修复后的读回显示合法图片的像素保留。另一个用例同时绘制五张 `1023×1023` 图片，当前图集能容纳四张。第五张在该帧被略过，单独绘制时恢复。旧版、新版和旧版再次进入同一绘制列表时，各自读回所属版本的像素。[Metal 回归](../../crates/octara-render/tests/integration/gpu.rs)
