# Exact 架构调研

编译器、平台、构建更新和 Agent 的全链路见 [Exact 完整架构研究](exact-full-architecture.md)。本文集中讨论运行时、窗口结算和呈现时序。

调研固定在 Exact `main` 的 `2c2673b`，2026 年 11 月 22 日。本文沿作者接口、Runner、布局内核和宿主追踪更新，对照上游回归。用本地 `50b6af94b` 加上官方单提交差异核对源码，最新变更覆盖 Windows 选区对比度。[Exact 版本](https://github.com/expo/exact/commit/2c2673b97fabce377a213a4db273894c8ce86348) · [上游差异](https://github.com/expo/exact/compare/50b6af94b519d50a9f7105f07606b4d96277c5c9...2c2673b97fabce377a213a4db273894c8ce86348)

**最值得借鉴的是给业务结算、树变更、宿主应用和实际可见各自定义完成条件。** Exact 的编译器能约束业务代码，原生宿主能看见具体操作触及的对象。两处都具备明确的权限或读写范围，所以能判断哪里要保序、哪里可续做。Octara 可以用 Rust + Signals 和保留树建立同样明确的边界，优先拆开当前混在窗口准备里的用户回调与纯窗口投影。

## 一次变更经过哪些层

`.contract` 把视图、状态、动作以及外部数据的类型约定编译成 `Plan`。原生 Runner 执行 Plan，持有实例状态、key 和可推进的时钟。Kernel 持有节点和布局，基于 Taffy，并接入宿主的文字测量。Apple 由 AppKit／UIKit 应用批次，Android 结合 HWUI 和原生控件，Linux／Windows 接共享绘制器。Web 从 Plan 生成 JS 模块，浏览器承担 DOM、布局和绘制。TypeScript 或 Rust 数据源走有类型的接口，外部能力按授权接入。[Exact 的分层与平台实现](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/README.md#L395-L419)

首帧入口也由这个边界决定。常量进 Plan，首轮 UI 可以先生成；Apple 宿主收到首轮绘制回调后再启动相应的数据源和平台能力。对 Octara 更直接的启发是把首帧必需的数据和之后才需要的工作列清楚，尤其是自定义 GPU 内容、外部请求和原生资源的就绪条件。[首帧预算与设计](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/README.md#L80-L95) · [Apple 的启动交接](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/SessionActivation.swift#L20-L83)

```mermaid
flowchart LR
    I["有时间与目标的输入"] --> R["Runner 执行动作并结算"]
    R --> K["Kernel 校验并提交树变更"]
    K -->|"已提交的增量回执"| H["Apple 宿主生成变更批次"]
    H --> A["主线程准备并应用"]
    A --> N["原生视图与系统呈现"]
    A -->|"应用回执或测量反馈"| R
```

1. **业务动作有明确的读写规则。** Action 读取开始时的状态，写入统一生效。若 `count` 从 0 开始，Action 先做 `count = count + 1`，再把读取的 `count` 写入另一个状态，两个值为 `1/0`。要共享新值，先算局部变量。Action 里没有循环和 `await`，`derive` 的依赖必须无环。Runner 为动作建立检查点，特定可恢复错误可恢复状态槽和待发命令，`then` 有自己的提交边界。这种能力来自执行器可控的写入范围。[Action 约定](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-humans.md#L278-L323) · [检查点与失败处理](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/runner/src/runner/commit.rs#L74-L163) · [动作与后续提交](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/runner/src/runner/commit.rs#L296-L325)
2. **树有自己的提交回执。** Kernel 先在暂存视图上验证整批结构操作，再写入实际树。回执给出当前 epoch、增删改的节点及布局失效等事实。Apple 宿主用它建立或更新平台视图镜像，在结构到齐后更新子节点顺序。某些语义更新未触及节点，可避开完整布局和绘序遍历。原生文字与控件尺寸有暂估和有限重试，也有记录未完成布局的状态。[Kernel 批处理与回执](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/kernel/src/txn.rs#L1-L87) · [Apple 结构投影](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/src/commit.rs#L51-L134) · [布局过期与空转路径](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/src/commit.rs#L189-L224) · [原生测量](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/src/layout.rs#L205-L274)
3. **系统应用也有自己的顺序。** Apple 主线程在 Core Animation 事务中应用平台批次，批次引起的部分原生反馈在随后有序处理。在大批新增节点的场景，Runner 已有新状态时，系统仍可保留完整的上一棵可见树。这里的系统事务是原生视图生效边界。实际显示时刻需额外证据。[Apple 应用事务](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/Session.swift#L1153-L1189) · [分片和发布](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L11-L33)

## 作者接口、组件身份和外部数据

Contract 向作者提供 CSS 风格的节点、布局、样式和语义。普通组件由 props、受限状态、动作和子树组成。编译器知道哪些字段可以写、哪些工作可能等待、渲染节点来自哪一行。Runner 比较表达式实际读取的状态槽、派生值、数据就绪或错误状态与上次的已结算值，未变化时复用。遇到不透明的依赖会重新求值。这是限制语言后获得的可分析性，适合把同一 Plan 交给多个执行后端。[组件约定](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-humans.md#L278-L323) · [Runner 的依赖比较](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/runner/src/runner/settlement.rs#L133-L198)

**异步约定属于执行协议。** 搜索字段更新 `query`，以该值作参数的 `resource` 可重新请求；保存通过 `mutation` 交给数据源。资源可保留上一份值，错误和等待仍能分别检查。请求带身份，旧答案无法覆盖较新的请求。需要逐一持久化的操作可选 `queue`，相互独立的写入可选 `parallel`。这些规则约束结果归属，外部源已经开始的写入有自己的提交时点。Fieldnotes 另给打开的笔记加会话版本，避免把另一份文档的回复接到当前草稿。[搜索和保存实例](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-humans.md#L580-L621) · [请求顺序与失败](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-humans.md#L647-L704) · [Fieldnotes 的文档会话](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/apps/fieldnotes/app.contract#L101-L126)

**身份和布局是不同的数据。** 作者给动态列表行提供业务 key。虚拟列表限定可枚举的直接流式子树及明确的视口尺寸，以便按范围准备并稳定滚动。Kernel 回执以带代次的节点 key 引用对象，同一个节点地址被重新绑定到另一挂载时也能显式标记 renew。布局树可导出有版本的节点行，Agent 与宿主可据此查询几何。节点查找名、挂载身份、语义和布局测量各自有用途。[虚拟列表结构](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-humans.md#L443-L479) · [变更与重新绑定回执](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/kernel/src/txn.rs#L55-L92) · [内核导出](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/kernel/src/kernel.rs#L903-L927)

**原生控件参与布局与观察。** Apple 文本控件持有编辑会话和系统 IME。Exact 的 `input` 会报告变化的组词文本，并带 `isComposing`；`change` 对应结束编辑。应用在组词中设置的新值暂存，待组合结束再处理，避免每次输入触发的格式化中断拼音。需要更深入的系统部件时，应用可用有布局身份的 native module 或访问实际平台对象。对 Octara 可借鉴的是明确同一会话中哪些值归系统、哪些值归应用，并让输入法和候选窗查询同一份选区与锚点。何时向应用报告 marked text 则属于各自编辑契约。[组词与应用值](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/FieldComposition.swift#L1-L28) · [原生扩展接口](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/README.md#L97-L108)

## Apple 宿主怎样避免一轮工作压住输入

**准备可以分片，生效仍有一次明确交接。** 大批次只提前构造符合条件的普通 box 和 text 子树。准备中的节点不参与命中、读取或可见性，原生控件在最后应用。Live 路径按显示机会续做，并按实际准备和绘制成本调整。批次持有 `viewsEpoch`，会话的视图代次变化后取消旧准备。按下高亮等原生反馈可用上一棵已应用的树继续响应，批次已移除的目标会被拒绝。[准备范围](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L11-L33) · [续做与代次](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L307-L352) · [输入入口](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L557-L573)

**先证明操作可以交换，再让它先应用。** 例如，正在准备大列表时，后到批次可以更新已存在的节点，前提是它避开了待生效批次变动的结构、属性和原生控件读写。结构操作和可能依赖前置对象的命令仍受顺序约束。同一属性的动画采样可以被更新值覆盖，整份新样式若带来了其它尚未生效的字段，要先扣住这些字段。这些判断依据已生成的平台操作及其影响范围。[批次冲突与样式保护](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L219-L265) · [操作影响范围](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/StagedApply.swift#L595-L640)

**线程所有权和会不会等是两件事。** Apple 路径用一个进程级 `exact.owner` 持有所有 Rust Runtime。部分工作可以异步回到各会话的发布队列，同步调用仍需要主线程等结果。等待循环会服务约定的回调通道，经该通道回到主线程后再次进入 Runtime 会被拒绝或触发契约错误。源码也记录了数据源挪到首帧之后执行时，后续同步 Runtime 请求仍可能排在它后面。[Owner 的同步等待和重入](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactCore/Owner.swift#L100-L151) · [首帧后激活](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/SessionActivation.swift#L90-L133)

**窗口局部独立来自会话边界。** macOS 每个文档窗持一个会话。外部回复跨回会话时核对生命周期和代次。跨窗共享存储写入与 topic 通知显式传播，特定 Widget 刷新还会等全部目标会话完成。这个例子支持把共享顺序写成可见协议。Octara 窗口之间能直接共享 Signals，业务依赖的边界需要由 Octara 自己定义。[窗口会话](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactMac/main.swift#L248-L286) · [会话代次校验](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactCore/RuntimeCompletion.swift#L16-L64) · [跨会话传播](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/SessionStore.swift#L73-L118)

在 Ink 的绘制路径还能看到同一个原则。后到帧只取代前面待处理帧的呈现资格，有顺序的资源作业和退休回执继续处理。GPU 获取状态限制每个获取对象的请求在途数。这个边界可与 Octara 已有的逐窗呈现和设备提交协议对照。[Ink 提交与呈现](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/ink/src/present.rs#L59-L94) · [GPU 目标获取](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/gpu/src/acquire.rs#L187-L238) · [Octara 渲染架构](../design/v2/04-native-pipeline-render-host.md)

Wayland 光标是个更小的分层例子。编辑器提供当前光标几何，宿主根据桌面设置和最后一次活动决定光标透明度。渐变时请求下一帧，明暗停留阶段只登记下次唤醒，超时后停止由光标引发的唤醒。合适的通用抽象是宿主可以提交一个时间点或下一次显示需求，光标习惯留在平台。最新提交的 Windows 选区配色也将系统对比度供给自绘文字和 RichEdit，用同一组平台值维持可读性。[光标调度与测试](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/linux/src/window/caret.rs#L25-L118) · [事件循环](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/linux/src/window.rs#L171-L209) · [Windows 选区提交](https://github.com/expo/exact/commit/2c2673b97fabce377a213a4db273894c8ce86348)

## 动画、常规宿主与 GPU 画布

**样式声明目标，呈现采样另有时钟。** 普通 CSS 声明经同一表示交给不同执行器。浏览器运行 CSS 动画，原生宿主使用 Rust 时间函数。可按绝对时刻求值。Apple 帧回调采样显示目标时间，普通定时任务仍由时钟入口推进。当前帧需要的 motion tick 和填充工作有自己的在途判断，已呈现的动画时刻亦给迟到的业务回执设下界。这让更新目标、局部动画与异步任务的时间关系可解释。[Motion 表示与时钟](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/motion/src/lib.rs#L1-L38) · [帧回调](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/Frames.swift#L50-L111) · [提交与 motion 时序](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/src/commit.rs#L150-L191)

**画布是按需加载的图形能力。** Apple 的普通界面由平台视图与 Core Animation 呈现，wgpu 画布有独立的尺寸、时刻、帧周期和输入生命周期。Apple 的画布可以在 drawable 归还时重试，等待实际目标期间会保留所需工作并继续处理宿主的其它机会。它提供有宿主约束的自定义绘制面。Octara 希望作者能在任意组件的背景、边缘和转场中插入 wgpu 工作，Rune 的绘序插槽更直接对应这项需求。[画布上下文](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/gpu/src/lib.rs#L78-L111) · [目标获取与唤醒](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/gpu/src/acquire.rs#L185-L238) · [宿主记录画布等待](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/Frames.swift#L112-L146) · [Rune 的绘制扩展](rune-architecture.md#gpu-作者扩展)

## 值得保留的失败样本与验证方法

| Exact 的问题或验证边界 | 对 Octara 有用的检验 |
| --- | --- |
| 输入曾越过已到期、正在等宿主应用回执的 change task。现在有时间的派发会记住输入及其抵达时刻，先推进应当先发生的工作，宿主需要应用时留住输入再续做。最新语义用例让两个 change action、一次点击或文字输入、由输入触发的第三个 action 相继发生。[问题记录](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/issues/closed/20261112-review-input-overtakes-change-task-yield.md#L3-L20) · [保序实现](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/runner/src/runner/advance/inputs.rs#L82-L139) · [时序用例](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/semantics/corpus/timers/input-order.contract#L13-L40) | 让输入抵达已开工而尚未结算的反馈之后，检查目标、执行次数和可见几何的顺序。加挂载替换和关闭重开，确保迟到事件不能交给同名新实例。具体保序采用 Octara 自己的输入契约。 |
| 节点被移到已有的 `display:none` 子树下后，增量路径曾保留它的旧导出矩形，全量新建则得到空几何。[问题记录](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/issues/closed/20260930-hidden-subtree-keeps-stale-frames.md#L3-L17) | 给同一状态比对增量与全量投影，核对绘制、命中、无障碍及原生放置，特别检查隐藏、卸载、重挂载和滚动范围。 |
| 旧 Swift 桥在后台 I/O 期间持有模块的共享锁，主线程的同步 `native.call` 也要等同一把锁。[问题记录](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/issues/closed/20260927-swift-native-call-waits-on-a-later.md#L11-L36) | 为每个跨所有者接口标出调用者等待什么，主动注入慢 I/O、长回调和迟到回执，再分别看输入派发和首个正确像素。 |
| Lean 4 模型把动作、结算、调度与错误写成可运行语义，语料和差分测试对照实现。它也指出语义模型按 `testId` 查找的时刻与 Runner 有差异，定时工作替换同名目标的情况超出该观察模型。[模型和证明范围](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/semantics/README.md#L18-L47) · [目标身份的边界](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/semantics/README.md#L215-L228) | 先用小型、可重复的时序和状态测试锁定执行协议，再用真实窗口检验平台目标、IME、命中与像素。两处使用相同的场景定义和可核对的身份。 |

**Agent 的等待协议也值得学。** Exact 将按 `testId` 操作、可控时钟、诊断和构建回执放在统一驱动里。Apple 必须读取完整批次时会同步排空在途准备和发布。代码同时把 `CADisplayLink` 回调间隔明确标为代理量。Octara 应让 Agent 读到状态结算、窗口快照、平台接纳和系统截图各自的版本与来源，等待当前稳定版本时明确报告在等哪个条件。[Agent 驱动](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/README.md#L71-L95) · [稳定读取](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/PublishedBatches.swift#L91-L114) · [显示代理量](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/host/apple/Sources/ExactKit/FrameSampler.swift#L597-L605)

调试构建的源码映射绑定 Plan 的 digest，Agent 可在接受的构建代次中从布局回到声明、组件调用点和最终生效样式。帧诊断把缺失的分段保留为空，区分经过的时间与线程实际消耗的 CPU，注明代理量只覆盖哪些阶段。这是工具和运行时共享事实的做法，也明确给真实系统验证留下入口。[诊断与源码映射](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-agents.md#L6240-L6267) · [帧报告覆盖范围](https://github.com/expo/exact/blob/2c2673b97fabce377a213a4db273894c8ce86348/docs/contract-for-agents.md#L6210-L6218)

## 对 Octara 第 6 阶段的设计结论

当前 `AppState::frame` 在全 App 轮次内推动任务、Signals、窗口生命周期与每窗准备。窗口准备会执行焦点、命令和布局反馈，其中有回调能写共享状态。准备之后，窗口提交仍检查全 App 的待执行工作。这里的全局屏障保护了真实存在的反馈依赖。要缩小它，首先要把两类工作从同一路径拆开。[App 结算和提交](../../crates/octara-runtime/src/cycle.rs) · [窗口准备与结算](../../crates/octara-runtime/src/cycle.rs) · [布局反馈的执行权限](../../crates/octara-runtime/src/engine.rs)

1. **有写权限的工作由 App 按因果顺序结算。** 让回调、Signals 传播、生命周期请求和需要回到应用的布局反馈保有明确的结算边界。尚未运行且可写任意 Signal 的回调，按可能影响所有窗口处理。上一轮的动态订阅只能描述上次读取，分支变化后可能写到别处。
2. **纯窗口投影只消费已结算、带版本的数据。** 从这里逐步移除全 App 窗口轮次。只让可冻结、可续做的框架操作按窗调度。提交前核对 App 版本、窗口实例、视口与相关资源，任何需要调用应用的反馈回到有序协议。先保住当前公开入口可见的回调和 `settle` 语义。几何、命中、无障碍和原生放置应一致来自同一个已完成的窗口版本。[窗口输出与场景](../../crates/octara-host-native/src/presentation.rs)
3. **每窗继续独立接纳，观测指向同一个窗口版本。** 新版本待结算时，上一已接纳版本可以继续承载画面。Agent 请求当前稳定状态时应看到明确的等待条件。设备完成、宿主接纳和系统显示继续各自报告，便于判断性能长尾在哪里。[Octara 的迁移门槛](../design/architecture-v2.md)

首批验收可用一个三窗用例区分两个因果边界。A 和 B 通过 Signal 与布局反馈互相影响。若在途工作已限定为 A 和 B 的窗口投影，C 可以接纳自己已结算的候选。若还有能改写共享状态的用户回调，C 的上一接纳画面照常服务，读取当前稳定版本需要等共同结算。另用单窗中文输入、连续滚动与动态内容更新核对输入派发、稳定观察与实际显示。在有实测证据时，再决定哪些纯工作值得分片或移到后台。Exact 的 Apple 分片阈值来自它自己的原生视图成本。
