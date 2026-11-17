# 前端组件语义与 Rune、Exact 的交叉审查

日期：2026-11-17。状态：调研与设计修订，未实施。
目标：检验“熟悉的前端语义是否适合 Octara”，包括支持证据、反例和适用边界。
本文补充[分层架构](../design/layered-architecture-redesign.md)与[框架架构调研](ui-composition-frameworks.md)。

## 1. 证据范围

- Octara：`b117aeea644d2480ff62be0c4a004ac7c7ed0fbc`，现有 kit、事件层、主题和组合入口。
- Rune：本地源码 `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`。
- Exact：本地源码 `08c889a9de21a30b7c3b308422fc588cf1a480d3`。
- React、Base UI、Tailwind：本日查阅的官方文档。Base UI 指 base-ui.com 的 React 组件库。
- 本轮未执行这些参考框架、未做新增窗口实验；“交叉审查”指源码/公开契约对照，不能当作行为测试通过。
- “AI 更熟悉前端模式”是待验证的可用性假设；没有训练数据统计或生成成功率证据。

## 2. 结论矩阵

| 想法 | 交叉证据 | 判断 | Octara 决定 |
| --- | --- | --- | --- |
| 使用熟悉的控件、属性、样式词汇 | Rune 明确沿用前端组件命名；Exact 明确使用 HTML/CSS 名称 | 支持降低概念迁移成本，未证明 AI 成功率 | 采用熟悉词汇，同时列出事件、更新和平台差异 |
| 行为、外观分离 | Base UI 暴露部件；Rune runtime 管模态焦点、kit 管控件及主题；Exact 分控件契约与宿主实现 | 支持职责分离，不证明必须单独建 headless crate | kit 内先形成共享行为实现，默认外观复用它 |
| 所有复杂组件必须拆成 Root/Part | Base UI 使用显式部件；Rune 多用 builder；Exact 的 slot 仅一个内容插槽 | 部分支持 | 复杂组件按需求提供命名部件，简单组件保持简短；不统一拆成大量公开类型 |
| 状态由应用控制，控件发出意图 | Rune Combobox 区分 current 与 search/open/highlight；Exact 区分 value 与 input/change | 支持 | 明确业务值、编辑草稿和交互状态各自所有者 |
| 一个交互根可叠加行为 | Base UI 合并 props/refs；Rune 按钮分根、标签与动作；Octara 已有 Behavior | 支持，但顺序不能照搬 | 保留一个目标和既有事件层，明确冲突与消费规则 |
| 同名样式具有统一覆盖规则 | Tailwind 依 CSS 生成顺序；Exact 用节点属性覆盖 named style；Rune 用 kit token 构建 | 支持明确规则，反对假定它们规则相同 | 使用 Rust 类型化样式与单一解析规则，公开解释优先级 |
| 视觉正确就足以说明语义正确 | Rune Field 的 group 标签；Exact 按钮后代和数字字段历史问题 | 反例 | 控件关系和真实宿主行为独立验收 |

## 3. 熟悉的词汇应携带可兑现的契约

Rune kit README 明确以 `@opencode/ui` 的名称、变体、尺寸和状态帮助迁移界面；
Exact `control.rs` 则以 HTML 控件名称解释 ControlKind，并集中定义 select 的有效选项规则。
这支持采用 Button、Dialog、Field、Select、value、disabled、placeholder 等熟悉词汇。
来源：[Rune kit README](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/README.md)、
[Exact control.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/control.rs)。

