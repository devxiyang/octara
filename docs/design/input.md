# 输入、编辑与命令

用户可以通过鼠标、触摸、键盘和辅助技术发起同一个动作。窗口掌握当前焦点和命中对象，组件负责激活后的行为。按压预览、消费事件和最终执行动作是可分别观察的时刻。[输入路径测试](../../crates/octara/tests/integration/activation.rs)

## Edit acceptance

`TextInput::value` 绑定应用接受的编辑文本。`El::a11y_value` 报告选择器等控件可供读屏的值。`numeric_range` 单独表达数值和范围，数值输入框同时保留正在编辑的原始文本。[编辑和选择值的测试](../../crates/octara/tests/integration/a11y.rs#L167)

`on_input` 收到本次编辑提议。应用通过绑定值接受、转换或拒绝提议，窗口在绑定稳定后更新选区与撤销历史。输入法提交的候选词进入同一条编辑路径。保存文件等业务操作由 `on_submit` 等业务动作触发。处理本次提议时向同一编辑器注入第二笔编辑会遇到 `EDIT_PENDING`，应在第一笔完成后再发起。[编辑提议的回归测试](../../crates/octara/tests/integration/edit.rs#L939) · [桌面重命名时序](interaction-scenarios.md#桌面文件重命名)

组词期间，`.value(..)` 对应最近接受的文本。组合区单独参与显示。重新绑定同一个已接受值会保留组合区与选区。绑定为不同值会主动替换文本并清除原组合，窗口在投递时拒绝旧文档上的原生编辑。需要在目录刷新期间继续编辑时，把输入绑定到编辑草稿，目录模型按自己的时序更新。[同值刷新](../../crates/octara/tests/integration/edit.rs#L1124) · [外部替换](../../crates/octara/tests/integration/edit.rs#L114)

Exact 的 Apple 宿主把带组合区的显示文本也发给应用，因此以显示文本判断回写是否相同，并把不同值延后到组词结束。Octara 的重命名场景通过独立草稿保留组词，应用可以在 `on_input` 接受候选词时做格式转换。[Exact 的组词与回写规则](https://github.com/expo/exact/blob/db89b50bb2037afc217b4b4f8a341338a4a5adf1/host/apple/Sources/ExactKit/FieldComposition.swift#L6-L29) · [系统输入法步骤](../../examples/IME.md)

## Scroll routing

当前触摸平移越过阈值时捕获一个视口，后续位移和抬手惯性都由该视口处理。框架的 `wheel` 对每笔输入重新命中，选择允许沿相应轴滚动的最近视口。两条路径都调用 `scroll_by`，它通过现有的滚动范围夹紧位置，调用方得不到未消费量。无窗口的嵌套视口对照中，内层剩余 `12` 时输入 `+30`，内层实际滚动 `12`，外层偏移仍为 `0`。[滚动与平移入口](../../crates/octara/src/app/pointer.rs#L406) · [偏移写入](../../crates/octara/src/kernel.rs#L2330)

联动的目标契约由窗口维护序列的接收者、结束和取消。接收者可以是一个视口，也可以是联动组件。组件选择参与者的消费顺序，窗口将每次实际消费从可用位移中扣除。视口改变尺寸时，以新的范围夹紧偏移，再从这个基线计算本次消费。使用现有布局与发布边界，让各参与者的结果在同一帧可见。Agent 通过诊断查看输入来源、接收者、各处消费量、剩余量和取消原因。Jetpack Compose 用前置消费、子节点消费和后置消费表达这类顺序。[面板验收时序](interaction-scenarios.md#移动地图的面板与列表联动) · [Compose 的嵌套滚动](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers#nested-scrolling-cycle)

桌面宿主需要保留平台可靠提供的手势阶段。Octara 当前在 `MouseWheel` 入口只保留位置和位移。macOS 上 winit 0.30.13 还把 AppKit 的直接阶段与惯性阶段折叠成同一个 `TouchPhase`。要验证触控板抬手后惯性是否归原视口，先在宿主采样可区分的两个阶段。无分段信息的离散滚轮仍可按单笔输入命中。[当前桌面入口](../../crates/octara-shell/src/lib.rs#L833) · [winit 的事件转换](https://github.com/rust-windowing/winit/blob/e9809ef54b18499bb4f2cac945719ecc2a61061b/src/platform_impl/macos/view.rs#L678-L700) · [AppKit 惯性阶段](https://developer.apple.com/documentation/appkit/nsevent/momentumphase)

## Shortcut configuration

`App::keymap()` 读取应用配置，`App::set_keymap(...)` 接受这份配置。`bind_keys` 和 `App::bind_keys` 走同一条注册路径，先解析整批，再应用到 App 的最新配置。窗口共享结果。默认绑定由平台生成，应用条目可以覆盖或显式解绑相应键位。[App 快捷键接口](../../crates/octara/src/app/keys.rs) · [批量配置回归](../../crates/octara/tests/integration/keys.rs)

`App::effective_keymap()` 提供平台默认项和应用配置合成后的只读快照。需要判断某个窗口实际能执行什么时，使用该窗口的焦点、命令资格与生效快捷键。[窗口命令解析](../../crates/octara/src/app/command.rs)

## Deferred commands

弹出菜单关闭后，延迟命令在待处理更新和焦点恢复后路由。`COMMAND_UNHANDLED` 表示恢复后的焦点路径和其余处理器均未处理该动作。检查实际窗口、焦点及菜单可用性。`COMMAND_QUEUE_LIMIT` 提示命令反复入队或批次超限，先定位相互触发的动作。[菜单用例](../../crates/octara/tests/integration/menus.rs) · [双文档验收场景](interaction-scenarios.md)

命令的查询和派发共用一套窗口解析。它检查当前焦点、可达范围、处理器所有者及编辑会话。组词和编辑提议结算期间，显式 `submit` 和编辑命令不可用。键盘快捷键若先结束组词，窗口在正文结算后解析动作。撤销、重做由接收编辑器的历史决定。已投递表示框架找到并交付了动作，应用可以通过模型接受或拒绝编辑。[解析与派发](../../crates/octara/src/app/command.rs) · [组词提交用例](../../crates/octara/tests/integration/keys.rs)

系统菜单在当前窗口更新后投影可用性和快捷键，工具栏通过 `Window::command_enabled` 绑定同样的资格。Kit 弹出菜单在打开时读取原焦点的资格，关闭后用恢复的焦点重新解析。点击系统菜单按命令名执行，菜单的键盘等价事件按当前键位解析。窗口级 Inspector 快照列出可用命令，拒绝的显式调用报告 `refused`。[菜单用例](../../crates/octara-kit/tests/integration/menus.rs) · [双窗口撤销回归](../../crates/octara/tests/integration/menus.rs)
