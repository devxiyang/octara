# Octara 自底向上的架构复核

基线为 `init@bd26ae6`。复核本地 Rune `3686e94`、Exact `08c889a9`，并在
[Rune dev `10a39584`][rune] 与 [Exact main `1dcad174`][exact] 核对相关实现。
结论来自代码路径与现有测试。分层检查通过。本轮为静态评审，下面列出运行验收应验证的现象。

## 各层职责

| 层 | 保留 | 下一处边界 |
| --- | --- | --- |
| Primitives / text / render | 独立的几何、像素、排版与绘制数据。CPU 参考和 GPU 后端都消费显示列表 | 有效图片和 GPU atlas 容量分开处理。栅格缓存的预算由渲染器负责 |
| Signals / task | 动态依赖、显式 owner、代次、任务取消和 shutdown 继续收尾 | 异步结果提交失败时明确如何交还或延后。避免任务结束但值永远停在 Pending |
| Window / Kernel | Window 协调更新，Kernel 保留节点、编辑事实、布局与语义。重载按存活 owner 路径恢复命名状态 | 窗口创建和关闭统一执行准入与 app 帧内队列。挂载完成后才能发布 |
| View / Kit | 普通 Rust 函数组装 `View`，容器拥有孩子。可选 Kit 提供 Model、键盘、焦点和可访问语义的组合 | 控件配置落到真实交互节点。共享焦点等动作根默认值 |
| App | Chat 持有请求、附件与输入状态。控件展示该状态，并提出操作 | 会话队列随会话 ID 切换。点击、键盘及可访问动作共用发送资格 |
| Shell / Agent | OS 输入、原生视图归 Shell。Inspector 查询本有事实，构建收据和 `target` 各有校验入口 | 请求交出输入前重新确认目标。截图回包归当次截图，诊断继续区分构建、操作和实际呈现 |

