# 用交互时序推导原语

以下是设计样本和验收条件。它们独立于仓库里的演示应用，覆盖桌面鼠标、键盘、触摸和系统输入服务。每一行描述一个可以观测、也可以做错的转折点。

## 桌面文件重命名

文件搜索结果中，两个目录下的 A 和 B 都显示为 `A😀Z`。应用持有业务 key、已保存的名称与选中集合。开始重命名时，以 B 当时的名称初始化行内草稿，并记下名称的版本。`input().value(..)` 绑定这个草稿，`on_input` 接受一次文本编辑，`on_submit` 才发起文件重命名。窗口维护焦点、选区、撤销历史和输入法组合区。macOS 宿主用同一编辑器的文档与几何回答系统查询。[编辑器接口](input.md#edit-acceptance) · [行的身份](collection-regions.md)

| 时刻 | 契约与验收条件 |
| --- | --- |
| 多选 A 和 B，把键盘焦点移到 B 并开始重命名 | 选中集合与焦点可分别观察。草稿初值为 B 当前的文件名，业务写入次数为 0 |
| 反向选择 `😀`，开始组词 `nihao` | 已接受的草稿是 `A😀Z`，显示文本为 `AnihaoZ`。原选区为 UTF-8 `[1,5]`、UTF-16 `[1,3]`，保留选区方向并单独报告组合区内的选区 |
| 目录返回新快照，同一业务 key 的 B 移到首行 | 保留 B 的行、输入焦点、草稿、选区和 Agent `target`，按新位置计算光标。编辑器滚出视口时，由 `virtual_each` 保留仍持有焦点的行；回到视口核对光标与候选窗的位置 |
| 目录刷新带回同名值，或者 B 已在外部改名 | 同名刷新保留编辑会话。外部改名更新目录记录，草稿继续保留。应用提交前按 key 重读名称版本，发现冲突就显示当前名称并保留草稿。仅改变目录排序或其他元数据不构成名称冲突 |
| 选择候选词 `你好`，随后再按一次 Enter | 候选词只更新草稿为 `A你好Z`，业务写入次数仍为 0。无组合时的 Enter 才按 B 的 key 发起一次提交；A 的名称保持原样 |
| 用 Escape 取消，或将焦点从字段移开 | 组合期间按 Escape 由输入法取消组合，保留进入组合前的选区。组合结束后再按 Escape 取消本次重命名。本样本将焦点移到别处也取消重命名，业务写入次数为 0 |
| 组词期间删除 B，再打开同名新文件 C 的编辑器 | C 使用新的业务 key。卸载 B 的行即释放草稿和编辑会话。B 的迟到原生编辑被拒绝，旧 Agent `target` 返回 `STALE_TARGET` 且 `delivery` 为 `refused`。C 的名称、选区和组合区保持当前值 |

当前原语已能表达上述应用策略。`each` 和 `virtual_each` 按业务 key 保留行。已经开始的编辑在该行的状态里保存草稿，目录刷新经行数据更新。框架按当前编辑器与预期文档校验原生编辑，macOS 在投递前再检查会话代次。Agent 的 `target` 标识挂载的对象，业务提交还要校验当初观察的名称版本。[列表实现](../../crates/octara/src/view/list.rs) · [原生编辑入口](../../crates/octara/src/app/edit.rs#L374) · [macOS 会话校验](../../crates/octara-shell/src/text_input_mac.rs#L92) · [Agent 操作指南](agent-knowledge.md)

临时无窗口验证在 `each` 和 `virtual_each` 上各走通三条路径。重排保留目标和文档，候选词确认后只有应用的提交键修改 B。外部改名冲突保留草稿，删除重建后旧原生编辑和旧 Agent 操作均未修改 C。取消保留进入组合前的选区，退出后保存次数为 0。用虚拟列表回归测试另外核对编辑器离开视口后的身份与选区。[虚拟列表回归测试](../../examples/tests/integration/virtual_interaction.rs) · [原生编辑回归测试](../../crates/octara/tests/integration/edit.rs)

### macOS 实窗记录

在 macOS 27.2 上用系统简体拼音按键驱动编辑器。目录事件由验收应用的 Inspector 命令触发。候选文字和窗口范围来自按 PID 采集的系统窗口图像。[重命名验收入口](../../examples/IME.md#文件重命名)

| 场景 | 实测结果 |
| --- | --- |
| 组词、重排和滚动 | B 的焦点、令牌与反向选区保留。选区为 UTF‑8 `[1,5]`、UTF‑16 `[1,3]`。系统标记的文本为 `ni hao`。重排把光标从窗口内 `y=162` 移到 `118`。编辑器滚出视口并返回时仍使用同一会话 |
| 系统候选位置 | 旧候选窗首项是「你好」。重排前后范围都为 `[1032,432,397,28]`，单位是屏幕逻辑点，原点在左上。下一次组词键触发 AppKit 重新查询，收到向上 44 点的新坐标。既有候选窗仍停在原位。重新开始组词时，候选窗出现在 `[1032,388,397,28]` |
| 候选确认与提交 | 数字键 `1` 得到 `A你好Z`，草稿编辑 1 次，业务提交 0 次。下一次 Return 保存 B 一次，A 保持原名 |
| 外部改名与取消 | B 在组词时变为 `External B.txt`。确认候选后的 Return 报冲突，保留 `A你好Z` 草稿，保存数为 0。单独的取消用例中，第一次 Escape 恢复反向 emoji 选区，第二次退出重命名 |
| 删除 B 并打开 C | 修复前 C 已获得新编辑器，B 的候选窗仍可见。每个原生编辑会话使用独立 AppKit 输入上下文后，B 的候选窗在退役时消失。旧 `target` 返回 `STALE_TARGET` 和 `refused`。C 从初值开启新的组合，确认候选只更新 C 的草稿 |

真实窗口确认了系统候选窗在换主时的清理。向旧编辑器提交迟到原生编辑的拒绝由确定性测试覆盖。组词期间的候选窗位置另有同窗 AppKit 对照。[迟到编辑验证](../../examples/tests/integration/ime.rs) · [标准 NSTextView 对照](#标准-nstextview-对照)

#### 候选定位复测

这次复测保持同一个编辑器，使用系统简体拼音组词，再在内存目录中移动 B。各次重排都保持 `ni hao`、原反向选区和编辑器目标。应用内插入点从 `y=162` 到 `118`。候选窗在左上原点的屏幕逻辑坐标中保持 `[1032,432,397,28]`。

| 对照动作 | 实测结果 |
| --- | --- |
| 坐标失效后发送 `textInputClientDidScroll` | `firstRectForCharacterRange` 查询次数仍为 5，候选窗停在原处 |
| 在坐标失效前后发送 `textInputClientWillStartScrollingOrZooming` / `textInputClientDidEndScrollingOrZooming` | 通知对发送一次，查询次数仍为 5，候选窗停在原处 |
| 移动 AppKit 的代理 `NSView` | 原生视图原点也向上移动 44 点，系统仍沿用现有候选窗位置 |
| 重排后退格再补回 `o` | 系统查询次数从 5 到 7，收到向上 44 点的新坐标。原候选窗实例仍在 `y=432` |
| 在同一编辑器取消组合后重新输入 `nihao` | 新候选窗实例出现在 `[1032,388,397,28]`，首项为「你好」，编辑器令牌保持一致 |

重排后，AppKit 的下一次字符范围查询得到了正确的新位置。既有候选窗停在原处，重新组词时采用新位置。[AppKit 坐标失效通知](https://developer.apple.com/documentation/appkit/nstextinputcontext/invalidatecharactercoordinates()) · [字符范围坐标查询](https://developer.apple.com/documentation/appkit/nstextinputclient/firstrect%28forcharacterrange%3Aactualrange%3A%29)

#### 标准 NSTextView 对照

在 macOS 27.2 上，同一进程的同一个窗口先后使用 Octara 编辑器和标准 `NSTextView`。两者都以 `A😀Z` 为初值，选中 emoji 后用系统简体拼音输入 `nihao`，确认组合文本为 `ni hao` 且实际候选窗首项为「你好」。两次位移都保持组合和选区。候选坐标以屏幕左上为原点，单位为逻辑点。[同窗验收步骤](../../examples/IME.md#同窗-appkit-对照)

| 编辑器与动作 | 插入点位置 | 实际候选窗 |
| --- | --- | --- |
| Octara 编辑器随目录行向上移动 44 点 | 窗口内 `y=162 → 118` | 同一个窗口，`y=432 → 432` |
| 标准 `NSTextView` 向上移动 44 点 | AppKit 屏幕字符范围坐标 `y=1015 → 1059` | 同一个窗口，`y=431 → 431` |
| 标准 `NSTextView` 在新位置取消并重新组词 | 仍在移动后的位置 | 新窗口，`y=387`，比原位置向上 44 点 |

系统简体拼音在标准 AppKit 控件上复现了候选停留。Octara 在布局后继续发布最新几何，并保留当前编辑会话。这个对照覆盖 macOS 27.2 系统简体拼音的同窗位移场景。

## 移动地图的面板与列表联动

面板和列表共同处理一段触摸拖动。窗口在识别拖动后把这一段输入交给同一个列表视口。视口的 `on_touch_scroll` 回调按方向决定面板在哪个阶段参与。以下初始状态为面板距展开上限 12 个逻辑像素，列表位于顶部。向上推的位移为正。

| 时刻 | 分配与提交 | 可验证结果 |
| --- | --- | --- |
| 手指落在列表按钮上 | 按钮可预览按下，等待手势归属 | 抬手之前业务动作尚未执行 |
| 拖动越过阈值 | 窗口撤销按钮的按压预览，滚动视口接手 | 此次接触最终抬手时按钮触发次数为 0 |
| 首笔位移 `+30` | 面板展开 `12`，列表滚动 `18` | 两处实际消费之和为 `30`，剩余量为 `0` |
| 同一根手指反向，下一帧位移 `−25` | 列表先回到顶部 `−18`，面板收回 `−7` | 两处实际消费之和为 `−25`，剩余量为 `0`，视口保持同一接触 |
| 系统中断或视口卸载 | 窗口取消会话，仍挂载的参与者保留已经生效的位置 | 迟到的位移和抬手被忽略，点击次数和新启动的惯性次数均为 `0` |

再用列表接近底部的状态检验布局变化。列表偏移为 `95`，上限为 `100`。面板先展开 `12`，增大的视口把滚动上限降到 `88`，布局把偏移校正到 `88`。此时把 `+18` 余量交给列表，列表消费 `0`，剩余 `+18`。这次输入的面板消费为 `12`，`−7` 是布局修正。发布这一帧时同时核对列表的偏移、实际可见范围和命中位置。[窗口的滚动契约](input.md#scroll-routing) · [布局后的偏移校正](../../crates/octara/src/kernel.rs)

Exact 的桌面测试在过边界、抬手、惯性和所有者卸载时核对手势归属。Rune 的滚动锁也保留接收者与轴。Octara 的面板回调沿用一个视口持有接触的规则，分配发生在这一视口内部。[Exact 的触控板测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/ScrollRoutingTests.swift#L349-L413) · [Rune 的滚动归属](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/scroll/x.rs#L30-L63)

### 窗口内验证

无窗口测试达到上述分配并检查每笔位移只发布一次。前置回调内重建视口时，旧接触在布局后已经失效，后续消费也停止。macOS 的 420×460 窗口用 Inspector 注入相同的触摸序列，配对检查了分配报告、GPU 帧和命中。接近底部时，新视口上缘命中第 3 项，下缘命中第 5 项。取消及同名重建后的迟到事件保持点击计数为 0。[实窗复现步骤](../../examples/README.md#面板联动) · [分配回归测试](../../examples/tests/integration/panel_scroll.rs) · [回调内重建测试](../../crates/octara/tests/integration/pointer.rs)

### iOS 模拟器实测

iOS 27.2 的 iPhone 17e 运行同一 Rust 视图。构建收据与 Inspector 的运行摘要一致，内容视图为 390×844 点，设备截图为 1170×2532 像素。XCUITest 从辅助功能树找到「Place 2」，发出原生触摸。

- 向上拖动 48 点。列表视口从 100 增至 112 点，滚动偏移从 0 增至 36 点，操作前后为同一列表目标。Inspector 的面板下缘由 220 移至 208 点，设备截图对应 660→624 像素。点击计数为 0。
- 按住「Place 2」时让系统设置进入前台。窗口在失去活动状态时释放接触。之后 UIKit 送达同一接触的迟到位移和取消，列表高度与偏移保持 112 和 36。
- 返回同一进程，恢复活动后提交新帧。设备截图在抬手前已经显示解除高亮。释放后点击计数为 0，新的点按将计数增至 1。无窗口回归另外复核旧接触在恢复后仍不能滚动或点击。[模拟器复现](../../examples/README.md#ios-模拟器面板联动) · [活动状态回归](../../examples/tests/integration/panel_scroll.rs)

## 资料审阅队列

用公开视图接口和普通 Rust 状态组装审阅队列。决定列表保存已通过的意见，Signals 派生待审和已通过列表。详情以文档 ID 挂载，编辑草稿属于当前详情。申请通过时由应用检查意见，控件的 `invalid`、说明和状态文本从同一校验结果派生。[实现](../../examples/src/bin/review_queue.rs)

macOS 实窗通过 `octara launch` 生成构建收据，再用 Inspector 检查并操作该实例。选择「导航说明」后空提交，回执为 `delivered`，应用显示「请填写审阅意见。」，仍有三份待审。在实际窗口缩到 390×600 后，补齐意见并再次提交。列表变成两份待审、一份已通过，原详情的操作目标返回 `STALE_TARGET`。切到已通过列表，从详情读回已保存的意见。GPU 抓帧和窗口诊断检查通过。[运行方式](../../examples/README.md#资料审阅)

试跑时依据单行角色查找编辑器，漏掉了实际的 `multiline_text_input`。用名称和 `input` 动作可以直接定位。重试旧目标时，标准输入模式给出了 `ok: false` 和 `delivery: refused`，会话退出码为 0。两处用法已补入 Agent 指引。[反馈契约](agent-knowledge.md#inspector-protocol-and-checks)

## 用另外三个场景找遗漏

| 场景 | 观察值 | 能排除的错误设计 |
| --- | --- | --- |
| 平板分屏中的中文表单，软键盘出现后窗口再缩窄 | 已提交值、组合区、选区、可视光标和候选窗位置 | 用设备型号决定布局，或每次重排重建编辑器 |
| 两个桌面文档各有撤销栈，从工具栏、菜单和快捷键执行撤销 | 当前窗口、实际动作接收者、撤销可用性和修改后的文档 | 把命令保存在进程级单例，或给三个入口复制三套判断 |
| 手机相册交互式返回，进度过半后取消，再次返回并完成 | 原页面、阅读位置、可操作目标和资源释放次数 | 只要开始返回就释放页面，或取消时重新挂载同名页面 |

先在确定性宿主断言状态与顺序，再用真实窗口或模拟器核对系统参与的环节。无窗口测试、窗口 Inspector 注入和系统输入分别记结果。待实现的移动宿主不能产生模拟器验收结果。[路线图](../ROADMAP.md) · [现有验证入口](../../examples/README.md)

## 手机相册的交互式返回

两页相册使用 Rust 状态选择详情。列表保持挂载，详情的 Back 按钮和视图局部的 `navigation.back` 命令调用同一个关闭回调。iOS 宿主在已有列表帧的基础上装配 UIKit 返回转场，原来的 Metal 视图继续绘制详情，列表帧只作为转场预览。UIKit 的终止结果进入同一个 Core 输入会话。Exact 同样把侧滑结果交给 UIKit 的转场协调器，并带着原页面身份处理完成。[相册实现](../../examples/src/album_back.rs) · [返回会话](input.md#interactive-back) · [Exact 转场协调](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/NavigationIOS.swift#L1210-L1248)

无窗口回归把返回进度推进到 `0.65` 后反向取消，详情、滚动视口、Back 操作目标和焦点仍对应原挂载，列表与详情的滚动偏移保持原值。下一次返回完成，关闭回调和详情清理各一次，焦点回到列表原行，旧 Back 操作目标返回 `STALE_TARGET`。另一条回归在侧滑开始后关闭并重开同一详情，核对原手势无法关闭新实例。[相册回归](../../examples/tests/integration/album_back.rs) · [模拟器复现](../../examples/README.md#ios-模拟器相册返回)

### iOS 相册实测

iPhone 17e、iOS 27.2，视口为 390×844 点，设备截图为 1170×2532 像素。用构建收据确认被测实例，使用 AXe 及其 Simulator HID 驱动发出系统触摸，使用 Inspector 读取挂载目标和状态。先滚动列表并打开 Breakwater，再滚动详情。

- 同一次侧滑先向右推进，设备截图中详情向右移动 690 像素，露出约 59% 宽度的列表预览。随后反向收回并抬手。UIKit 报告交互式取消，详情、滚动视口和 Back 目标保留原挂载，焦点仍是 Photo notes，Location 的纵坐标保持 224 点，关闭和释放计数均为 0。取消前后的应用内容像素一致，头部只显示应用的 Back 按钮。
- 再次侧滑，UIKit 报告交互式完成并投递一次 Back。列表与 Breakwater 行保持原目标及滚动位置，Breakwater 的纵坐标仍为 221.7 点，焦点回到该行。关闭和释放计数各为 1，尝试操作旧的详情 Back 目标收到 `STALE_TARGET`。回到列表后，计数栏以外的应用内容像素与打开详情前一致。

### 模态辅助功能范围

在同一型号和系统版本的模拟器，用原生触摸打开 Breakwater，分别读取 Inspector 和 iOS 原生辅助功能树。详情打开前，系统树有 12 个照片按钮。打开后，系统树保留详情、Back 和 Photo notes，照片按钮退出可达范围。点击 Back 后 12 个照片按钮重新出现，Inspector 恢复到 `album-photo-6` 的焦点、原列表和行的 `target`，旧详情 Back 请求返回 `STALE_TARGET`。除状态栏和计数栏外，列表回到打开前的像素。[模态规则](ui-primitives.md#5-overlays)

在 iOS 27.2 模拟器用 Xcode 的设备控制启用 VoiceOver。设备截图中，系统焦点框位于 Back。发出单指右扫后，焦点框移至 Photo notes。再右扫两次，焦点框仍停在详情内，原生辅助功能树也保持同一范围。[Xcode 27 发行说明](https://developer.apple.com/documentation/xcode-release-notes/xcode-27-release-notes)

macOS 的 `showcase interaction` 用 Inspector 驱动 Dialog 和编辑器的非模态建议列表。验证建议的键盘选择、关闭顺序和焦点恢复，并核对原生窗口缩放及 GPU 绘制帧。[实窗脚本](../../examples/live_primitives.py)

iOS 27.2 运行同一个 `showcase interaction`。原生触摸打开 Dialog 后，原打开按钮退出原生辅助功能树，编辑器取得焦点。点击系统键盘上的 `/` 后，原生树能读到建议列表、`/review`、禁用的 `/deploy` 和 `/summary`。点按 `/review` 后，应用回报 `已选择：review`，建议列表关闭，编辑器保持焦点。[模拟器复现](../../examples/README.md#ios-模拟器嵌套弹层)

### iOS 键盘收起与点击资格

修复前，在键盘展开时按住「完成」中心 `(195, 427)`。同一接触的 `Down` 和 `Up` 都在这个坐标。两者之间，焦点已转到「完成」，原生编辑器退出第一响应者，键盘收起使窗口可用高度从 515 增为 844 点。按钮的上缘从 405 移到 570 点，抬手时原坐标已落在编辑器上。再次点击新位置才关闭 Dialog 并恢复原打开按钮的焦点和 `target`。[模拟器复现](../../examples/README.md#ios-模拟器嵌套弹层)

修复后，在相同的 iPhone 17e、iOS 27.2 模拟器上，确认键盘显示后在同一坐标发送一次 80 毫秒的原生触摸。轨迹只有同一接触的 `Down` 和 `Up`。抬手后 Dialog 关闭，键盘收起，原打开按钮恢复焦点、原 `target` 和操作资格，并重新出现在系统辅助功能树中。[触摸焦点时序](input.md#pointer-sessions)

按住「完成」时，在 `Down` 与 `Up` 之间核对编辑器仍是第一响应者，可用高度保持 515 点，按钮坐标和目标保持原值，原处抬手关闭 Dialog。手指从 `(195, 427)` 拖到 `(195, 360)` 抬起时，Dialog、键盘和编辑器焦点都保留。读取已关闭的旧「完成」目标得到 `STALE_TARGET` 与 `refused`。

- Exact 的 iOS 路径覆盖了键盘收起导致工具栏移位的同类问题。它先在抬手时确认命中，再交接焦点，最后复核触摸会话和目标的挂载实例。Octara 在 Core 沿用这个顺序，复用窗口的更新队列等待失焦回调和布局结算。[Exact 触摸结束](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/PointerIOS.swift#L478-L510)
- Rune 将触摸先作为待判定的会话处理。系统取消或滚动接管会取消按压，抬手时检查捕获的路径仍然存活、会话仍为命中状态。Octara 同样保留拖离、取消和原目标的检查。[Rune 触摸会话](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/touch.rs#L82-L128) · [Rune 释放目标](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/pointer.rs#L314-L352)
- Expo 的 React Native 示例在按钮的 `onPress` 中调用 `Keyboard.dismiss`。React Native 的 `Pressable` 区分按压反馈和抬手后的操作，`ScrollView` 则通过 `keyboardShouldPersistTaps` 决定容器是否拦下点击来收起键盘。Octara 在这个场景由 Core 确认点击并交接焦点，Dialog 可以继续用普通按钮关闭。[Expo 键盘处理](https://docs.expo.dev/guides/keyboard-handling/#keyboard-events) · [React Native Pressable](https://reactnative.dev/docs/pressable#how-it-works) · [React Native ScrollView](https://reactnative.dev/docs/scrollview#keyboardshouldpersisttaps)

### iOS Dialog 内的中文虚拟选择器

验收窗口在 Dialog 内打开 121 个选项的 Combobox。输入「你好」后剩 120 个匹配项，可以从首项按 Up 循环到尚未挂载的末项 `greeting-119`，再重排到首项。应用单独记录提交次数和业务 key。[复现步骤](../../examples/IME.md#dialog-内的中文虚拟选择器)

基线中，iPhone 17e 模拟器的拼音键盘把视口高度压到 515 点，弹层顶边到达 `y=8`，部分搜索框进入状态栏。揭示列表末项时外层弹层也滚动，搜索框从 `y=17` 移到 `y=-45`，焦点还在搜索框，但屏幕上已经看不到它。弹层定位应结合宿主报告的安全区域与键盘上沿，列表视口应使用搜索框以下实际可用的高度。[Exact 的定位区域](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/MenusIOS.swift#L749-L792) · [Rune 的搜索与结果布局](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L333-L338)

修复后的 iOS 27.2 模拟器上，拼音组词期间点按系统键盘的「确认」会提交 `nihao`，选择次数保持零。清空后重新组词，点按系统候选「你好」，得到查询 `你好` 和 UTF-16 光标 `[2,2]`，选择次数仍为零。系统键盘保持显示，弹层位于 `y=298`，搜索框和结果视口分别位于 `y=307` 和 `y=359`，列表高 139 点。

通过 Inspector 发送 Up 和重排命令，两步均返回 `delivered`、`settled` 和 `submitted`。原先未挂载的 `greeting-119` 在向上循环后成为 `active_descendant`，行显示在 `y=454`。重排后活动项保持同一个业务 key，行显示在 `y=359`。搜索框的挂载身份、UIKit 第一响应者与位置保持一致。模拟器的硬件 Up 模拟未产生窗口按键，本次列表导航证据来自 Inspector。[回执契约](agent-knowledge.md#inspector-protocol-and-checks) · [组合回归](../../examples/tests/integration/ime.rs)

最后点一次系统键盘的「换行」，选择次数从零变为一，提交 `greeting-119`。列表和搜索框卸载，焦点恢复到选择器触发器，Dialog 保持打开。键盘收起后窗口恢复为 `390×844` 点，读取 Inspector 确认宿主已提交画面。无窗口回归还覆盖选择器保持打开时的同一尺寸变化，结果列表恢复到声明的 240 点高度。[组合回归](../../examples/tests/integration/ime.rs)

## 两个同标题文档的撤销

启动 `target/debug/window undo` 时设置独立的 `OCTARA_INSPECT` socket。文档 A 的「Open Document B」按钮打开第二个同标题窗口。两个编辑器都用 `editor` 作为查找名，分别持有文档值和撤销历史。`edits` 统计应用收到的文本写入，工具栏的可用性读本窗的 `command_enabled("edit.undo")`。Inspector 用窗口编号和从快照获得的窗口 `target` 调用 `edit.undo`。[示例实现](../../examples/src/apps.rs) · [命令回归](../../crates/octara/tests/integration/menus.rs)

macOS 27.2 实测先在 A 输入 `A1`，再打开 B。B 的空历史使系统 Edit 菜单禁用 Undo，即使 A 有可撤销的编辑。依次检查以下路径。

| 操作 | 实窗回读 |
| --- | --- |
| B 输入 `B1` 后按 `⌘Z` | B 恢复为 `B`，编辑计数从 1 到 2 |
| B 输入 `B2` 后点系统 Edit → Undo | B 恢复为 `B`，编辑计数从 3 到 4。此时 A 为 `A1`，编辑计数为 1 |
| 切回 A，观察系统菜单，再点窗口内的 Undo | 系统 Undo 随 A 启用。点击后 A 恢复为 `A`，编辑计数从 1 到 2，焦点仍在原编辑器，系统 Undo 再次禁用 |
| A 保持原生焦点，通过 Inspector 向 B 写入 `B3` 再撤销 | B 恢复为 `B`，编辑计数从 5 到 6。第一次撤销回执为 `delivered`，空历史上的第二次为 `STEP_FAILED/refused`。A 保持原生焦点与两次文本写入的状态 |
