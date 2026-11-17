# 前端组件契约独立审查

日期：2026-11-17。状态：调研与提案修订建议，未实施。
Octara 源码基线：b117aeea644d2480ff62be0c4a004ac7c7ed0fbc。

本报告独立复核[分层架构提案](../design/layered-architecture-redesign.md)、
[前端/原生交叉审查](frontend-native-cross-review.md)与[框架调研](ui-composition-frameworks.md)，
检查 React、Base UI（base-ui.com）、Tailwind、SwiftUI 的公开契约能支持哪些决定。
本轮只读相关源码，只新增本文件；没有修改实现、运行参考框架、运行测试或测量 Agent 成功率。
总提案可能同期继续修订，下文位置按本轮阅读的章节主题定位。

“确认事实”限于本次读取的源码与官方说明；“推论”是设计判断；
“建议”尚需选择/实施；“待验证”不表示缺陷已运行复现。
在线资料查阅日期均为 2026-11-17，网页标示 React v19.3、Base UI npm 1.9.0、Tailwind v4.3，
不代表本机安装或测试过这些版本。
SwiftUI 依据 Apple 在线符号文档和 WWDC21 公开说明，未审计私有实现。
W3C Input Events Level 2 使用 2026-10-15 Working Draft，不视为所有宿主已兑现的证据。

## 1. 关键判断

| 优先级 | 发现 | 提案需要补齐的契约 |
| --- | --- | --- |
| P1 | “同一校验结果”不能压成“错误字符串决定 invalid” | 草稿解释、控件约束、业务接受、显示时机与保存资格应保留区别 |
| P1 | Root/Part 公开的是关系与挂载协议 | 先与改良 builder 比较；定义部件基数、归属、动态移除和 overlay 所有权 |
| P1 | render 合并没有普适的顺序/禁用规则 | 保留 Octara 行为先于应用通知；分别定义取消、接受值、禁用与焦点 |
| P1 | 受控值不是完整编辑状态 | 明确 IME 中外部写入、拒绝/变换提议、异步过期和提交/失焦冲突 |
| P2 | 熟悉的样式词不能替代优先级定义 | 明确阶段和属性来源；读取语义投影不等于增加独立可写状态 |
| P2 | 行为/外观值得分工，公开无样式 API 的门槛仍须具体 | 改颜色不足以公开部件协议；真实布局变化且能保留同一行为才是强证据 |

这些判断支持总提案的大方向，尚不能证明候选 API 比 builder 更简单，
也没有证据支持任何模型训练覆盖或生成成功率主张。

## 2. 校验：存在两个方向相反的覆盖风险

### 确认事实与反例

[NumberField](../../crates/octara-kit/src/range.rs)（181、190 行）从模型字符串派生数字，
把空串、解析失败或超范围判为 invalid，Enter/blur 对完整数值规范化。
[Field](../../crates/octara-kit/src/forms.rs)（47、75 行）接收已配置的 control，
随后按非空 error 是否存在再次调用 invalid。
[El::invalid](../../crates/octara/src/view/element.rs)（704 行）直接替换该属性来源。
因此，按源码组合，Field 没有保留 NumberField 先前的 invalid 来源；本轮未运行新复现。

反方向同样成立：[Chat 设置界面](../../apps/chat/src/ui.rs)（379、387 行）覆盖 NumberField 的 invalid，
而 [Generation::parse](../../apps/chat/src/settings.rs)（22 行）允许空串表示 None，即使用模型默认值。
这里覆盖有业务目的。把两者无条件 OR，会把产品允许的空值重新判为无效；
把所有重复 invalid 一律删除，也会丢掉这项语义。
非空 token 又必须是整数，业务约束比“可解析有限 f64”更严格。

