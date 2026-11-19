# 输入、编辑与命令

用户可以通过鼠标、触摸、键盘和辅助技术发起同一个动作。窗口掌握当前焦点和命中对象，组件负责激活后的行为。按压预览、消费事件和最终执行动作是可分别观察的时刻。[输入路径测试](../../crates/octara/tests/integration/activation.rs)

## Edit acceptance

`TextInput::value` 绑定应用接受的编辑文本。`El::a11y_value` 报告选择器等控件可供读屏的值。`numeric_range` 单独表达数值和范围，数值输入框同时保留正在编辑的原始文本。[编辑和选择值的测试](../../crates/octara/tests/integration/a11y.rs#L167)

`on_input` 收到本次编辑提议。应用通过绑定值接受、转换或拒绝提议，窗口在绑定稳定后更新选区与撤销历史。输入法提交的候选词进入同一条编辑路径。保存文件等业务操作由 `on_submit` 等业务动作触发。处理本次提议时向同一编辑器注入第二笔编辑会遇到 `EDIT_PENDING`，应在第一笔完成后再发起。[编辑提议的回归测试](../../crates/octara/tests/integration/edit.rs#L939) · [桌面重命名时序](interaction-scenarios.md#桌面文件重命名)

组词期间，`.value(..)` 对应最近接受的文本。组合区单独参与显示。重新绑定同一个已接受值会保留组合区与选区。绑定为不同值会主动替换文本并清除原组合，窗口在投递时拒绝旧文档上的原生编辑。需要在目录刷新期间继续编辑时，把输入绑定到编辑草稿，目录模型按自己的时序更新。[同值刷新](../../crates/octara/tests/integration/edit.rs#L1124) · [外部替换](../../crates/octara/tests/integration/edit.rs#L114)

Exact 的 Apple 宿主把带组合区的显示文本也发给应用，因此以显示文本判断回写是否相同，并把不同值延后到组词结束。Octara 的重命名场景通过独立草稿保留组词，应用可以在 `on_input` 接受候选词时做格式转换。[Exact 的组词与回写规则](https://github.com/expo/exact/blob/db89b50bb2037afc217b4b4f8a341338a4a5adf1/host/apple/Sources/ExactKit/FieldComposition.swift#L6-L29) · [系统输入法步骤](../../examples/IME.md)

### 受控文本框实测

macOS 27.2 上使用系统简体拼音、反向 emoji 选区和两个受控字段。应用值操作和旧提交重放由验收窗口的 Inspector 命令触发。[复现步骤](../../examples/IME.md#受控文本值)

| 操作 | 观察结果 |
| --- | --- |
| `A😀Z` 组词中刷新同值 | `ni hao`、原反向选区和组合光标保持。候选窗实例、首项「你好」、画面与屏幕范围 `[950,362,397,28]` 一致 |
| 应用将值改为 `R😀S` | 同一编辑器显示替换后的文本。组合清空，候选窗消失，光标落在 UTF-16 偏移 `4` |
| 系统选择「你好」，应用拒绝 | 收到一次 `A你好Z` 提议，接受次数为零。正文为 `A😀Z`，恢复 UTF-8 `[1,5]`、UTF-16 `[1,3]` 的原反向选区 |
| 组词期间改焦，随后重放旧提交 | 旧候选窗关闭，交付入口返回 `Refused`。第二框从 `B😀Y` 独立组词并提交为 `B你好Y` |

从第一框到第二框，Inspector 光标、AppKit 字符范围锚点、候选窗和 AX 字段边界均向下移动 52 点。候选窗屏幕 `y` 从 `362` 到 `414`，AX 字段边界从 `335` 到 `387`。两字段都能读取 AX 值和边界，`AXSelectedTextRange` 与显式首字符的 `AXBoundsForRange` 查询均返回 `kAXErrorNoValue`。文字范围的辅助技术反馈单独列入清单。[AccessKit 投影](../../crates/octara/src/kernel/a11y.rs#L451) · [AX 查询脚本](../../examples/ime_candidates.swift)

## Scroll routing

应用的 `scroll_to` 和 `scroll_to_end` 与窗口的定址滚动共用目标查找和滚动轴校验。按名称请求需要唯一的已挂载视口。各非零坐标分量都需要对应的滚动轴。返回值报告请求是否被接受，包括到达边界时。回到末端需要纵向滚动能力，并保留横向偏移。`scroll_into_view` 按元素位置交给其外层视口执行揭示。[轴校验](../../crates/octara/src/kernel.rs#L2309) · [定址滚动回归](../../crates/octara/tests/integration/scroll.rs#L48)

普通触摸平移越过阈值时捕获一个视口，后续位移和惯性由同一个视口处理。框架的 `wheel` 对每笔输入重新命中，选择允许沿相应轴滚动的最近视口。无窗口的嵌套视口对照中，内层剩余 `12` 时输入 `+30`，内层实际滚动 `12`，外层偏移保持 `0`。[滚动与平移入口](../../crates/octara/src/app/pointer.rs) · [偏移写入](../../crates/octara/src/kernel.rs)

在滚动视口上设置 `on_touch_scroll` 可以联动面板。回调在 `Before` 和 `After` 阶段接收有符号的 `[dx, dy]`，返回面板实际消费的部分。窗口先执行 `Before`，运行布局并夹紧偏移，再从新偏移开始滚动视口。若还有余量，则交给 `After`。每轴的消费都限制在输入的方向和幅度内。各处位置通过同一帧发布。窗口持有初始视口的身份，取消或卸载终止这次接触。联动直接触摸在抬手时结束。[面板验收时序](interaction-scenarios.md#移动地图的面板与列表联动) · [Compose 的嵌套滚动](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers#nested-scrolling-cycle)

Inspector 的 `touch_scroll` 记录最近一次完成的联动位移，包含视口目标、输入、前置消费、列表消费、后置消费与余量。`layout_correction` 记录前置回调后布局造成的偏移变化。消费与余量相加等于输入，几何位移另计布局校正。[Inspector 快照](../../crates/octara/src/inspect.rs) · [面板示例](../../examples/src/panel_scroll.rs)

桌面宿主需要保留平台可靠提供的手势阶段。Octara 当前在 `MouseWheel` 入口只保留位置和位移。macOS 上 winit 0.30.13 还把 AppKit 的直接阶段与惯性阶段折叠成同一个 `TouchPhase`。要验证触控板抬手后惯性是否归原视口，先在宿主采样可区分的两个阶段。无分段信息的离散滚轮仍可按单笔输入命中。[当前桌面入口](../../crates/octara-shell/src/lib.rs#L833) · [winit 的事件转换](https://github.com/rust-windowing/winit/blob/e9809ef54b18499bb4f2cac945719ecc2a61061b/src/platform_impl/macos/view.rs#L678-L700) · [AppKit 惯性阶段](https://developer.apple.com/documentation/appkit/nsevent/momentumphase)

## Shortcut configuration

`App::keymap()` 读取应用配置，`App::set_keymap(...)` 接受这份配置。`bind_keys` 和 `App::bind_keys` 走同一条注册路径，先解析整批，再应用到 App 的最新配置。窗口共享结果。默认绑定由平台生成，应用条目可以覆盖或显式解绑相应键位。[App 快捷键接口](../../crates/octara/src/app/keys.rs) · [批量配置回归](../../crates/octara/tests/integration/keys.rs)

`App::effective_keymap()` 提供平台默认项和应用配置合成后的只读快照。需要判断某个窗口实际能执行什么时，使用该窗口的焦点、命令资格与生效快捷键。[窗口命令解析](../../crates/octara/src/app/command.rs)

## Deferred commands

弹出菜单关闭后，延迟命令在待处理更新和焦点恢复后路由。`COMMAND_UNHANDLED` 表示恢复后的焦点路径和其余处理器均未处理该动作。检查实际窗口、焦点及菜单可用性。`COMMAND_QUEUE_LIMIT` 提示命令反复入队或批次超限，先定位相互触发的动作。[菜单用例](../../crates/octara/tests/integration/menus.rs) · [双文档验收场景](interaction-scenarios.md)

命令的查询和派发共用一套窗口解析。它检查当前焦点、可达范围、处理器所有者及编辑会话。组词和编辑提议结算期间，显式 `submit` 和编辑命令不可用。键盘快捷键若先结束组词，窗口在正文结算后解析动作。撤销、重做由接收编辑器的历史决定。已投递表示框架找到并交付了动作，应用可以通过模型接受或拒绝编辑。[解析与派发](../../crates/octara/src/app/command.rs) · [组词提交用例](../../crates/octara/tests/integration/keys.rs)

系统菜单在当前窗口更新后投影可用性和快捷键，工具栏通过 `Window::command_enabled` 绑定同样的资格。Kit 弹出菜单在打开时读取原焦点的资格，关闭后用恢复的焦点重新解析。点击系统菜单按命令名执行，菜单的键盘等价事件按当前键位解析。窗口级 Inspector 快照列出可用命令，拒绝的显式调用报告 `refused`。[菜单用例](../../crates/octara-kit/tests/integration/menus.rs) · [双窗口撤销回归](../../crates/octara/tests/integration/menus.rs)