但同名也可能制造错误预期。React 文本输入的 `onChange` 随用户编辑立即触发；
Exact guide 将 `input` 用于草稿编辑，`change` 用于 Enter/blur 规范化。
所以“统一叫 on_change”本身不能让接口更明确。
来源：[React input](https://react.dev/reference/react-dom/components/input)、
[Exact 输入契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#input-events-and-commands)。

Octara 必须分别说明：何时触发、载荷是提议值还是接受值、是否发生在组合输入中、何时更新模型。
使用熟悉的名称不意味着 JSX、DOM Event、React 重渲染或浏览器完整默认行为也随之存在。
输入激活优先使用可覆盖指针/键盘/辅助技术的语义动作；平台原始事件作为更低层能力保留。

## 4. 行为与外观分离，但不先制造完整新框架

Rune Dialog 的 open 来自应用，焦点陷阱、背景 inertness 和恢复由 runtime 模态栈负责；
kit builder 构造表面、标题、关闭按钮并调用 modal 原语。
模态栈还区分对话框与指针打开的菜单在恢复焦点环时的行为。
来源：[dialog.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/dialog.rs)、
[modal_focus.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/modal_focus.rs)。

这部分源码以 runtime + kit builder 完成职责分离，没有要求所有组件经过独立公开的无样式包。
Exact 的部分基础控件语义又位于 kernel，由多个宿主兑现，不能说各参考都把行为放在同一层。

**修订：**Octara 采用“基础机制 → 语义行为组合 → 默认外观”的责任边界。
语义行为组合首先是 kit 内可复用实现；自定义外观通过受控部件/内容入口使用它。
有第二种外观、现存重复逻辑或明确高级组合需求时，才提取对应公开无样式能力。
不建立独立调度器、通用控件状态机框架或强制所有组件实现的 trait。

行为分离不等于可以完全省略结构约束：命中区域、焦点、定位、滚动和隐含布局可能是正确交互的条件。
默认视觉尺寸可改，不能在无说明的情况下把必要条件一起删掉。

## 5. 命名部件、builder 和内容插槽共同使用

Base UI Field 的 Label、Control、Description、Error 使配置位置明确。
Rune Field 的 builder 则允许调用者先配置 control，再组合进字段。
Exact 的 slot 保留调用者词法作用域和 provider，但其 guide 明确只提供一个插槽，
不等于命名插槽或任意 render function。
来源：[Base UI Field](https://base-ui.com/react/components/field)、
[Rune field.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/field.rs)、
[Exact slots](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#components-providers-and-slots)。

**反例：**查阅的 Rune Field::build 给外层 group 设置 label/description，并隐藏可视 label，
随后直接放入传入的 control；这一函数本身没有给 control 补上标签关系。
不能由 Field 名称或外观推定内部输入已经获得可访问名称，也不能据此断言所有 Rune 调用现场无名称。

Octara 的选择：

- 简单按钮有短构造；复杂布局可配置标签内容，不要求用户手工组装每个内部节点。
- Field/弹层/选择类组件按实际需求提供有名的控制点，明确布局根和交互根。
- 部件关系在所属组件范围内建立，自动维护 label/description/控制关系；字符串 id 用于外部查找，
  不要求应用拼接内部关系。错误嵌套、缺失必需部件和重复唯一部件应可诊断。
- 部件身份与所有权沿用现有 scope/结构区域，不另建全局部件注册表。
- 挂载到 overlay 层改变展示位置，不应悄悄改变回调所有者与组件生命周期。
  这是 Octara 的目标契约，不声称与 Exact slot 的词法 provider 行为完全相同。

## 6. 单一根上的组合需要明确冲突规则

Base UI 的 render 合并在一个元素上保留多个处理器和 ref，但对属性、style、disabled 的合并规则不同，
处理器还可阻止后续 Base UI handler。这个机制证明了组合契约的重要，也体现了其学习成本。
来源：[Base UI composition](https://base-ui.com/react/handbook/composition)。

Rune Button 把根角色和名称、视觉标签、动作分别组织；自定义标签保留显式按钮名。
但其自定义标签整体标为 a11y_hidden，不是任意有独立语义的子内容都能安全照抄的策略。
来源：[Rune actions.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/actions.rs)。

Octara 保留组件 Behavior 与应用通知的区分，不能借鉴 render API 后悄悄改成另一种回调顺序。
一次更换 Trigger 视觉应保留身份、名称、禁用与焦点；同一根声明冲突角色或不兼容主动作时应明确拒绝，
而不是“最后合并的属性赢”。内部正确性必需的禁用行为不受视觉样式覆盖。
装饰内容隐藏；带独立意义的后代按控件契约处理；嵌套可交互控件不靠自动拼接名称来补救。

## 7. 状态与样式：借词汇，定义自己的规则

Rune Combobox 的 committed selection 由 current/on_choose 控制，search/open/highlight 在传入的
ComboboxState；实现组合了 Popover、TextInput 和 PopupList，并在 preedit 存在时不消费选项导航键。
来源：[combobox.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs)。

Octara 可用作用域 Signal 承载同样的不同事实，无需把该状态结构和所有权 API 原样搬来。
如果一个属性同时支持 controlled/uncontrolled，必须说明初始化、写入接受和模式切换；
本轮不为 API 对称强制所有属性提供两种模式。用户编辑草稿也不能被“受控值”概念抹掉。

Tailwind 的冲突工具类按生成样式表顺序取胜，并非 class 字符串中后写的取胜。
Exact named style 则由节点本身的属性覆盖，明确没有完整选择器级联。
来源：[Tailwind 样式冲突](https://tailwindcss.com/docs/styling-with-utility-classes#managing-style-conflicts)、
[Exact 样式契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#styling-and-layout)。

因此 Octara 保留类型化样式，统一默认外观、变体、调用者设置、条件样式和继承的优先级，
不许不同组件各写一套优先规则。具体规则应与现有声明顺序机制核对，不在此声称简单的“最后写入总赢”。
状态样式读取真实的 disabled/expanded/checked 等状态；不得另建可独立修改的视觉状态副本。
Inspector 应解释最终样式和状态来源，复用现有检查设施。

另一个架构启发是 token 的所有权：Rune core 支持主题数据，kit 定义组件所需的 token schema，
品牌包提供值。Octara core 当前包含 SurfaceHover、Accent 等内置槽位。
目标应区分通用资源/尺度解析与 kit 的设计语义；组件专属 token 不继续向内核堆积。
现有通用槽位是否迁出须逐项审计，不为了搬名字新增一套主题运行时。
来源：[Rune kit 分层说明](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/README.md)、
[Octara theme.rs（审查基线）](https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/theme.rs)。

## 8. 宿主反例与验收

Exact 的历史记录包括数字字段在 painted host 未实现步进，以及按钮内部 heading 在 macOS 语义树消失。
这些记录分别指向特定旧提交；本轮没有运行确认当前状态，也不把某宿主结论扩展到所有宿主。
它们说明熟悉的声明名称需要真实兑现，不能仅检查编译、像素或角色字符串。
来源：[数字字段记录](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/20261110-number-field-is-plain-text-on-painted-hosts.md)、
[按钮语义记录](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261010-macos-a-heading-inside-a-button.md)。

后续实施验证分为两条：

| 验证 | 场景与标准 |
| --- | --- |
| 框架契约 | 自定义图标标签保持一个焦点和正确名称；Field 名称归实际控件；过滤不丢 IME；嵌套弹层恢复焦点；业务拒绝提议值时显示一致；不支持的结构/组合明确诊断 |
| Agent 使用体验 | 使用同一模型、上下文与工具，比较现有 API 和候选 API 完成上述任务的多次尝试；记录编译错误、语义遗漏、查询/修复次数和最终窗口正确性；保留失败样本，不用一个演示宣称更易用 |

原生输入、Inspector 动作、语义树和截图分别留证据；合理的跨平台交互差异由契约解释。
本轮没有新增测试或统计，所有验收都是后续计划。

## 9. 对总方案的修订

采用熟悉词汇、明确语义部件、行为与外观分离、可解释的样式与状态；
保留 Rust + Signals 和现有生命周期/事件层。
将独立 headless crate、所有控件强制 Root/Part、完整 CSS 级联、任意 render 属性合并列为不采用的默认方向。
本轮的依据支持这些责任边界，不足以宣称某个尚未实现的公共 API 已通过交叉验证。

## 10. 独立子 Agent 审查与采纳记录

同日由三位 GPT-6.1 SOL、max 推理强度的子 Agent 分别审查，主线程复核关键源码并整合。
独立报告保留证据与局限；下表是本轮设计决定，不把三方建议全部转成新增机制。

| 独立报告 | 主要贡献 |
| --- | --- |
| [前端组件契约](agent-frontend-contract-review.md) | 校验覆盖的相反需求、Root/Part 的真实关系成本、动作与通知、受控输入冲突、样式阶段、公开无样式能力门槛 |
| [Rune 架构](agent-rune-architecture-review.md) | modal 与非 modal 的区别、custom trigger 单槽覆盖、重建式编辑/主题快照不能照搬、文本投影、通用 token、虚拟列表退化边界 |
| [Exact 架构](agent-exact-architecture-review.md) | 结构合法性与测量分别建模、compiler/kernel/runner/host 的保证分布、当前源码与历史 issue 的区别、IME 仲裁、native/bare 资格、失败后发布限制 |

| 主线程决定 | 对总方案的具体修订 |
| --- | --- |
| 保留内容所有权硬契约，暂不锁死 Rust 类型方案 | §5.1：区分应用 children、内部结构、测量与语义后代；比较窄类型与早诊断 builder |
| 公开无样式能力需有实际结构复用 | §8.2：换颜色优先 token/variant，内部复用不自动要求公共 API |
| 普通 builder 与命名部件并存 | §8.2：逐类定义基数、关系、稳定边界、实际打开者与解绑，不按视觉父链猜所有权 |
| Field 校验须明确政策 | §8.2：草稿解释、业务接受、展示/播报分别建模；修正原审查对 invalid 覆盖的泛化 |
| 保留 Octara 编辑、事件和失效机制 | §6–8：不搬 Rune TextEdit 快照；通知不能事后撤销；主题迁层不失去动态更新 |
| 共享窗口机制不统一所有 popup 策略 | §7.1：定位、模态、背景输入资格、焦点恢复分别规定，overlay 保持局部交互归属 |
| 不机械迁移全部 token | §8.3：底层通用文本/链接/焦点默认值可保留，组件专属语义归 kit，避免 core 反向依赖 |
| 既有样式规则优先复用 | §8.3：声明顺序与条件阶段共同解释；只读 invalid 投影不等于独立可写视觉状态 |
| 宿主验收与资源退化边界独立检查 | §9、12：不能从共用内核推出宿主等价；虚拟列表重试不能无说明地挂载所有行 |

对历史证据进一步澄清：当前 Exact 参考快照的 macOS 按钮 heading 已有修复与回归测试源码，
不能仍按旧 issue 称其当前未修复；painted 路径的差异线索另列在 Exact 独立报告中。
本轮没有执行这些测试或原生窗口，只有源码/文档层面的确认。

尚未定稿：具体 builder 类型、公开部件签名、受控编辑仲裁的最终策略、按组件的 disabled 维度和校验配置。
这些问题已有最小验收案例，应在贯穿控件的设计/实现切片中决定，不先创建通用系统覆盖所有组合。

本次仅新增和修订文档，未修改实现或测试代码；未新增运行验证或 Agent 成功率实验。
