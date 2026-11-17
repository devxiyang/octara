# 从底层向上的 API 与机制审查

本轮逐层瘦身清单与实施门槛见文末。它区分已核对的保留项、可删除的重复事实和仍待验证的接口，
不要求每一层都发生代码改动。

2026-11-17；基线 `d5619e8` 加当前未提交工作区。本文补充
[组件树评审](component-tree-review.md)，只审查和提出方案，不修改生产代码。
按钮草案与其文档不代表已接受的接口。此次覆盖分层、代表性入口及调用方，
不是完整的运行时正确性、安全性或性能认证。
后续[前提审查](../research/ui-composition-mechanisms.md#10-再向下一层组件边界是否由命名状态倒推出来)
进一步区分状态新分配与命名恢复；下文保留 state 的建议指值/依赖能力，不预先锁定其命名构造签名。
组件独立 owner 不再作为解决同名 state 碰撞的唯一候选。
最新的[kit 职责拆分建议](ui-composition-redesign.md#9-是否取消独立-ui-kit-层)进一步讨论取消独立 kit 层；
本审查对现有源码职责的描述不代表必须保留该包或全部控件 API。

## 结论与优先级

最需要优先解决的是**组件的来源标记、状态作用域和内容组合没有形成单一清晰的使用模型**。
底层不是没有容器模型；`el/row/col` 已经共用一种容器。
不应通过删除必要的编辑、身份或释放机制来减少 API 数量。

| 优先级 | 发现 | 证据 | 处置 |
| --- | --- | --- | --- |
| 高 | `#[component]` 与 `component(...)` 名称相近，实际职责不同 | `octara-macros/src/lib.rs:8`；`octara/src/view/mod.rs:276` 起 | 优先统一面向作者的组件定义与作用域契约，再定命名；不能直接删除状态边界 |
| 高 | 传入内容值与挂载时执行工厂的上下文不能视为相同 | `view/mod.rs::component_at`、`mount`、`El::overlay` | 明确创建/挂载两个时机的 provider、状态和释放归属 |
| 高 | 复合组件返回外层 El，链式配置不等于配置内部 control | `octara-kit/src/forms.rs::field`、`structure.rs` | 保留普通树组合，参数明确配置对象；不做万能属性转发 |
| 中 | 选项输入被复制为另一个可写集合，组件维护同步算法 | `combobox.rs:71–121`、`popup_list.rs:70` 起 | 迁移到已支持的 keyed 快照之前验证行为；删除冗余同步，而非新增选项原语 |
| 中 | prelude 平铺了常用构建、增量数据、平台互操作等不同层次 | `octara/src/lib.rs::prelude` | 先按使用者分组，专用能力改为明确模块入口的候选；不把公共类型全算成核心原语 |
| 中 | 草案按钮的内容限制只靠文档，AX 隐藏并不使内容不可交互 | `octara-kit/src/buttons.rs::button_with_content` | 不采纳为最终接口；明确并验证单动作内容契约 |

这些是可定位的架构/API 风险；除既有验证记录外，本轮没有通过新运行测试宣称触发运行时故障。

## 0. 基础包：类型协议与默认设计语言

`octara-primitives/src/lib.rs` 不依赖 App、Signals、kit 或 shell；renderer 只依赖这些共享协议。
当前分层方向合理。几何、绘制、输入、节点句柄属于底层协议，不是每个应用都要学习的 UI 构造原语。

`theme.rs` 同时承载颜色/阴影值、语义 token、尺寸刻度和 Tailwind 风格默认值。
这里包含设计选择，但没有因此形成反向包依赖。建议保留可解析的主题数据与通用默认值；
具体 Button 的尺寸、危险外观、组合样式留在 kit。不要仅为“底层纯净”再拆出整套主题系统。
如果未来抽离预设，应是数据构造的拆分，不能让底层解析器依赖 kit。

`row()` 是 `el().flex_row()`，`col()` 是 `el().flex_col()`，`spacer()` 也是容器配置。
它们不是三个独立运行时原语，删除这些便利入口不会简化内核。

## 1. 身份、Signals 与执行权限

`reactive/graph/execution.rs` 已统一阶段、操作资格、owner 恢复；它们是内部协议。
`with_execution` 恢复上下文与 `admits` 检查是否可执行分开，有明确用途：释放需要在退休后收尾。
不应为了减少 helper 把两者合成一个隐含允许执行的操作。

`Signal<T>` 的 get/with/set/update 支持普通 Rust 数据；memo 返回只读句柄。
`state(key, init)` 按当前 scope 注册命名状态，关联热重载。
本轮没有证据支持重做 Signals 或删除 untrack；后者是显式采样边界，且 TextBuffer 等已有实际调用。
普通 UI 教程不需要先教内部阶段枚举、图节点或调度入口。

节点句柄、树代次、集合 key 和命名状态 key 分别解决不同身份问题。
保留内部区别；公开 API 不要求开发者手工拼接运行时身份。

### 增量能力作为按场景选用的工具

`view/list.rs::IntoRows` 同时接受普通 `Vec<(K,T)>` 和 `ReadList`，并共享行身份机制。
因此普通列表不需要先创建 StateList。
`StateList/ReadList` 的 filter/map/join/group_by 是增量集合能力，存在专门的纯函数规则；
应放在进阶使用路径，不让普通列表自动继承整套查询模型的认知成本。

`TextBuffer` 保留有界编辑日志，`text(buffer)` 可以消费增量；它不等价于只有一个值的 Signal<String>。
保留流式长文本用途，普通标签使用字符串或动态属性。是否删除某个增量接口应有调用方和性能证据，
不能仅因两者都能显示文本就合并。

## 2. 内核、编辑和输出

`kernel/editor.rs` 将提案与历史放在节点上，不在其中调用应用回调。
`kernel/semantics.rs` 统一解析名称，不把输入值当名称，也不递归猜测任意后代文字。
这些边界应继续保留；ui-kit 不增加自己的编辑器和名称解析副本。

View 层 Content 与 kernel Content 是构建描述和挂载后状态的不同表示。
它们存在转换并不等于重复事实所有权，不能直接合并为一个跨层公开对象。
`Kernel` 的公开 setter 用于底层接口；`Window::kernel()` 返回只读 Ref，不能据此认定应用可以
直接可变借用正在运行的 kernel。后续按宿主/测试/应用分类公开面，不盲目私有化全部 kernel。

本轮检查了编辑、语义与呈现入口；没有重新审计全部 shaping、GPU、原生适配或并发唤醒路径。
这些保持既有验证结论，不以此次接口评审替代专项验证。

## 3. 构建层：优先解决两种 component 的含义

宏源码明确说明 `#[component]` 记录组件及源码来源，不创建状态 scope。
`component(...)`/`component_keyed(...)` 返回延迟 UI 描述，在挂载时创建 scope 并运行正文。
所以“标了 component 的普通函数”不自动等于“有独立状态和 provider 范围的组件”。

这不是已经证明的运行时错误，却是采用 React/SwiftUI 组件心智模型时最容易误解的接口。
同一函数多实例、内部命名 state、provider 隔离和 cleanup 归属都依赖这一区别。

建议先评审两条路线：

- 普通函数只组合内容；有状态边界显式表达。来源标记应采用不会冒充状态边界的命名。
- 一个组件定义入口统一表达来源与独立 scope，内部自动处理挂载。必须证明不会强迫无状态
  wrapper 新增布局节点，也不会把所有内容变成不必要的延迟工厂。

本轮不预选宏自动扩展方案。先写出无状态包装、有状态双实例、延迟弹层三个调用样例，
比较解释成本及实现成本，再删除另一条对作者造成歧义的旧路径。

### 属性与 modifier 不能混成隐式转发

现有 `Styled::style` 替换同类属性，row/col 配置同一节点；这不同于每次修饰都生成包装视图。
保留直接属性的简单语义。若以后增加包装型操作，应明确它增加结构，不能仅因 SwiftUI
使用链式 modifier 就让所有 Octara 链式方法改变结构。

`View` 已是一次性描述，支持空内容和 fragment；El 是可配置元素。
目前没有必要机械地将两者合成一个“万能类型”。目标是让内容参数一致地接收已有描述，
并让原子节点拒绝 children 的规则清晰，而不是制造大量类型状态泛型。

## 4. 行为、ui-kit 与应用

`Behavior` 在构建时合并回调，沿用 mounted handlers；层按注册顺序执行，最后执行应用回调。
它不是第二套事件总线。保留组件可复用行为的能力，将其定位为组件作者接口，
应用通常只需 on_press/on_input 等事件属性。

field 已明确要求实际 control，也把无障碍与验证属性放到 control 上。
问题不是实现错误转发，而是返回 El 后的 API 仍允许用户产生错误预期。
优先清楚的内容/控件参数和容器配置；不把所有复合组件强行做成单交互根。

Combobox 中 options、rows 和 matches 的同步可以成为删除对象；open、query、highlight、selected
则是不同事实。用派生快照替代镜像时，必须保留移除/重排后的行身份、IME 和焦点政策。

Chat 和 Agent 使用可访问名称、动作目标、诊断位置。新内容模型必须同时保留三者，
不能通过全部隐藏子内容或忽略诊断使回归测试通过。

## 公开能力处置清单

| 能力 | 建议 |
| --- | --- |
| el/row/col、text、text_input、图像等内容 | 保留，明确容器与原子内容区别 |
| child/children、View/fragment、IntoElement | 统一各组件的接收方式；不新增按组件命名的内容 trait |
| state、memo、Signal | 保留；先解决状态 scope 在组件定义中的表达 |
| watch、untrack、cleanup、context | 保留明确用途，按进阶机制讲解，不强迫普通组件全部使用 |
| StateList、ReadList、TextBuffer | 保留为场景能力；普通列表/文本默认使用普通数据路径 |
| component 宏和两个 component 函数 | 优先重构语义与命名；不能直接以删函数代替作用域设计 |
| show、keyed、each、virtual_each | 共享既有挂载机制；条件、重建、集合、虚拟化语义不同，不盲目合并 |
| Behavior、overlay、focus 等 | 保留组件作者所需通用机制；应用由 kit 封装政策 |
| button_with_content 草案 | 不定型；纳入统一内容构造方案后删除被替代入口 |
| primary_button 等薄外观函数 | 可保留便利性，只要共用构造与行为机制 |
| option 镜像同步 | 在有验证的迁移中删除 |
| 原生互操作、绘图协议、测试驱动 API | 明确专用模块入口；不作为最小 UI 入门原语集合 |

## 参考依据与适用边界

- SwiftUI：[View](https://developer.apple.com/documentation/swiftui/view/)、
  [Button](https://developer.apple.com/documentation/swiftui/button) 的组合与自定义 label；
  [Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/) 的身份、生命周期、依赖。
  借鉴一致的作者模型和内容可替换的语义控件，不移植 SwiftUI 的更新引擎或全部修饰符类型体系。
- Rune 固定 `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`：
  `rune-kit/src/actions.rs::Button::label_with/build` 的内容与名称分离；
  不必同时采用 Kit/Acts 全套抽象。
- Exact 固定 `08c889a9de21a30b7c3b308422fc588cf1a480d3`：
  `docs/contract-for-humans.md` 的 slot 明确保留调用者词法上下文与 provider；
  这是 Octara 内容上下文契约需要明确的参考，不代表当前实现等同。
- 本地 Rust 为 1.100.0、Edition 2024；[官方发布列表](https://blog.rust-lang.org/releases/)
  本次核对最新稳定版为 2026-11-12 发布的 1.100.0。本轮没有编译器升级或语言特性改写。

## Rune / Exact 源码交叉审查

本节继续使用上述两个固定本地提交，不声称已同步远端最新版本。
检查了实现与测试源码，未运行参考框架测试。下面的“采用”是 Octara 的设计建议。

| 维度 | Rune | Exact | Octara 的取舍 |
| --- | --- | --- | --- |
| 组件表达 | `AppSpec<S>::view` 是 `fn(&S, &mut Cx<S>) -> El`；kit builder 最终返回 El | 组件经 compiler inline 成展开后的 view，声明重命名避免捕获 | 采用普通树组合；不为每个无状态组件增加内核节点 |
| 状态归属 | App 持有普通 S；事件/更新入口取得可变状态，Cx 表达视图环境 | 展开结果记录 state owner；初始化一次，随实例/region arm 存亡 | 保留 Signals，但让作者明确哪些调用创建独立状态 scope |
| 内容 | `El::child/children` 直接接收 El；Button 可用 `label_with` 替换呈现 | 单 slot；caller 的词法/provider 上下文；每次 children 展开有自己的实例 | 统一内容组合，同时分别规定读取上下文与挂载生命周期 |
| 身份 | `El::key` 表达兄弟身份；重复 key 会改名并警告 | 实例、分支及状态 owner 是展开与执行模型的一部分 | 保留本框架明确的重复 key 拒绝政策，不照搬自动改名 |
| 动作 | `Tx` 汇集当前提交所请求的焦点、滚动、剪贴板等操作 | action prop 传递行为，编译器检查调用关系与参数 | 借鉴明确的动作权限和提交时机，不引入第二套公开事务对象 |
| 来源信息 | key 保存调试调用位置 | `Instance` 的来源链属于 development map，不进入 plan | 来源标记与状态归属分开存储；作者层面不应出现两个含义模糊的 component |

### Rune：简洁来自普通数据与函数，不是隐藏了一套 Signals scope

依据：`crates/rune/src/tooling.rs::AppSpec`、`app.rs::App`、
`runtime/cx.rs::Cx`、`el.rs::El::child/children/key`。
这些入口显示的是应用状态 S、视图函数和元素树。不能用“Rune 不需要相同的 scope API”
推导出 Octara 的 Signals 也不需要生命周期；其状态所有权和更新模型不同。

`Cx::retype/absorb` 显式处理子视图上下文设置与读集合的传回；并非一个可直接照搬的
组件 provider 机制。`Tx` 也包含许多宿主请求，不能仅因集中在一个对象中就认定整体更小。

Rune 的 Button 在 `rune-kit/src/actions.rs::build` 中组装 row，设置 button role 和显式名称。
`label_with` 接收普通函数，原名称保留为可访问名称。值得采用的是同一控件的内容可替换，
而不是按内容形态产生不同控件。其隐藏视觉子内容的做法也不能自动证明任意交互嵌套安全；
Octara 仍需定义并验证自己的内容约束。

### Exact：组件可被展开，但身份与生命周期不能被省略

依据：`contract/syntax/src/inline.rs` 的 `Expanded`、`Owner`、`Instance`。
组件声明与调用会展开，子声明按实例重命名；状态仍记录 Root、Instance 或 region Arm owner。
因此“组件只是组合”并不意味着组件状态不需要身份，而是源代码结构和执行表示可以不同。

该文件明确区分两个事实：

1. slot 内容在调用处词法/provider 上下文中展开；包装组件的 provider 不接管传入内容。
2. 每个 children 出现位置分别实例化内容，内容中的状态归周围的实际 region arm 所有。

`contract/cli/tests/it/compose.rs` 覆盖 slot 内容位置、空内容、最近 provider、转发 slot 的类型检查；
`action_related.rs::slot_fills_keep_the_callers_provider_context` 检查包装组件不能用自身 provider
挽救调用者不匹配的 action。这些测试把契约写成了可检查的边界，值得借鉴。

但 Exact 用编译期替换和类型检查实现，Octara 是 Rust 值与闭包。若选择同样的 lexical provider
语义，就必须明确捕获哪些上下文，并让内容的实际挂载 owner 继续负责取消和释放。
不能捕获整个旧执行上下文后无条件恢复，也不能通过克隆 View 复用有状态内容。
重复呈现内容在 Rust 中通常需要显式工厂，而不是重复消费一次性 View。

### 对前一轮结论的校正

不应因为 `#[component]` 不创建 scope，就立即让所有普通组件函数隐式创建 scope。
Rune 表明普通函数组合本身是有效模型；Exact 表明有状态实例必须有明确归属。
我们的实际问题是两个相近入口让作者难以预测，而不是底层缺少组件 VM。

下一轮 API 推演保留两个候选：普通函数组合 + 清楚命名的状态边界，或统一组件定义入口。
用同一组双实例、provider、延迟内容、重排/移除测试比较，选择规则更少的一种。
不把 Rune 的事件事务、Exact 的 compiler plan 和 Octara 的 Signals 混成三套并行机制。

## 下一阶段的门槛

先确定组件定义/作用域与内容上下文，再定 Button/Field 的 API，随后删除镜像及旧入口。
不能先把 Button 改完，再逐个给 scope、provider 和特殊内容打补丁。
最低验证包含：同组件双实例状态独立、key 重排身份保持、调用者内容上下文、延迟内容释放、
纯视觉更新不重建输入、组件行为与应用回调顺序，以及 no-kit 组合能力仍成立。

本轮只运行文档差异与包分层检查；未新增执行测试，也未做真实 OS 验收。


## 逐层瘦身清单与实施门槛

2026-11-17，继续基于当前工作区审查。此次用 Cargo metadata 核对正常包依赖，
并检查 primitives 的身份/输入/文本/截图协议、kernel 导出、view 内容和列表入口。
结合前文已经检查的执行协议、Chat 和 kit 调用方作出以下判断；并非全仓逐行正确性审计。
本轮只更新审查文档，没有迁移生产代码。此前 4 个隔离检查是函数组合证据，不是此次全量回归。

### 1. 从最底层开始：保留有独立职责的边界

正常包依赖为：primitives 不依赖其他 Octara 包；text → primitives；render → primitives；
core → primitives/text/macros；kit → core/primitives/icons。render 的 core 依赖是 dev-dependency，
不能据此判定正常渲染路径反向依赖运行时。此结果只覆盖这些包，不代替所有宿主依赖审计。

| 层次/对象 | 处理 | 源码依据与理由 | 迁移约束 |
| --- | --- | --- | --- |
| primitives 的几何、绘制、文本编辑消息和宿主描述 | 保留 | lib.rs 与 Cargo.toml：不持有 App、scope 或组件实例 | 不为了减少 crate 数把运行时塞回基础包 |
| NodeId / TreeIdentity | 保留区别 | primitives/identity.rs 是槽位代次；core/identity.rs 表示 App/窗口/整树代次 | 同名控件重建和整树重载都必须使旧 target 失效 |
| Picture / Capture | 保留区别 | primitives/capture.rs 是图片数据；core/capture.rs 包含窗口与原生覆盖报告 | 渲染结果不能冒充已完成 OS 呈现 |
| 基础主题值与默认外观 | 保留通用值；迁移具体控件政策 | theme.rs 包含颜色、token 和刻度；kit buttons/status 又组合具体尺寸外观 | 一份主题解析；不能另建并行主题系统 |
| Signals 图、执行权限、释放协议 | 保留已验证机制 | reactive/graph 与 execution/scope：依赖、运行资格、owner、退休收尾职责不同 | 暂无证据需要重写调度或合并这些维度 |
| 命名状态与普通函数 | 审查构造入口，暂不改恢复保证 | named_state 的名称用于复用；传入 Signal 已能实现普通函数组合 | 不把匿名状态改造作为 UI 瘦身前置条件 |
| retained tree 与编辑状态 | 保留唯一事实所有者 | kernel/editor、semantics、focus；上层只组合和请求动作 | 不向普通控件状态结构复制 composition、selection、undo |
| View / El / 内容转换 | 保留职责，审查重复转换入口 | View 支持 fragment/空/延迟内容；El 配置实际节点；IntoElement 统一普通内容 | 不用万能 View setter 猜测后代；内部 Children 的合并需先核对所有调用 |
| component / component_keyed / 来源宏 | 解除普通函数对实例边界的依赖，随后删除无用入口 | 函数来源与创建 scope 是不同能力；show/each 已拥有真实动态边界 | 保留初始化/退休/恢复用途和 Agent 来源；不先换成新的强制 scoped API |
| 控件选项镜像 | 首批删除对象 | combobox/popup_list 的 rows + watch；view/list 已接受 keyed Vec | 保留活动项、选择、禁用、重复 key 和退出后的回调资格 |
| kit 外观与复合控件 | 分解迁移，最终决定删包 | 当前混合普通样式函数、交互规则、Markdown | 不整包搬到 core；不让每个应用复制复杂交互状态机 |
| Chat 的隐式业务 context | 改为明确函数参数的候选 | ui.rs 中 chat()/listing() 隐含依赖 | Chat::new 注册的请求清理仍归原有资源根，不能顺便改变请求寿命 |

最底层此次没有确认必须删除的协议。该结论不表示它们永远不可简化，表示目前没有足够证据
支持为满足“从底往上”而修改这些边界。第一批实际删减可从上表已有明确重复事实的项目开始。

### 2. 选择性对照 Rune

固定本地版本 3686e94ae09545fd5f2a61e22aabfbeba1cd896f；没有同步远端或声称最新。
本轮再次阅读 el.rs 的 child/children、tooling.rs 的 AppSpec 和 memo.rs 的失效说明。

| Rune 的机制 | Octara 的取舍 | 不能直接移植的部分 |
| --- | --- | --- |
| 普通状态 S、普通 view 函数，El 通过 child/children 组合 | 普通 Rust 函数、结构体和内容组合；参数显式传入业务依赖 | Cx/Tx 整套状态更新模型；保留 Signals 精细更新 |
| El 是直接的组合结果 | 样式/交互配置明确对应实际节点，减少组件转发层 | 不为统一返回类型抹掉 Octara 延迟 View 的时机与所有权 |
| 按需 memo；文档明确旧 handler 捕获和环境失效 | 优化边界必须有场景依据；保留简单 keyed 快照，增量列表按需选用 | 不在 Signals 旁增加手写 subtree version 机制 |
| kit 的 Button 最终构造 El，label_with 允许替换内容 | 借鉴内容可组合和明确动作根 | Rune 自己保留 kit/builder，并不能据此证明我们必须保留或删除 kit |
| tooling 使用运行时/渲染器和虚拟时钟 | 同一实现提供 Agent 检查和可重复框架输入 | headless 不等同于 OS 输入、IME 或真实窗口呈现 |

来源：[El](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el.rs)、
[AppSpec](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/tooling.rs)、
[memo](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/memo.rs)。
Button 的源码依据见前文。借鉴的是机制，不以目录形状或 API 名称相似作为完成标准。

### 3. 按证据推进的工作顺序

1. **底层保留边界确认（本轮审查完成）**：以上正常依赖和事实所有者记录为基线。
   不新增 VM、状态代理、组件注册表或每控件必备的 XxxState/Controller 家族。
2. **第一实施切片：去掉重复选项事实**。让 Combobox/PopupList 消费已有 keyed 快照；
   删除镜像 Row/order 同步和仅服务镜像的 watch。键盘选择、有效活动项纠正、滚动揭示等行为仍须保留，
   不能把混合在同一 watch 中的必要逻辑一起删除。这个切片不要求先新增 API 或新的包。
3. **组合入口收敛**：用 Chat 显式参数、实际输入节点与富内容动作验证 El/View 配置对象和工厂时机；
   只删除失去用途的组件边界入口。普通函数提取不得改变 owner，源码来源必须仍可定位。
4. **kit 职责迁移**：外观移到应用/参考配方，共有交互沿一份实现；确定 Markdown 的独立去向。
   最后删除无调用者的 kit 包、vocab 入口与依赖，不能先删包或全量复制其实现进 core。

第一切片的验证必须覆盖选项插入/移除/重排、重复 label 与重复 key、选中值不再可用、禁用项、
活动项更新、打开/关闭/重开、外部 setter 拒绝或归一化，以及 editor target 和中文组合输入的保持。
先使用现有 headless 回归与适量补充验证；真实窗口仍独立验收，不从 headless 推断。
每个实施切片都应完成调用方迁移、删除旧路径、适当验证和提交，才进入下一切片。

此处的实施顺序是审查产物。当前“先调研、审查和分析”的约定仍有效，生产迁移尚未开始。
