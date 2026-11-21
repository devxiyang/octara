# 输入、编辑与命令

用户可以通过鼠标、触摸、键盘和辅助技术发起同一个动作。窗口掌握当前焦点和命中对象，组件负责激活后的行为。按压预览、消费事件和最终执行动作是可分别观察的时刻。[输入路径测试](../../crates/octara/tests/integration/activation.rs)

## Edit acceptance

`TextInput::value` 绑定应用接受的编辑文本。`El::a11y_value` 报告选择器等控件可供读屏的值。`numeric_range` 单独表达数值和范围，数值输入框同时保留正在编辑的原始文本。[编辑和选择值的测试](../../crates/octara/tests/integration/a11y.rs#L167)

`on_input` 收到本次编辑提议。应用通过绑定值接受、转换或拒绝提议，窗口在绑定稳定后更新选区与撤销历史。输入法提交的候选词进入同一条编辑路径。保存文件等业务操作由 `on_submit` 等业务动作触发。处理本次提议时向同一编辑器注入第二笔编辑会遇到 `EDIT_PENDING`，应在第一笔完成后再发起。[编辑提议的回归测试](../../crates/octara/tests/integration/edit.rs#L939) · [桌面重命名时序](interaction-scenarios.md#桌面文件重命名)

组词期间，`.value(..)` 对应最近接受的文本。组合区单独参与显示。重新绑定同一个已接受值会保留组合区与选区。绑定为不同值会主动替换文本并清除原组合，窗口在投递时拒绝旧文档上的原生编辑。需要在目录刷新期间继续编辑时，把输入绑定到编辑草稿，目录模型按自己的时序更新。[同值刷新](../../crates/octara/tests/integration/edit.rs#L1124) · [外部替换](../../crates/octara/tests/integration/edit.rs#L114)

Exact 的 Apple 宿主把带组合区的显示文本也发给应用，因此以显示文本判断回写是否相同，并把不同值延后到组词结束。Octara 的重命名场景通过独立草稿保留组词，应用可以在 `on_input` 接受候选词时做格式转换。[Exact 的组词与回写规则](https://github.com/expo/exact/blob/db89b50bb2037afc217b4b4f8a341338a4a5adf1/host/apple/Sources/ExactKit/FieldComposition.swift#L6-L29) · [系统输入法步骤](../../examples/IME.md)

### 原生编辑录制

开启 `App::start_recording()` 后，窗口会把通过校验的原生编辑或选区请求按交付顺序写入 `Recording`。记录在应用回调之前取样，包含窗口、当前编辑器的挂载身份、编辑前的非密文文档和请求。原文档包含选区方向以及可能正在显示的组合区。宿主已过期的请求、窗口拒绝的文档和密码编辑器不会产生原生编辑记录。

回放在同样构建的应用里核对当前焦点、编辑器和原文档，再用正常编辑路径处理请求。原文档不匹配时，该次回放返回错误。应用接受、转换或拒绝文本仍由自己的绑定决定，候选确认之后的业务提交仍来自对应命令。 [录制与回放回归](../../crates/octara/tests/integration/replay.rs)

### 受控文本框实测

macOS 27.2 上使用系统简体拼音、反向 emoji 选区和两个受控字段。应用值操作和旧提交重放由验收窗口的 Inspector 命令触发。[复现步骤](../../examples/IME.md#受控文本值)

| 操作 | 观察结果 |
| --- | --- |
| `A😀Z` 组词中刷新同值 | `ni hao`、原反向选区和组合光标保持。候选窗实例、首项「你好」、画面与屏幕范围 `[950,362,397,28]` 一致 |
| 应用将值改为 `R😀S` | 同一编辑器显示替换后的文本。组合清空，候选窗消失，光标落在 UTF-16 偏移 `4` |
| 系统选择「你好」，应用拒绝 | 收到一次 `A你好Z` 提议，接受次数为零。正文为 `A😀Z`，恢复 UTF-8 `[1,5]`、UTF-16 `[1,3]` 的原反向选区 |
| 组词期间改焦，随后重放旧提交 | 旧候选窗关闭，交付入口返回 `Refused`。第二框从 `B😀Y` 独立组词并提交为 `B你好Y` |

从第一框到第二框，Inspector 光标、AppKit 字符范围锚点、候选窗和 AX 字段边界均向下移动 52 点。候选窗屏幕 `y` 从 `362` 到 `414`，AX 字段边界从 `335` 到 `387`。当时两个字段都能读取 AX 值和边界，`AXSelectedTextRange` 与首字符的 `AXBoundsForRange` 都返回了 `kAXErrorNoValue`。[AX 查询脚本](../../examples/ime_candidates.swift)

### 辅助功能的文本选区与范围

Rune 将输入显示文本、文字分段和选区投影给 AccessKit，再把辅助技术的选区操作交回编辑器。Exact 的 macOS 输入框由 AppKit 的 `NSTextView` 编辑器提供选区。Octara 使用已经结算的文字排版和滚动位置生成 AccessKit 分段，由 macOS 适配器转换 UTF-16 范围和屏幕坐标。[Rune 文字分段](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/text.rs#L140-L187) · [Exact 选区归属](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/FieldSelections.swift#L563-L596) · [AccessKit macOS 转换](https://github.com/AccessKit/accesskit/blob/f3026222559c7f5f46d8f25cb7404de68eb149f0/adapters/macos/src/util.rs#L13-L80)

普通编辑器的 AX 文本、选区和字符坐标使用当前显示文档，包含组合区。业务绑定仍持有已接受的值。选区写入要求普通编辑器持有焦点，两个端点都属于该字段的当前文字分段，并且没有待结算编辑或正在进行的组合。文本替换和影响分段的重排会使旧端点失效。安全字段省略文字范围与选区。可选中文档里的嵌套编辑器只发布字段值，以隔开字段和外围文档的范围。[AccessKit 投影](../../crates/octara/src/kernel/a11y.rs) · [回归测试](../../crates/octara/tests/integration/a11y.rs)

macOS 27.2 的同窗对照对实际 AccessKit 字段和标准 `NSTextView` 调用 AppKit 的辅助功能接口。两者从 `A😀Z` 都读出 `😀`，选区的 UTF-16 位置为 `1`、长度为 `2`。将各自字段向上移动 44 点后，emoji 矩形的 AppKit 屏幕 `y` 均增加 44 点，宽高各自保持一致。向 Octara 的 AX 字段写入位置 `3`、长度 `1` 后，真实编辑器读回 UTF-8 `[5,6]`，选中 `Z`，焦点与文本值保持。此次实测在验收应用进程内调用 AppKit 接口。

### 编辑器的视觉范围

编辑器按显示文本、字体和折行宽度保留一份完整排版结果。选区和组合下划线沿视觉片段绘制，指针命中、原生范围查询和 AccessKit 也使用这些片段。Rune 的选区按 Parley 视觉簇计算。Exact 曾记录仅用每行两个端点会把双向选区画错，例如只选中夹在拉丁字母之间的希伯来文时，高亮可能缩成一条线。[Rune 选区](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/text/cursor.rs#L230-L309) · [Exact 双向选区问题](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/20261110-paragraph-selection-wrong-on-mixed-direction-line.md)

软折行共用相邻两行的字节边界，光标落在下一行。双向交界的光标采用下一个逻辑文字片段的前缘。LF 或 CRLF 都作为一个换行字符留在上一行，原始 UTF-8 偏移保持不变。选择换行时，绘制、原生范围和 AccessKit 共用该行末端的高亮标记。原生非空范围查询返回第一行上所选片段的外包矩形及实际字节范围，折叠范围保留零宽。[原生文字几何](../../crates/octara/src/kernel/text_input.rs) · [多行回归](../../crates/octara/tests/integration/multiline.rs) · [AppKit 范围契约](https://developer.apple.com/documentation/appkit/nstextinputclient/firstrect%28forcharacterrange%3Aactualrange%3A%29)

### 多行编辑器模拟器实测

在 iPhone 17e 模拟器（iOS 27.2）用 Inspector 指令定位精确字节选区，核对运行构建回执和 1170 × 2532 的系统截图。

| 场景 | 复测结果 |
| --- | --- |
| `aאבb` 的 UTF-8 选区 `1..5` | 两个希伯来字母都被高亮。旧构建的同一选区没有高亮。 |
| `aאבb` 的 UTF-8 选区 `0..3` | 正向和反向选区都画成两段，中间的 `ב` 留白。 |
| 选择 `abc\nnext` 的 LF 或 `abc\r\nnext` 的 CRLF | 高亮都在第一行。CRLF 选区保留两个原始字节，之后的光标只下移一行（17.5 点）。 |
| 跨软折行选择 | 高亮分布在两行，共用边界的光标出现在第二行。 |
| 换行落在内容边缘 | 左侧 RTL 换行和右侧 LTR 换行的高亮都画在裁剪区域内。位于右缘的光标保留 1 点可见宽度。 |

另用系统触摸点击 `abc\ndef` 的第二行。光标从字节 0 移到 7，截图中的落点和 Inspector 一致。

### iOS 多行软键盘 Return

winit 的 iOS `UIKeyInput` 会把软键盘 Return 交成没有物理键码的 LF 按下事件。这个问题最初通过宿主把 LF 映射成 Enter 修复。现在多行字段的 Return 由 UIKit 适配器直接交给同一个 Enter 入口。普通多行字段插入一个换行，配置了 `submit_on_enter()` 的字段由现有键位规则提交。[winit 事件转换](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/ios/view.rs#L543-L578) · [UIKit 路由](../../crates/octara-shell/src/text_input_ios.rs)

在 iPhone 17e 模拟器（iOS 27.2）上，从系统键盘点两次「换行」。`alpha` 依次变为 `alpha\n` 和 `alpha\n\n`，字节光标从 5 到 6 再到 7，每次向下移动 17.5 点，各收到一次编辑提议。旧构建收到按键后仍停在 `alpha` 的字节 5。

同一轮在聊天输入示例点一次系统 Return，表单提交并清空正文，焦点仍在输入框。普通单行字段由 UIKit 编辑器处理，系统删除键把 `A😀Z` 变为 `A😀`，随后 Return 只记录一次提交，正文保持 `A😀`。

### iOS 原生多行编辑

`UITextView` 接收软键盘和输入法的组词、选区及候选确认。窗口沿用 `TextDocument` 的已接受值、标记区和 UTF-8 选区。适配器在交付时校验原字段实例及文档，UIKit 回调只登记变化，业务回调在窗口事件循环中运行。类似 Exact 的 Apple 宿主，UIKit 负责原生输入会话。Octara 的画布绘制文本、选区和光标。[Exact 多行输入](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/TextAreaIOS.swift) · [Octara UIKit 适配器](../../crates/octara-shell/src/text_input_ios.rs)

UIKit 请求首行范围矩形或插入点时，多行适配器使用 Octara 的 `TextGeometry`，再转换到 UIKit 文本视图的坐标系。若系统先查新草稿的坐标，适配器用已发布的布局配置直接排版该草稿。Inspector 的 `live.ime.last_geometry_query` 记录最近一次成功回答的实际字节范围和窗口逻辑坐标。[UIKit 首行范围查询](https://developer.apple.com/documentation/uikit/uitextinput/firstrect%28for%3A%29) · [Octara 范围实现](../../crates/octara/src/kernel/text_input.rs)

在 iPhone 17e（iOS 27.2）模拟器上，反向选中 `A😀\nZ` 里的 emoji 后，用系统拼音键盘输入 `nihao`。组词期间已接受值与编辑次数保持原样。候选条位于键盘顶部（`y = 515` 点）。点选「你好」后只产生一次编辑，得到 `A你好\nZ`，光标落在 UTF-8 字节 7。删除最后一个拼音字母会恢复原反向选区。切到另一个输入框会结束原字段的组合。

在第一行组词时，UIKit 请求字节 `9..9` 的插入点，得到窗口 `y = 305` 点。点到第二行再组词时，UIKit 请求字节 `8..8`，得到 `y = 322.5` 点。两个位置均与 Octara 绘制的光标一致。聊天输入框的第一次 Return 确认组词，第二次 Return 才提交正文。

### iOS 连续组词的交付顺序

UIKit 在原生变更完成后捕获显示文本、标记区和选区。同一组合内的草稿更新可以合并，组合结束的快照和每次 Return 保留顺序。窗口逐笔校验字段和预期文档，从实际结算的文档继续处理下一笔。在前一笔被拒绝、被应用改写或字段退休时，依赖旧文档的余下输入失效。若 Return 改变了文档，之后基于原文档捕获的带范围操作也会失效。

无窗口回归将「第一段组词 → 确认候选 → 开始下一段组词」排在同一轮。第一段只提交一次，第二段取消后恢复到该提交。另验证了应用拒绝、应用改写、焦点交接和连续两次 Return。[队列与交付回归](../../crates/octara-shell/src/ime_draft.rs)

## Pointer sessions

窗口把轨迹的设备类型、指针 ID 和按下时的原始按钮与本次接触绑定。Move 和 Cancel 校验设备与 ID，Up 也校验原始按钮。经过平台映射的操作另行记录。例如 macOS 的 Ctrl+主键派发右键动作，仍以主键抬起结束。

窗口失去活动状态、活动模态层收窄范围或后续阶段发现所有者已失效时，清理当前捕获及其后续工作。已开始的可达拖动可以收到一次取消，后来的 Move 或 Up 不会继续这次捕获。 [指针回归](../../crates/octara/tests/integration/pointer.rs)

编辑器持有焦点时，主触摸按下另一类可激活控件，先保留编辑器焦点。窗口用抬手时的坐标核对原按压目标，确认点击后才交接焦点。焦点回调产生的更新结算后，窗口复核原命中和目标的挂载身份及操作资格，再投递按压。拖离后抬手、取消及已转为滚动的接触不会走这次焦点交接。触摸编辑器仍在按下时定位光标。[触摸会话实现](../../crates/octara/src/app/pointer.rs) · [触摸焦点回归](../../crates/octara/tests/integration/pointer.rs)

## Interactive back

平台手势开始时，窗口从当前焦点路径捕获视图自己的 `navigation.back` 命令。iOS 宿主还会校验 UIKit 开始侧滑时显示的详情命令，避免排队期间的页面替换改变接收者。开始和进度阶段只更新手势报告。窗口失活时先撤销返回会话，再通知取消指针的回调。iOS 宿主也撤销旧转场的交付资格，并让 UIKit 收尾。完成时，只有仍挂载、未被模态遮挡且命令身份一致的原视图能收到一次回调。UIKit 未接受完成回调时，宿主可以从 `didShow` 核对最后显示的页面来结清同一次转场。

`InputEvent::BackGesture` 可以进入录制与回放。Inspector 的 `back_gesture` 给出宿主最新阶段及完成是否送达原命令。UIKit 负责手指移动期间的动画，并在交互转为自动收尾时提供进度。业务结果来自应用自己接受的页面状态。[返回会话](../../crates/octara/src/app/back.rs) · [替换回归](../../crates/octara/tests/integration/keys.rs) · [UIKit 交互变化回调](https://developer.apple.com/documentation/uikit/uiviewcontrollertransitioncoordinator/notifywheninteractionchanges(_:))

## Scroll routing

应用的 `scroll_to` 和 `scroll_to_end` 与窗口的定址滚动共用目标查找和滚动轴校验。按名称请求需要唯一的已挂载视口。各非零坐标分量都需要对应的滚动轴。返回值报告请求是否被接受，包括到达边界时。回到末端需要纵向滚动能力，并保留横向偏移。`scroll_into_view` 按元素位置交给其外层视口执行揭示。[轴校验](../../crates/octara/src/kernel.rs#L2309) · [定址滚动回归](../../crates/octara/tests/integration/scroll.rs#L48)

`reveal_row` 为已挂载的单个虚拟区登记业务 key，并绑定该虚拟区实例与请求所有者。窗口先结算行来源和布局，再保留并测量目标行。滚动后用同一行作锚点校正邻近行的测量误差。key 消失、所有者退休或列表实例被替换时，丢弃原请求。同一处理器里的 `focus` 请求随后按当前挂载树结算。保留输入焦点的选择器可以通过现有的 `active_descendant` 关系发布挂载后的目标。[虚拟行实现](../../crates/octara/src/view/list.rs) · [窗口时序](../../crates/octara/src/app/frame.rs) · [揭示回归](../../crates/octara/tests/integration/virtual_reveal.rs)

普通触摸平移越过阈值时捕获一个视口，后续位移和惯性由同一个视口处理。无阶段的 `wheel` 每笔重新命中，选择允许沿相应轴滚动的最近视口。无窗口的嵌套视口对照中，内层剩余 `12` 时输入 `+30`，内层实际滚动 `12`，外层偏移保持 `0`。[滚动与平移入口](../../crates/octara/src/app/pointer.rs) · [偏移写入](../../crates/octara/src/kernel.rs)

触摸仍在惯性滚动的视口时，从已呈现的偏移接住同一视口。移动立即接续，单纯按下和抬手只结束旧惯性，保留焦点。反向后抬手的速度按视口支持的轴和最后一段同向位移估计。取消时保留已应用的偏移，后续位移和抬手失效。惯性同时进入 `settle` 的待结算列表，Agent 可以按视口观察它何时结束。[指针回归](../../crates/octara/tests/integration/pointer.rs) · [等待回归](../../crates/octara/tests/integration/motion.rs)

在滚动视口上设置 `on_touch_scroll` 可以联动面板。回调在 `Before` 和 `After` 阶段接收有符号的 `[dx, dy]`，返回面板实际消费的部分。窗口先执行 `Before`，运行布局并夹紧偏移，再从新偏移开始滚动视口。若还有余量，则交给 `After`。每轴的消费都限制在输入的方向和幅度内。各处位置通过同一帧发布。窗口持有初始视口的身份，取消或卸载终止这次接触。联动直接触摸在抬手时结束。[面板验收时序](interaction-scenarios.md#移动地图的面板与列表联动) · [Compose 的嵌套滚动](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers#nested-scrolling-cycle)

Inspector 的 `touch_scroll` 记录最近一次完成的联动位移，包含视口目标、输入、前置消费、列表消费、后置消费与余量。`layout_correction` 记录前置回调后布局造成的偏移变化。消费与余量相加等于输入，几何位移另计布局校正。[Inspector 快照](../../crates/octara/src/inspect.rs) · [面板示例](../../examples/src/panel_scroll.rs)

macOS 宿主在内容视图收到 `scrollWheel:` 时，从同一 AppKit 事件保存位置、位移和阶段。继承的处理器继续让 winit 派发指针位置和滚动回调。滚动回调按捕获顺序取出该视图的原始样本。AppKit 直接交给原生子视图的事件由该视图处理。窗口接收带 `phase` 的 `InputEvent::Wheel`，旧录制省略阶段时按无阶段输入回放。[AppKit 接收处](../../crates/octara-shell/src/scroll_mac.rs) · [输入事件](../../crates/octara/src/app/event.rs) · [winit 的事件转换](https://github.com/rust-windowing/winit/blob/e9809ef54b18499bb4f2cac945719ecc2a61061b/src/platform_impl/macos/view.rs#L662-L700)

窗口从第一个有效的直接位移选定视口。后续位移沿用其挂载身份，直至惯性结束。指针移动或到达滚动边界都保留这次选择。视口退休、手势取消或窗口失焦结束投递，迟到尾段被丢弃。AppKit 的 `MayBegin` 只表示可能开始，首次实际序列从 `Began` 发起。[窗口滚动测试](../../crates/octara/tests/integration/scroll.rs) · [Exact 的惯性归属测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/ScrollRoutingTests.swift#L374-L413)

`OCTARA_TRACE_SCROLL=1` 配对记录实际交给内容视图的 AppKit 阶段和框架前后的偏移。AppKit 样本保留 `direct_phase`、`momentum_phase`。winit 样本同时列出折叠后的 `phase` 与窗口采用的 `routing_phase`。Apple 指出惯性事件的位置会随指针移动，原生视图归属沿用抛动时的接收者。[AppKit 惯性阶段](https://developer.apple.com/documentation/appkit/nsevent/momentumphase) · [实窗采样步骤](../../examples/README.md#macos-触控板)

### macOS 实窗采样

macOS 27.2 实窗的无阶段滚动样本中，AppKit 报告精确位移，两个阶段值均为 `0`。winit 报告 `Moved` 和纵向 `+100`。内层偏移从 `336` 到 `348`，外层保持 `0`。精确位移和 `Moved` 本身只表明收到滚动，实际触控板的抬手与惯性需配对原始阶段。[验收窗口](../../examples/src/bin/trackpad.rs)

同一窗口用真实触控板从内层距底部 12 点的地方开始滚动。内层从 `336` 到 `348`。抬手时 AppKit 给出零位移的 `phase=Ended`，约 14 毫秒后发出 `momentumPhase=Began`，带来纵向 `+211` 点的位移。直接阶段的 `Began` 和惯性的 `Began` 在 winit 都是 `Started`。指针留在内层时，惯性结束后外层偏移仍为 `0`。

修复前的卸载实验在检测到真实惯性开始后通过 Inspector 移除内层视口。惯性的首笔纵向 `+152` 在内层边界被夹紧。卸载后，指针在同一坐标，后续惯性将外层从 `0` 推到 `132`，直至惯性结束时达到 `456`。

修复后的 macOS 27.2 实窗仍从内层偏移 `336` 开始，直接阶段到达 `348`。抬手和惯性开始分别进入 `direct: up` 与 `momentum: down`。通过 Inspector 卸载内层后，第一笔迟到惯性仍有纵向 `+164` 点。之后的 53 个样本（含惯性结束）中外层偏移均为 `0`。下一笔独立滚轮使外层从 `0` 到 `100`，AppKit 和窗口各记录一次。[实窗复现步骤](../../examples/README.md#macos-触控板)

## Shortcut configuration

`App::keymap()` 读取应用配置，`App::set_keymap(...)` 接受这份配置。`bind_keys` 和 `App::bind_keys` 走同一条注册路径，先解析整批，再应用到 App 的最新配置。窗口共享结果。默认绑定由平台生成，应用条目可以覆盖或显式解绑相应键位。[App 快捷键接口](../../crates/octara/src/app/keys.rs) · [批量配置回归](../../crates/octara/tests/integration/keys.rs)

`App::effective_keymap()` 提供平台默认项和应用配置合成后的只读快照。需要判断某个窗口实际能执行什么时，使用该窗口的焦点、命令资格与生效快捷键。[窗口命令解析](../../crates/octara/src/app/command.rs)

## Deferred commands

弹出菜单关闭后，延迟命令在待处理更新和焦点恢复后路由。`COMMAND_UNHANDLED` 表示恢复后的焦点路径和其余处理器均未处理该动作。检查实际窗口、焦点及菜单可用性。`COMMAND_QUEUE_LIMIT` 提示命令反复入队或批次超限，先定位相互触发的动作。[菜单用例](../../crates/octara/tests/integration/menus.rs) · [双文档验收场景](interaction-scenarios.md)

命令的查询和派发共用一套窗口解析。它检查当前焦点、可达范围、处理器所有者及编辑会话。组词和编辑提议结算期间，显式 `submit` 和编辑命令不可用。键盘快捷键若先结束组词，窗口在正文结算后解析动作。撤销、重做由接收编辑器的历史决定。已投递表示框架找到并交付了动作，应用可以通过模型接受或拒绝编辑。[解析与派发](../../crates/octara/src/app/command.rs) · [组词提交用例](../../crates/octara/tests/integration/keys.rs)

系统菜单在当前窗口更新后投影可用性和快捷键，工具栏通过 `Window::command_enabled` 绑定同样的资格。Kit 弹出菜单在打开时读取原焦点的资格，关闭后用恢复的焦点重新解析。点击系统菜单按命令名执行，菜单的键盘等价事件按当前键位解析。窗口级 Inspector 快照列出可用命令，拒绝的显式调用报告 `refused`。[菜单用例](../../crates/octara-kit/tests/integration/menus.rs) · [双窗口撤销回归](../../crates/octara/tests/integration/menus.rs)