Base UI 的 Field 分别公开 validationMode、Validity 和 Error.match，说明校验内容和展示策略可以分开表达，
并不证明 Octara 需要整套表单状态机。
来源：[Base UI Field](https://base-ui.com/react/components/field#api-reference)。
SwiftUI 的某个可选格式化 TextField 初始化器在解析失败时将绑定置为 nil，
说明两向可选值绑定也未必保留“空值”和“非法草稿”的区别。
来源：[Apple 可选格式化 TextField](https://developer.apple.com/documentation/swiftui/textfield/init(_:value:format:prompt:)-4g6if)。

### 推论与建议

唯一事实源不要求把所有结果压成一个布尔值。
未显示错误、未验证、可编辑但暂不能提交、允许空值、提交时规范化、业务拒绝，是不同事实。
Field 由 error 是否可见直接决定有效性会丢失约束；机械 OR 又把默认策略变成不可调整的硬约束。

建议将“同一校验结果驱动字段语义和 Save”细化为：
**共享同一份经明确策略解释的编辑/校验结果，各消费者读取自己的投影。**
至少区分以下职责，具体类型可在切片实施前定稿：

| 事实 | 来源/所有者 | 必须回答 |
| --- | --- | --- |
| 草稿、selection、composition、编辑版本 | 编辑文档 | 中间文本不能因解析失败或隐藏错误而消失 |
| 空值、格式、范围、步进、规范化 | 控件机制与显式配置 | 哪些是默认策略、哪些是硬约束；空串是否有效；步进是否等于合法值限制 |
| 业务/跨字段接受结果 | 应用模型 | 接受哪个候选值，可选值与完整数值如何校验 |
| 结果是否仍适用 | 产生结果的所有者 | 结果针对哪个草稿/接受值；pending、未验证、过期不能冒充 valid |
| 错误显示与播报 | Field 呈现策略 | 编辑、blur、提交后何时显示；隐藏错误不自动等于有效 |
| 语义 invalid 与 Save 资格 | 上述事实的派生投影 | 分别读取哪个阶段，不能从红边或错误节点存在性反推资格 |

默认约束可由产品显式选择替代策略，不能再靠“最后一个 invalid 恰好覆盖”表达。
Field 负责展示与关系时，不能不知控件策略便宣告它有效；也无需新增全局表单引擎。
Chat 的覆盖应保留为需求证据：先显式表达“空串为默认值”和“整数 token”，
再删除真正已被替代的重复校验。

**待验证：**严格拒绝超界与 blur 自动 clamp 的冲突。输入 2.1 后直接点 Save，
若 blur 先改为 2，是否符合产品意图必须先选定。
共享解析函数不能自行解决这个时序；保存应明确读取规范化前还是后的候选值及实际接受结果。

## 3. Root/Part：并不自动比 builder 简单

### 确认事实与反例

Base UI Field.Root 渲染 div；Combobox.Root 不渲染自己的 HTML 元素。
Root 没有跨组件一致的结构责任。
来源：[Field.Root](https://base-ui.com/react/components/field#root)、
[Combobox.Root](https://base-ui.com/react/components/combobox#root)。
Dialog 允许多个 Trigger，也允许通过 handle 关联 Root 子树之外的 Trigger；
受控多 Trigger 另需管理当前 triggerId。不能假定所有 part 唯一，或最近布局父节点就是所有者。
来源：[Base UI Dialog 多 Trigger](https://base-ui.com/react/components/dialog#multiple-triggers)。

现有 Field builder 消费一个已配置 control 后组合标签、说明和错误，
已经集中建立最常见的一对一关系。主要风险是返回 El 后把 wrapper 冒充 control，
以及 invalid 被覆盖，不是缺少 Root/Part 拼写。
来源：[Octara Field](../../crates/octara-kit/src/forms.rs)。

React portal 保留 React 树的 context 和事件传播关系，只改变 DOM 放置位置；
更换目标 DOM 节点会重建内容。这是 React 的具体契约。
Octara 的“overlay 不改变原所有者”不应扩展为默认拥有 React 式冒泡。
来源：[React createPortal](https://react.dev/reference/react-dom/createPortal)。

### 推论与建议

Root/Part 把“传入一个 control”换成多个部件的注册、查找与解绑。
分散布局、多个控制点和弹层可能值得承担这项成本；单个标签加输入未必值得。

| 需求 | 优先候选 | 理由 |
| --- | --- | --- |
| 一个输入、标签、帮助、错误 | 短 builder，返回值明确区分布局根与控制项 | 一次完成关系，避免手工注册 |
| 按钮换图标、两行标签、颜色 | 内容入口与外观配置 | 不需要公开根替换协议 |
| 多控制点、标签跨布局、自定义错误区 | 命名部件或明确控制项的 builder | 位置与关系确需分离 |
| 弹层、多 Trigger、脱离布局的面板 | 组件范围与显式关联入口 | children 顺序与字符串拼接不足以表达归属 |

公开部件前，每类组件需有短关系表：

- 哪个部件拥有角色、名称、焦点、disabled、编辑模型，哪些只负责布局或内容。
- 基数是 0/1、恰好 1 还是可重复；关闭/延迟挂载时哪些可以缺席。
  动态更新的中间缺失应在规定稳定边界验证，不能每挂一个部件便误报不完整组件。
- 关系所属的现有组件 scope、嵌套同类组件的隔离、条件替换与卸载时的解绑规则。
- 词法回调所有者、组件关系所有者、布局父节点、overlay 位置分别是什么。
  跨窗口移用必须有明确支持规则，不能隐式转移执行资格。
- 视觉 Label、显式名称、Description、Error、prompt 的来源和覆盖规则。
  名称变化不改变交互身份；移除部件不把关系留给同名的新节点。

名称用于标明目的，description 提供附加说明，Web 名称计算还按角色和显式命名方式区分；
不能统一拼接整棵子树。
来源：[W3C APG 名称与说明](https://www.w3.org/WAI/ARIA/apg/practices/names-and-descriptions/)。
原生投影应保留概念区别并独立验证宿主结果，不复制 DOM 属性实现。

**待验证：**普通 Field、带前后缀 NumberField、弹层选择器各比较“改良 builder”和“命名部件”。
对照正确挂载所需关系数、配置位置、错误诊断与常见调用长度，再决定公开范围。
符号数量或代码相似度不能替代这个比较。

## 4. 同一交互根不等于任意合并 props/handler

### 确认事实

Base UI render 的元素形式对 class、style、属性、处理器和 ref 使用不同规则：
render 元素的普通属性/同名 style 值优先，其处理器先执行，所有 ref 指向同一元素；
disabled 等组件输入则由各 part 分别读取。自定义组件需转发 ref 和收到的 props。
来源：[Base UI Composition](https://base-ui.com/react/handbook/composition#how-props-are-merged)。

独立 mergeProps 对 ref 只保留最右值；synthetic event 的 preventBaseUIHandler、
浏览器 preventDefault、stopPropagation 是不同操作，非 synthetic 值事件没有相同的取消机制。
render 函数形式不会自动替调用者合并 props。
来源：[Base UI mergeProps](https://base-ui.com/react/utils/merge-props)。

Octara 当前 Behavior 各层按注册顺序执行，最后运行应用回调；Key 的 true 消费后续链，
非 Key 通知没有通用取消返回值，回调之间检查所有者存活。
Input 各处理器收到同一提议文本；应用另读模型观察当时接受值。
来源：[Octara Behavior](../../crates/octara/src/view/behavior.rs)（8、23、120 行）。

### 建议

总提案 7.1、8.2 应分开三件事：

1. **动作资格与内部不变量：**目标存活、禁用、IME 路由、焦点/模态资格由框架和行为保障；
   样式与通知不能重新启用被拒绝的动作。
2. **可拒绝的意图：**setter 可以拒绝/变换输入。若未来有专门取消入口，
   必须规定它在内部行为之前还是之后、阻止哪一步；不能把现有通知解释为事后撤销。
3. **行为后的观察：**保留组件行为先于应用回调；标明载荷是提议值、当时模型值还是提交结果。
   setter 返回不表示异步业务验证已完成。

“冲突主动作”应附例子：Button 加 Tooltip 是相容关系；互相覆盖 role、
一个按键同时提交并打开另一模态，需要拒绝或明确策略。
不能因有两个 handler 就认定冲突，也不能因可合并就认定相容。

### disabled 按维度兑现

Base UI Button 可选择 disabled 时仍可聚焦，还要求显式 type=submit 才提交，并明确不应用作链接。
这直接说明熟悉名称也可能有不同默认行为。
来源：[Base UI Button](https://base-ui.com/react/components/button)。
SwiftUI 普通 ButtonStyle 在平台标准交互之上定制外观，也不意味着跨平台焦点政策一致。
来源：[Apple ButtonStyle](https://developer.apple.com/documentation/swiftui/buttonstyle)。

建议声明 disabled 是否退出 Tab、是否允许程序聚焦、AX 是否仍可发现、拒绝哪些语义动作、
按下后禁用如何取消、子部件有效 disabled 如何派生。
继承禁用不应被子部件任意后写 false 绕过；readonly、loading、inert 分别定义。
禁止用户动作不等于禁止应用更新模型。有效禁用结果可派生给样式/AX/Inspector 共读，
不需要第二份可独立修改的状态。

**待验证：**指针/KeyDown 后禁用、AX/Inspector 激活、可聚焦禁用 Trigger 的 Tooltip、
父组件禁用与局部 false。仅测点击不触发回调不足以证明完整成立。

## 5. 受控值、编辑文档与 IME 需要冲突表

### 确认事实与边界

React input 的 value/checked 控制当前值，defaultValue/defaultChecked 表示初值；
文本 onChange 随编辑触发，受控输入需要同步回写，并禁止生命周期中切换控制模式。
官方把异步回写、输入时变换值、key 导致重挂载列为 caret 问题的检查点。
来源：[React input](https://react.dev/reference/react-dom/components/input)。
这说明控制值有额外义务，不证明每次编辑都重写/规范化适合 Octara。

Base UI Combobox 分别控制 selected value、inputValue、open；
“组件受控”不能替代逐项所有权。
来源：[Base UI Combobox](https://base-ui.com/react/components/combobox#root)。
SwiftUI Binding 连接外部事实源，本身不直接保存数据。
来源：[Apple Binding](https://developer.apple.com/documentation/swiftui/binding)。

Octara Model<T>::set 返回空值并支持任意 get/set 闭包；
text_field 先调用 setter 再通知应用，类型并不保证接受参数或完成异步检查。
来源：[Model、text_field](../../crates/octara-kit/src/lib.rs)（40、233 行）。
InputEvent 分别表示 Text、Composition、带选区的 Preedit，不是只有字符串变化。
来源：[InputEvent](../../crates/octara/src/app/event.rs)。
W3C 草案也区分 composition session 与其中 input，一部分 IME 期间事件不可取消；
它提供设计理由，不能证明原生宿主时序已正确。
来源：[Input Events Level 2 §7](https://www.w3.org/TR/input-events-2/#input-event-order-during-composition)。

### 建议

总提案 §6、§8.3 不能只写“优先级明确”，实施前应选定：

| 冲突 | 最少需要承诺 |
| --- | --- |
| 同步拒绝/变换提议 | 通知保留提议语义；另读的是当时模型值；selection、undo 与接受值怎样协调 |
| 异步检查 | pending 与最终接受分开；旧结果不能覆盖更新草稿；不把网络完成视作同步编辑已接受 |
| composition 中外部值替换 | 排队、重基或取消的明确政策；取消时清理并通知宿主，不能静默覆盖预编辑 |
| 同值回显、主题/名称/错误更新 | 不重挂编辑器，不重置 selection、undo、composition |
| IME 中 Enter/Up/Down/Escape | 输入法优先，不误选选项、提交表单或关闭弹层 |
| blur、Enter、Save 同时规范化 | 指定先后、候选值、拒绝后草稿及通知次数 |
| 筛选/重排移除 active 项 | selected、active、键盘焦点、输入草稿分别处理 |

这是必须补齐的契约，不声称某个外部更新政策已实现。
优先保留编辑身份，真正外部重置作为可解释的编辑操作。
同步本地接受仍可使用 Signals/Model，不为模仿 React 强制再传重复 onChange。

**反例：**外部写入永远立即获胜可能重置候选词；无限推迟写入又使应用重置失效。
“单一事实源”不能自行决定正确冲突政策。

## 6. 样式：优先级必须包含阶段和来源

### 确认事实

Tailwind 同属性冲突按生成样式表顺序决定，不按 class 字符串顺序；
官方建议避免冲突，必要时用明确组件参数表达定制。
来源：[Tailwind 样式冲突](https://tailwindcss.com/docs/styling-with-utility-classes#managing-style-conflicts)。
@theme 变量还影响生成哪些 utility，不只是任意运行中字典。
来源：[Tailwind theme variables](https://tailwindcss.com/docs/theme#what-are-theme-variables)。

Octara style_with 的声明位置固定，后写 base 覆盖先写 base；
动态 patch 撤回属性时退回更早声明/默认。状态在 base 之后，
screen、touch、state 另有阶段顺序。
来源：[style_with](../../crates/octara/src/view/element.rs)（549 行）、
[When/State](../../crates/octara/src/kernel/style.rs)（23、46 行）。
所以调用者后写 bg 不自动覆盖组件 hover bg。

当前 State 位集有 CHECKED、EXPANDED、DISABLED 等，没有 INVALID；
Node invalid 与语义查询已存在。
来源：[State](../../crates/octara/src/kernel/style.rs)、
[Kernel::invalid](../../crates/octara/src/kernel.rs)（1129 行）。
不能把“所有语义状态均可直接 style_when”写成现状。

### 建议

总提案 §8.3 补目标规则表，并标明哪些保持现状：

- 默认外观、variant、调用者 base 的排列，以及动态 patch 撤回时的 fallback。
- 状态/断点阶段与同阶段冲突次序；覆盖 hover 反馈的明确入口。
  不能同时承诺“调用者总赢”和“状态总在 base 后赢”。
- 仅对可继承属性定义继承默认与显式局部值的关系。
- 视觉优先级不能修改动作资格。覆盖禁用颜色不会使动作合法，
  但无样式能力仍需说明焦点提示、命中尺寸等外观义务由谁承担。
- invalid 样式读取已解释的语义/校验投影即可。新增只读条件不违反唯一状态源，
  独立 set_visual_invalid 才会制造新的写入来源。

Base UI 的状态属性/回调服务于样式；Popover 的 available-height、anchor-width 是运行中几何，
并非品牌设计 token。
来源：[Base UI Styling](https://base-ui.com/react/handbook/styling#style-hooks)。
Octara 同样要区分资源/尺度、kit 语义 token、变体与运行中几何。
组件专属 token 移出内核值得采用，还需规定缺失值、fallback、别名和主题失效；
“品牌提供值”没有回答这些规则，通用颜色槽仍应逐项审计。

**待验证：**默认 hover + variant + 调用者 base/hover、patch 撤回、多状态同时激活、
继承颜色、主题变化中的预编辑。Inspector 应能解释属性阶段/来源。

## 7. 何时公开无样式行为

Base UI 不打包 CSS，提供部件/状态样式入口，证明完整无样式产品可以形成稳定契约；
并不证明任何库都需立即公开同等部件面。
来源：[Base UI Styling](https://base-ui.com/react/handbook/styling)。
SwiftUI 区分沿用标准交互的 ButtonStyle 与可自定义交互的 PrimitiveButtonStyle；
普通 ButtonStyleConfiguration 提供 label、isPressed、role。
来源：[ButtonStyle](https://developer.apple.com/documentation/swiftui/buttonstyle)、
[PrimitiveButtonStyle](https://developer.apple.com/documentation/swiftui/primitivebuttonstyle)、
[ButtonStyleConfiguration](https://developer.apple.com/documentation/swiftui/buttonstyleconfiguration)。
这是比“替换整个根”更窄的定制反例。

**建议：**保留“kit 内先共享行为”，收窄“第二种外观就提取”：
第二套颜色、尺寸、品牌通常只需 token/variant/标签内容入口；
必须重排部件、改变视觉结构且仍保留同一语义状态机，才是公开行为组合的强证据。
内部重复也可先收敛，不自动构成稳定公共 API 的需求。

公开前至少证明：

1. 默认皮肤与一个有产品用途的不同布局复用行为，应用无需复制焦点、IME、名称、弹层逻辑。
2. 所有权、状态资格、禁用、事件顺序和结构前提有契约与诊断。
3. 去掉默认外观仍有最小功能验收；必要定位/裁剪/命中条件与可访问外观义务有承担者。
4. 短 builder 走同一行为，常见控件不用手写全部 part。

这是 API 稳定性门槛，不要求新 crate、统一 Widget trait 或新调度器。

## 8. Rust + Signals 应消除的 React 语义假象

React render 重新调用组件函数，事件闭包读取那次 render 的状态快照，
setState 请求下次 render，不改变已经捕获的快照。
来源：[State as a Snapshot](https://react.dev/learn/state-as-a-snapshot)、
[Render and Commit](https://react.dev/learn/render-and-commit)。
SwiftUI 也区分短暂 View 值与持续身份/状态生命周期；
这支持身份连续性，不支持复制 body 重求值到一次构建的 Octara。
来源：[Apple Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/)。

| 容易产生的预期 | Octara 应明确 |
| --- | --- |
| 改 Signal 重跑周围函数/普通 if | 一次构建、绑定、结构区域的区别；只有规定闭包重求值 |
| builder 中提前 get 的普通值以后自动变 | Rust 普通捕获与 Signal 句柄不同；动态读取应在追踪入口 |
| on_input 是绑定唯一入口 | kit 行为可能已绑定；通知不应重复写同一次值 |
| render 闭包像 React render 一样反复执行 | 每个闭包是构建工厂、绑定计算还是事件动作；次数、阶段、所有者都需说明 |
| 新 render 自然替换旧回调 | 回调归挂载作用域；来源更新、释放与存活检查有自己的机制 |
| key、id、label 可互换 | 结构连续性、运行中目标身份、语义查找分别定义 |
| portal 自动具备 React 冒泡 | 仅承诺 Octara 的 scope/context、输入路由和 overlay 规则 |
| 框架能回滚任意 Rust 回调 | 阶段/存活约束仍在，但不承诺撤销任意 Rust 副作用 |

**建议：**采用 value/open/disabled/label 等通用词；公开 render/slot/controlled 时附求值与所有权规则。
不为每个属性强行提供 defaultX/onXChange 的对称组合。
可用性实验用相同任务、模型、上下文与工具并保留失败样本；
无实验数据前只声称词汇熟悉，不声称训练覆盖或成功率。

## 9. 对总提案的具体修订定位

| 位置/主题 | 需要的修订 | 原因 |
| --- | --- | --- |
| §6、§8.3 输入/受控值 | 外部值、预编辑、拒绝、异步、提交冲突表 | “有优先级”不足以判断行为 |
| §7.1 事件顺序 | 提议、当时接受值、提交结果；取消独立于通知 | 顺序不能暗示业务已接受 |
| §8.2 Field/NumberField | 保留阶段的共享结果、空值与约束策略 | 两种覆盖反例方向相反 |
| §8.2 部件 | builder 对照、基数/归属/解绑表 | Root/Part 不自动减少关系成本 |
| §8.2 单根/Trigger | 区分标签、根替换、相容关系和冲突动作 | 合并不等于安全组合 |
| §8.2 无样式公开条件 | 真实布局复用与契约证据作为门槛 | 第二种颜色不是充分条件 |
| §8.3 样式 | 阶段/来源优先表、只读语义投影 | 当前 INVALID 尚不在 State |
| §10 Chat 删除项 | 只删除已被新策略替代的重复逻辑 | 保留空值默认、整数等业务需求 |
| §12 切片 D | 纳入下面最小案例 | 配置位置正确不代表编辑、名称、禁用成立 |

## 10. 最小验收案例

以下均为后续验收说明，本轮未执行、未新增测试。
每例记录输入路线、模型/编辑状态、语义/Inspector 输出；
IME、焦点、辅助技术需真实宿主证据。

| 案例 | 最小动作 | 必须观察 |
| --- | --- | --- |
| A. 相反校验策略 | 必填数值与空串默认值字段输入空串、负号、2.1；token 输入 1.5 | 必填/可选不同；格式、范围、整数规则不被 Field 隐藏错误抹掉 |
| B. 保存时序 | 选定拒绝/规范化策略，越界后直接点 Save | 接受值符合策略；blur 不替产品意外改决定；通知次数明确 |
| C. Builder/parts 对照 | 普通 Field、前后缀输入、选择器分别配置 disabled/on_input | 命中实际控制项；名称/描述一致；关系成本可比较；错误 wrapper 可诊断 |
| D. 动态关系 | 同名 Field、嵌套 Root、条件 Error、两个 Trigger；移除打开者后关闭 | 不串关系；基数按组件契约；解绑完成；焦点目标过期有退路 |
| E. 同根行为链 | Tooltip + Trigger，记录组件写模型后通知；前层卸载组件 | 一个交互根；提议/接受值符合契约；退休后续回调不执行 |
| F. 禁用维度 | 按下后禁用、键盘/AX/Inspector 激活、父禁用子 false | 动作被拒；pressed/capture 不滞留；焦点/可发现性一致 |
| G. IME/外部值 | 中文预编辑中改错误、主题、筛选，再外部重置；Enter/Escape | 保留编辑身份；重置遵守政策；不误选项/提交/关闭 |
| H. 拒绝/延迟 | 同步拒绝/变换；异步检查逆序完成 | 提议与接受可区分；旧结果不覆盖新草稿；selection/undo/pending 可解释 |
| I. 样式溯源 | variant + base/hover + patch 撤回 + 多状态 + 继承；invalid 变化 | 属性来源可解释；语义驱动外观；样式不改变动作资格 |
| J. 一次构建 | 计工厂/绑定次数，只改标签/值，再切结构 key | 无关更新不重跑工厂；绑定更新；仅定义的身份变化重建局部状态 |

这些案例可否定过强主张，也能验证 Rust + Signals 的保留方案；
不替代各宿主后续完整交互、无障碍与性能验收。