重载修复仍成立。旧名字在调用析构前全部撤销，同路径的值在新 owner 挂载后恢复，
包含 handler 首次调用才访问的状态。[scope.rs](../../crates/octara/src/reactive/graph/scope.rs#L252-L297)

Rune 在首次显示之前完成内容测量，有隐藏窗拟合高度的专门验收。其帧报告区分已绘制、无需重画
和跳过。Exact 的文档提交只有在成功结束时取得 receipt 中的 epoch。可用这样的阶段边界定义
Octara 的首次平台提交。Exact 对原生请求按 token 等待，
对输入在执行前复核实际目标。Octara 已有 Signals、窗口 owner 和执行准入，可在各自边界
落实这些规则。[Rune 首帧用例][rune-first] · [Rune 帧报告][rune-frame] · [Exact 文档提交][exact-commit] ·
[Exact 原生输入][exact-input] · [Exact 原生截图][exact-capture]

## App 窗口生命周期

从 handler 调用 free `open_window`，先创建平台窗口并把 ID 加进 `order`，再登记待挂载。
`frame_except` 先遍历 `order` 做 prepare/present，后处理 `opening`。因此一个声明了内容的
新窗口可以先把空树交给平台，再挂载根。winit 有重绘合并，需用宿主记录验证提交次数，
实际屏幕是否闪空白需另看真实窗口。
[插入与打开队列](../../crates/octara/src/app.rs#L826-L876) ·
[遍历与挂载顺序](../../crates/octara/src/app/frame.rs#L19-L59) ·
[平台提交](../../crates/octara/src/app/presentation.rs#L333-L375)

同一条路径还有两处归属问题。挂载根回调 `settle_frame`，后者又进入 `frame_except`。
根挂载中可以合法注册首次立即执行的 watch，callback 可再请求开窗，新的递归调用会重新获得
16 轮窗口处理预算。另一个问题是开关窗入口主要检查 App 是否运行中，未复用调用者的
执行权限，因此可从绑定或 memo 触发框架可见的窗口副作用。
[挂载与帧入口](../../crates/octara/src/app/frame.rs#L72-L99) ·
[窗口入口](../../crates/octara/src/window.rs#L17-L26) ·
[执行准入](../../crates/octara/src/reactive/graph/execution.rs#L103-L125) ·
[执行契约](runtime-contract.md#structure)

建议先让 App 外层更新负责消费开关窗队列，让挂载只做该窗口的准备。开关窗纳入已有的
调用者准入，明确宿主初始化和 handler 请求两条调用方式。验收请记录首个 host frame 的内容，
并覆盖纯阶段请求、不递归的开窗链、打开后同一轮关闭、正常 handler 开窗和现有重载收尾。
现有 `a_handler_opens_a_window_at_the_end_of_its_frame` 校验的是最终内容。
[现有测试](../../crates/octara/tests/integration/windows.rs#L285-L305)

同一外层更新还需覆盖没有窗口的 App。`App::poll` 的无窗分支只 drain task，task 写入后
可能留下 graph 中待传播的 watch，`App::settle` 又可以在任务 idle 时返回。现有异步测试
在两次 poll 之间特意调用 `app.run(|| ())` 完成一次传播。建议同一组回归覆盖启动前与最后
一窗关闭后，验证 task 改 App 状态后 watch 可在该次更新中执行。
[无窗 poll 与 settle](../../crates/octara/src/app/time.rs#L123-L155) ·
[现有异步用例](../../crates/octara/tests/integration/task.rs#L599-L607)

### 第一阶段实现

App 外层更新统一消费开关窗请求。根挂载不再触发递归更新；首个宿主提交在根挂载和布局完成后
发出。开关窗沿用现有执行权限，同轮关掉的窗口跳过挂载。关窗先撤销窗口及子树的执行资格，
当前回调返回后再释放。连续开窗耗尽 16 轮时，剩余请求可从 `App::pending()` 观察并在下一轮推进。

显式 `App::poll` 在有窗和无窗时都推进 ready task 与其触发的响应式更新。嵌套 poll 等到调用者
归还借用后再推进，覆盖了 `AsyncMemo::with` 期间就绪结果遗留 Pending 的场景。
宿主记录测试检查第一个提交中的原生视图创建和可见位置；系统窗口的实际呈现仍需独立验收。
[窗口契约](windows.md#api-gpuis-names-octaras-free-functions) ·
[更新契约](runtime-contract.md#an-update)

## 原生采样和动作投递

### 截图结果应归当次请求

原生 WebKit/MapKit 截图回包当前只携带 `NativeId`。截图 A 超时结束后，A 的回包仍能写当前
节点的 `snapshot`，即使本轮没有活跃截图。截图 B 请求同一视图，看到 `snapshot.is_some()`
即认为 B 的回包已完成，可以用 A 的像素报告成功。正常显示列表也会读取这个字段。
[AppKit 回包](../../crates/octara-shell/src/native_mac.rs#L551-L592) ·
[等待和接受条件](../../crates/octara/src/app/presentation.rs#L171-L235) ·
[原生节点写入](../../crates/octara/src/app/presentation.rs#L297-L303) ·
[正常绘制](../../crates/octara/src/kernel/paint.rs#L471-L492)

Exact 为每次原生截图分配 token，超时后删除等待记录，迟到回包查不到等待者。本次图像放在
调用局部的结果集合。Octara 可以让现有 capture job 持有请求身份及快照结果，绘制截图时消费
该集合，并删除常态节点的截图暂存。WebKit/MapKit 继续按现有事件循环异步返回。
用可控 host 验证 A 超时、B 开始、A 抵达、B 抵达的顺序，另测缩放、关窗及正常显示列表。
[Exact 等待与撤销][exact-capture] · [Exact 回包检查][exact-reply]

### 缩放要在实际投递前复核

Live Inspector 解出 `target` 后会 tick/poll，期间任务可以重载同一窗口。大多数操作在使用
目标前复核。当前 `size` 直接进入 `request_inner_size`，直到后续轮询才发现窗口树已经换过，
因此可能实际改了尺寸并回报 `STALE_TARGET/delivered`。用已有 `TargetGuard` 在 tick/poll 后、
第一个宿主副作用之前复核，保留长操作在后续阶段的检查。
验收分别覆盖投递前失效，以及投递后等待期间失效，分别检查 refused 和 delivered。
[Inspector 分派](../../crates/octara-shell/src/inspect.rs#L266-L321) ·
[缩放副作用](../../crates/octara-shell/src/resize.rs#L28-L43) ·
[轮询复核](../../crates/octara-shell/src/inspect.rs#L440-L450) ·
[Exact 复核](https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/host/apple/Sources/ExactKit/NativeModule.swift#L1062-L1096)

R3 继续用相同构建测试真正的系统拼音、候选内容和位置、接受/取消、缩放和重开。系统截图
用于候选窗，Inspector 提供编辑事实。构建收据验证进程运行的产物，`target` 校验当前树和
节点是否还在。截图请求身份只回答回包归属。需要比对采样时先用能复现的事件顺序约束。
[R3 清单](editing-architecture-review.md) ·
[构建收据](agent-context.md#verify-the-running-build)

### 第二阶段实现

窗口只保留当前 painted capture 的收集状态。宿主原样回传每个原生视图的请求，匹配且仍在
等待的回包只会被消费一次。旧截图完成或取消时也要匹配截图身份。超时、窗口重载和关闭后
抵达的回包不会写入后续截图。窗口收到尺寸或比例变化时撤销旧尺寸的像素，结果按新布局绘制，
并在报告中标记失效的原生快照。实时显示列表继续独立生成。

Inspector 在推进 App 工作后复核缩放目标，确认仍有效才请求宿主调整窗口。已投递的操作
在等待时继续复核，失效回包保留 `delivered`；投递前失效返回 `refused`。
[截图协议](capture.md#painted-default) · [Inspector 目标规则](agent-context.md#targets-and-observation-identity)

可控宿主覆盖超时后重试、重复回包、显式失败、缩放、重载、关闭及实时绘制缓存。
真实 macOS WebKit 示例用 Inspector 从宿主取得 680 × 640 的调整结果，连续两次 GPU
截图为 1360 × 1280。截图报告均包含 WebKit 快照，视图内取样与页面背景色一致。
重载后用旧目标请求另一个尺寸返回 `STALE_TARGET / refused`，视口维持 680 × 640。
独立的 `OCTARA_CAPTURE` 入口保存含 WebKit 快照的 PNG 后正常退出。
在准备及等待期间失效的精确时序由可控测试验证。

## 其他层的验证候选

- **GPU 图片容量。** `Image` 接受合法的 2560×1440 RGBA，CPU 可绘制。GPU 按原图尺寸放进
  单张 2048×2048 atlas，缩成小缩略图也一样进不去。重建全部 atlas 再试仍会跳过实例，两张
  1920×1080 同屏也可能触发这条路径。先在 CPU/GPU 共用场景验收，考虑有界分页或独立纹理
  路径。Rune 的 renderer 为适合设备限制的较大图分配相应页面，并回收闲置页。Exact 按原图
  或采样尺寸上传独立纹理，并在帧边界清掉未使用的项。容量政策可以留在 Octara GPU 后端。
  [Octara 分配和重试](../../crates/octara-render/src/gpu.rs#L378-L434) ·
  [Rune 图片页][rune-images] · [Exact 纹理尺寸与回收][exact-texture]
- **栅格字形留存。** CPU 和 GPU 持有的 CPU bitmap 缓存按字体、字号精确位和变体等键累计，
  GPU 重置 atlas 只清 slot 映射。先用字体/DPI 多档切换测条目和字节量，再参考 Rune 的帧边界
  回收和字节预算。Rune 名为 GlyphCache 的段落重播缓存与栅格 bitmap 预算各自独立。
  [Octara bitmap cache](../../crates/octara-render/src/glyphs.rs#L26-L74) ·
  [Octara atlas 重置](../../crates/octara-render/src/gpu.rs#L333-L340) · [Rune 栅格预算][rune-raster]
- **异步派生结果。** App 持有无窗口的 `AsyncMemo` 时，若 `memo.with` 的借用期间重入
  `app.poll`，就绪 future 可能正好完成。内部 `transition` 调用的 `Signal::update` 因借用冲突
  诊断后返回，任务仍结束，遂遗留 Pending。先以可控 future 复现，再决定是拒绝这类重入，
  还是由当前任务持有结果到可提交时间。两种方式都应保留取消及过期请求检查。
  [poll](../../crates/octara/src/app/time.rs#L123-L136) ·
  [AsyncMemo 提交](../../crates/octara/src/task/async_memo.rs#L124-L137) ·
  [借用规则](../../crates/octara/src/reactive/graph.rs#L565-L592)
- **Chat 会话队列。** 持续挂载的 composer 用 `untrack` 读取当前会话来筛选增量列表。
  切换会话只写 `current`，`ReadList::filter` 不会为已有队列项重算。A 回答中排队、再切 B，
  B 仍可显示 A 的待发送行与动作。将可观察的会话 ID 移到筛选区域的入口。增量谓词只使用该
  区域捕获的固定 ID 与 item。Rune 的 Combobox 在同一次构建使用选项与查询，Exact 的示例
  用完整派生表达式让列表和查询都成为依赖。相应的 Octara 实现可在区域层跟踪会话，
  保留增量谓词只依赖单项和固定参数的规则。验收 A → B → A，观察列表和行目标随会话切换，
  composer 保持挂载。
  [composer 挂载位置](../../apps/chat/src/ui.rs#L309-L311) ·
  [队列筛选](../../apps/chat/src/ui.rs#L861-L881) ·
  [会话切换](../../apps/chat/src/chat.rs#L268-L272) ·
  [filter 更新规则](../../crates/octara/src/reactive/list.rs#L807-L826) ·
  [Rune 选项与查询][rune-query] · [Exact 列表派生][exact-query]
- **组合控件与 Chat。** Kit 的文字按钮是叶节点，图标按钮是容器。Chat 欢迎卡自己拼角色、
  名称、焦点、回调，但漏掉 Kit 的可见焦点样式。可以让 Kit 的选择性组合共享动作根默认值，
  用现有欢迎卡检验。附件 chip 的 ID 目前落到外壳，实际移除按钮只有可能重复的名称。
  普通 builder 组合应让调用者配置真正的动作节点。Chat 还把纯附件草稿的发送按钮禁用，
  而 Enter 的 `Chat::send` 接受它。发送资格应在 Chat 汇合。Rune 让富内容与按钮语义名称
  分开配置。Exact 的主题卡也用同一个动作根持有按下动作、可访问名称和 test ID。
  先用现有调用场景检验动作根的复用和命名，再定组件 API。
  [Kit 按钮和可见焦点](../../crates/octara-kit/src/lib.rs#L74-L152) ·
  [欢迎卡](../../apps/chat/src/ui.rs#L543-L567) ·
  [附件动作](../../apps/chat/src/ui.rs#L904-L933) ·
  [发送按钮](../../apps/chat/src/ui.rs#L982-L988) ·
  [发送接受条件](../../apps/chat/src/chat.rs#L311-L321) ·
  [Rune 按钮内容][rune-button] · [Exact 主题卡][exact-button]

建议先用现有平台协议复现窗口生命周期和原生回包的差异。首个实现阶段收敛 App 更新，
覆盖有窗、无窗、开窗与关闭，再处理宿主投递和截图结果。Chat 队列、纯附件发送与图片容量
可各自用具体场景验证。R3 验收按系统输入窗口的可用时间安排。字形回收以实际数据确定预算。

## 第三阶段实现

Chat 的队列区域以会话 ID 跟踪挂载，区域内保留增量筛选。切走时只退休队列行及其动作目标；
会话的请求继续由模型持有。返回时从模型恢复可见队列，输入框身份和编辑数据保持。
发送按钮通过 `Chat::can_send` 读取发送资格。该检查与 `Chat::send` 共用内容、模型和参数规则，
鼠标、键盘和无障碍动作走同一模型入口。

Kit 的 `button_root` 让欢迎卡继承标准按钮的键盘焦点样式，卡片仍自定义布局和视觉内容。
`removable_chip` 的配置闭包接收实际移除按钮，Chat 将每个附件的动作 ID 设在该按钮上。
真实 macOS Chat 用可控模型验证了 A → B → A、旧目标拒绝、同名附件移除、纯附件发送及
Tab 聚焦和 Enter 激活欢迎卡。GPU 截图中的卡片边框颜色与焦点 token 一致。文件选择通过
Inspector 回答实际窗口的请求。
[Chat 验证入口](../../apps/chat/README.md#request-lifecycle-verification) ·
[Kit 接口](ui-primitives.md)

## 内容原语与挂载边界复核

重读当前代码时，以“谁能创建这个事实、谁能更新它、谁负责结束它”检查各层。
以下是对先前[原语推演](core-primitives-derivation.md)和[内容契约](core-primitive-contracts.md)的实施取舍。

| 边界 | 保留 | 收敛决定 |
| --- | --- | --- |
| 基础值与资源 | 几何、样式、编辑协议；共享图片资源 `Image`、截图结果 `Picture` | 图片资源归像素生产者。UI 的 `image(&pixels)` 创建一个可布局、命名和检查的节点 |
| Signals 与 owner | `state`、`memo`、`watch`、只读句柄、按 key 的 owner 和退休后的释放 | 派生值由 Signals 传播。动态结构需要在调用用户代码之后重新确认 owner 的执行资格 |
| 声明与保留树 | 普通函数返回 `El` 或 `View`；容器接收孩子；文本管理内部链接 | 构造时确定内容种类。删除先建 Box 再初始化图片、图标、画布、原生视图的路径 |
| 窗口与输出 | Window 路由输入、协调布局与发布；Inspector 和宿主读取相同节点 | 内容种类、可访问角色、能否持有应用孩子分别有各自的来源。图片可保持节点身份并由显式角色表达按钮等用途 |
| Kit 与应用 | 视觉外观和复合交互继续由普通 Rust 函数组装 | 图注作为容器中的兄弟元素；图片、编辑器和复合控件共用样式、事件和语义接口 |

Rune 的普通函数返回同一个可组合元素。Exact 将内容类别、可容纳孩子和测量分别判断。
Octara 采用统一的 Rust 组合方式，同时只让容器拥有应用孩子。[Rune 元素构造](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el.rs#L319-L356) · [Exact 节点契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/node.rs#L69-L147)

### 内容从创建起确定

此前 `El::img` 可以把空容器改成图片。图片、图标、画布、原生视图在内核的创建日志中都写作
`Box`，随后再用 setter 替换内容。画布要等到执行绘制绑定才取得实际内容类型。
现在每种节点在创建时已经持有正确的内容，独立 Kernel 的调用者也遵守这份契约。
`Kernel::kind` 和创建日志区分这些种类，富文本链接只由文本创建。画布更新只写绘制指令，
原生属性更新沿用当前宿主实例。旧的 `Kernel::create(Kind)`、跨种类 setter 和 `El::img` 已删除。

作者用 `image(&pixels).object_fit(ObjectFit::Contain)`。共享句柄继续通过 `Image::set` 发布像素。
图片默认的可访问角色从 GenericContainer 改为 Image，调用者可以显式赋予其他角色。
布局尺寸、圆角、图注、标签和事件仍由相同的 builder 组合。所有内容构造函数都返回 `El`，
让样式和行为函数仍只接收一个元素类型。删除改种类的操作可以在编译期挡住旧写法；
同一 `El` 上的 `child`、`object_fit` 等专用方法仍在调用时检查内容。
[节点创建](../../crates/octara/src/kernel.rs) · [内容构造](../../crates/octara/src/view/element.rs) · [作者契约](ui-primitives.md#element-content)

### 下一处像素边界

在自然尺寸或等比适配的图片上，样式解析和缓存的绘图几何使用当时的资源尺寸，绘制器可以稍后读取
同一图的新像素。应先验证横图换竖图、空图变非空、同图多处展示和旧显示列表重画，然后决定何时
让布局和绘制取得同一版资源。独立验证单图超过图集和多张图挤满图集两种情况，再选择 GPU 的容量政策。
同尺寸更新、跨尺寸更新和 GPU 容量各自需要观测证据。
[布局](../../crates/octara/src/kernel.rs) · [绘图缓存](../../crates/octara/src/kernel/paint.rs) · [GPU 上传](../../crates/octara-render/src/gpu.rs)

[rune]: https://github.com/anomalyco/rune/commit/10a39584cc1ff739477be52e333a742a024ac956
[exact]: https://github.com/expo/exact/commit/1dcad174583dfee6d2690af9a5d67a3538ef7c31
[rune-first]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune/src/runtime/tests/window_wake.rs#L75-L106
[rune-frame]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune/src/frame/encoded.rs#L23-L53
[rune-images]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune/src/image/pixels.rs#L327-L425
[rune-raster]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune/src/text/atlas.rs#L313-L360
[rune-query]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune-kit/src/combobox.rs#L190-L217
[rune-button]: https://github.com/anomalyco/rune/blob/10a39584cc1ff739477be52e333a742a024ac956/crates/rune-kit/src/actions.rs#L266-L277
[exact-commit]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/kernel/src/kernel/document.rs#L84-L153
[exact-texture]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/host/painted/src/ink/images.rs#L83-L133
[exact-query]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/docs/contract-for-humans.md#L260-L271
[exact-button]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/examples/calendar/src/themes.contract#L17-L43
[exact-input]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/host/apple/Sources/ExactKit/NativeModule.swift#L1080-L1096
[exact-capture]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/host/apple/Sources/ExactKit/NativeModule.swift#L1296-L1318
[exact-reply]: https://github.com/expo/exact/blob/1dcad174583dfee6d2690af9a5d67a3538ef7c31/host/apple/Sources/ExactKit/NativeModule.swift#L1264-L1268
