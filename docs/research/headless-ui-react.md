# Headless UI React 的组件、机制与 Octara 分层

Headless UI React 把完整控件行为装进可组合的部件，同时复用真实焦点、集合导航、弹层仲裁和语义关系。对 Octara，最值得采用的是让 Kit 配置真正参与交互的节点，让 Core 及宿主统一需要跨节点或跨系统协调的能力。应用用普通 Rust 函数组装，可以直接使用 Core，也可以用 Kit 完成常见交互。[Octara 的分层与原语准入](../ARCHITECTURE.md)

调研基线为 Tailwind Labs 的 `@headlessui/react` **2.2.10**，发布于 2026 年 4 月 7 日，对应提交 `d13526d02a2de92c4ad7b62c15cd980636543fe2`。公开面按发布包根入口的运行时命名导出统计，机制按同一提交核对。本轮结论来自发布包与源码的静态审查。文末的 Octara 场景是验收设计。[npm 版本](https://www.npmjs.com/package/%40headlessui/react?activeTab=versions) · [发布记录](https://github.com/tailwindlabs/headlessui/releases/tag/%40headlessui%2Freact%40v2.2.10) · [固定版本导出表](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/index.ts)

## 公开能力全表

包根入口有 **65 个运行时命名导出**。53 个是当前可渲染组件或部件，10 个是废弃的可渲染名称，其余两个是枚举 `FocusTrapFeatures` 和 Hook `useClose`。官方目录中的 16 个组件族占 46 个当前导出，共用表单部件和工具占 7 个。[官方目录](https://headlessui.com/) · [发布版本的组件源码](https://github.com/tailwindlabs/headlessui/tree/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components)

| 组件族 | 当前导出 | 行为 |
| --- | --- | --- |
| Menu | `Menu`, `MenuButton`, `MenuItems`, `MenuItem`, `MenuSection`, `MenuHeading`, `MenuSeparator` | 在命令集合中导航、按字查找、执行并关闭。 |
| Combobox | `Combobox`, `ComboboxInput`, `ComboboxButton`, `ComboboxOptions`, `ComboboxOption` | 搜索输入保留焦点，选项独立高亮和提交，支持单选、多选和虚拟列表。 |
| Listbox | `Listbox`, `ListboxButton`, `ListboxOptions`, `ListboxOption`, `ListboxSelectedOption` | 自定义单选或多选列表，可在触发器内显示选中值。 |
| Radio Group | `RadioGroup`, `Radio` | 互斥选择，方向键移动实际焦点并选择。 |
| Tabs | `TabGroup`, `TabList`, `Tab`, `TabPanels`, `TabPanel` | 标签与面板配对，支持方向和自动或手动激活。 |
| Dialog | `Dialog`, `DialogPanel`, `DialogTitle`, `DialogBackdrop` | 模态范围、初始焦点、关闭与焦点恢复。 |
| Popover | `Popover`, `PopoverButton`, `PopoverPanel`, `PopoverBackdrop`, `PopoverGroup` | 锚定面板，管理触发器、面板与同组成员之间的焦点。 |
| Disclosure | `Disclosure`, `DisclosureButton`, `DisclosurePanel` | 在文档流中展开与收起。 |
| Transition | `Transition`, `TransitionChild` | 协调进入、退出和后代退出后的卸载。 |
| Button | `Button` | 使用原生按钮，提供焦点、悬停和按压反馈。 |
| Checkbox | `Checkbox` | 用外部值或内部默认值管理勾选，支持中间态。 |
| Switch | `Switch` | 用外部值或内部默认值管理开关。 |
| Fieldset | `Fieldset`, `Legend` | 表单分组、组名与禁用状态传递。 |
| Input | `Input` | 原生单行输入，接入字段语义和交互状态。 |
| Select | `Select` | 原生 HTML 选择控件。 |
| Textarea | `Textarea` | 原生多行输入，接入字段语义和交互状态。 |

### 共用部件、工具和关闭能力

| 公开入口 | 责任 |
| --- | --- |
| `Field`, `Label`, `Description` | 把控件与已挂载的标签和说明关联，并传递禁用状态。 |
| `DataInteractive` | 给自定义节点提供焦点、悬停和按压状态。 |
| `CloseButton`, `useClose` | 调用最近的 Dialog、Disclosure 或 Popover 的关闭操作。Menu 通过自己的 render prop 提供 `close`。 |
| `FocusTrap`, `FocusTrapFeatures` | 可独立配置 `InitialFocus`、`TabLock`、`FocusLock`、`RestoreFocus` 和 `AutoFocus`，也可用 `None` 关闭这些能力。 |
| `Portal` | 改变内容的实际挂载位置，并参与登记嵌套 Portal 的范围。 |

这里的 `DataInteractive` 提供交互反馈，按钮语义与业务动作由使用者声明。Portal 的挂载位置也可以独立于模态策略配置。[DataInteractive](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/data-interactive/data-interactive.tsx#L29-L56) · [关闭上下文](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/close-provider.tsx) · [FocusTrap](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/focus-trap/focus-trap.tsx#L53-L88) · [Portal](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/portal/portal.tsx#L208-L259)

### 废弃入口

10 个顶层名称是 `ComboboxLabel`、`DialogDescription`、`ListboxLabel`、`PopoverOverlay`、`RadioGroupOption`、`RadioGroupLabel`、`RadioGroupDescription`、`SwitchGroup`、`SwitchLabel`、`SwitchDescription`。其中七个指向同一个 `Label` 或 `Description` 组件。`PopoverOverlay`、`RadioGroupOption`、`SwitchGroup` 有各自的旧实现。[发布版本组件导出](https://github.com/tailwindlabs/headlessui/tree/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components)

旧式点号可渲染入口共有 40 个。表中的部件名加上所属组件和点号即为旧入口。`Portal.Group` 是唯一只通过点号暴露的可渲染部件。`Transition.Root` 指向 `Transition`，`FocusTrap.features` 另外指向 `FocusTrapFeatures` 枚举。[旧 Portal 入口](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/portal/portal.tsx#L254-L259) · [旧 Transition 入口](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/transition/transition.tsx#L637-L648)

| 所属组件 | 旧式点号部件 |
| --- | --- |
| `Combobox` | `Input`, `Button`, `Label`, `Options`, `Option` |
| `Dialog` | `Panel`, `Title`, `Description` |
| `Disclosure` | `Button`, `Panel` |
| `Listbox` | `Button`, `Label`, `Options`, `Option`, `SelectedOption` |
| `Menu` | `Button`, `Items`, `Item`, `Section`, `Heading`, `Separator` |
| `Popover` | `Button`, `Backdrop`, `Overlay`, `Panel`, `Group` |
| `Portal` | `Group` |
| `RadioGroup` | `Option`, `Radio`, `Label`, `Description` |
| `Switch` | `Group`, `Label`, `Description` |
| `Tab` | `Group`, `List`, `Panels`, `Panel` |
| `Transition` | `Child`, `Root` |

## 可复用机制及其边界

### 配置实际交互节点

`as` 选择承载行为的元素。渲染层把 ref、语义属性和事件合并到该元素，并以 render props 和 `data-*` 提供状态。使用 Fragment 转发这些属性时需要一个能够承接它们的子元素。属性发生冲突时，组件拥有的属性优先，调用方事件先执行，可通过 `preventDefault()` 中止后续事件处理。禁用时，合并器禁用相应交互处理器。[节点渲染](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/render.ts#L130-L270) · [属性与事件合并](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/render.ts#L339-L394)

按压也有自己的生命周期。React 实现跟踪指针离开、移动、抬起和取消。菜单类触发器根据设备类型选择开始时刻，鼠标可在焦点变化前打开，触摸和笔走 click 路径。从触发器按下、拖到选项再抬起也有独立判定。这些路径能共用命中与取消信号，最终是否打开、选择或执行由控件决定。[按压状态](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-active-press.tsx#L38-L97) · [设备与开关时机](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-handle-toggle.tsx#L6-L53) · [拖动释放](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-quick-release.ts#L22-L113)

Octara 用 `El::behavior` 把组件行为与应用回调放在同一挂载节点上，顺序为 IME 路由、组件行为、应用回调。Kit 的 `select` 与 `combobox` 把控件的角色、名称、展开状态和输入行为安装到应用提供的触发元素上。应用可在同一个元素上设置样式、禁用和提示，并追加按压回调。[Octara `El::behavior`](../../crates/octara/src/view/element.rs) · [行为层](../../crates/octara/src/view/behavior.rs) · [Kit 选择器](../../crates/octara-kit/src/choices.rs)

### 集合身份、焦点、活动项和提交

Headless UI 的集合机制能根据活着的节点更新位置，在重排时保留同一个活动项，移除后清理目标。部分复合控件共用 reducer、订阅和清理机制，各自维护控件状态。Combobox、Listbox、RadioGroup 提供受控值和默认值两种归属，支持用 `by` 比较业务值。动态选项、当前活动项和已提交值可以独立变化。[集合重排](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/menu/menu-machine.ts#L69-L92) · [活动项计算](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/calculate-active-index.ts#L25-L94) · [共用状态机](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/machine.ts#L5-L88) · [受控值](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-controllable.ts#L5-L45)

| 控件 | 实际焦点 | 导航和提交 |
| --- | --- | --- |
| Menu | 菜单容器 | 活动项是当前命令目标，确认时执行。 |
| Listbox | 选项容器 | 活动项与选中值独立。单选提交后关闭，多选可继续。 |
| Combobox | 编辑器 | 活动项作为编辑器的语义关系发布。应用可独立维护查询，确认后写入选择。 |
| 手动激活 Tabs | 当前 Tab | 方向键改变焦点，确认后才切换选中 Tab 和面板。 |
| RadioGroup | 当前 Radio | 方向键移动焦点，也更新选中值。 |

Combobox 让 IME 优先处理组词期间的 Enter，同时仍可把编辑变化交给应用参与过滤。Octara 的宿主和编辑器已经有系统组合区与草稿协议。Kit 需要根据产品定义何时使用草稿过滤、何时提交业务值。集合顺序、禁用项过滤和身份解析可以共享，焦点是否移动、导航是否提交以及提交什么留给各控件。[Menu 键盘处理](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/menu/menu.tsx#L458-L556) · [Listbox 提交](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/listbox/listbox-machine.ts#L513-L543) · [Combobox 输入](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L678-L825) · [Tabs 键盘处理](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/tabs/tabs.tsx#L444-L502) · [RadioGroup 键盘处理](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/radio-group/radio-group.tsx#L223-L275) · [Octara 文本输入](../design/input.md)

### 弹层位置、逻辑范围和模态权限

定位逻辑根据锚点和空间更新面板。Portal 管实际挂载位置并登记后代。弹层栈决定哪层有权处理 Escape 或外部按压。Dialog 还限制背景交互并恢复焦点。Popover 能把触发器、面板、嵌套 Portal 和同组弹层视作逻辑范围，若在外部点到可聚焦控件，可以保留焦点的新去向。焦点隔离、滚动锁和退出生命周期各有自己的释放时机。[锚点定位](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/floating.tsx#L197-L248) · [弹层栈](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-is-top-layer.ts#L29-L65) · [Dialog](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/dialog/dialog.tsx#L191-L269) · [Popover](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/popover/popover.tsx#L175-L240)

在 Octara，Kit 声明弹层的锚点和关闭条件，窗口依据挂载关系和输入会话确定谁响应。当前 Kit `popover` 使用模态行为，`popup_list` 通过失焦关闭。用非模态建议列表嵌套在 Dialog 内的场景，可以复核外部按压、焦点离开和恢复能否使用现有接口组合。[Octara `popover`](../../crates/octara-kit/src/popover.rs) · [Octara `popup_list`](../../crates/octara-kit/src/popup_list.rs)

### 虚拟选项与退出时序

Combobox 是这版 React 包提供公开虚拟列表接口的组件族。导航基于完整逻辑选项，活动项移出视口时可以请求滚动。可访问的活动后代关系只指向已经挂载且与当前活动项相符的行。挂载行也报告自己在完整集合中的位置和总数。Octara 的 `virtual_each` 已使用业务 key 保留行身份，可用同一个 key 串起导航、滚动和语义关系，再用挂载身份确认本次操作的目标。[虚拟列表用法](https://headlessui.com/react/combobox#virtual-scrolling) · [虚拟行测量](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L134-L232) · [活动目标校验](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox-machine.ts#L608-L628) · [Octara 虚拟集合](../../crates/octara/src/view/list.rs)

Transition 让正在退出的内容保留绘制，等待参与过渡的后代完成再卸载，重新打开会取消过时的退出完成。Combobox 在离开期间还可以保留退出内容的视觉快照。Octara 应按场景定义输入、可访问对象、绘制与卸载各自的生效时刻。弹层开始关闭后，旧 Agent 目标的有效性应由窗口根据当前交互权限判定，独立于旧像素的存在。[嵌套退出](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/transition/transition.tsx#L176-L274) · [取消过渡](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-transition.ts#L255-L284) · [退出快照](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L1334-L1398)

### 字段语义与系统编辑

`Field`、`Label` 和 `Description` 组合已挂载的语义关系。`Input`、`Textarea` 和 `Select` 使用浏览器原生控件，选项类组件通过表单归属、禁用、提交和重置协议参与原生表单。对 Octara，Kit 可组合标签、说明、控件和模型绑定，窗口与宿主负责真实编辑会话、选区、组合区、候选窗锚点与平台辅助功能。标签文字变化应通过关系更新到原来的控件，并在同一更新周期内供 Inspector 和平台语义读取。[Field](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/field/field.tsx#L28-L74) · [表单参与](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/form-fields.tsx#L21-L105) · [Octara 字段组合](../../crates/octara-kit/src/forms.rs) · [窗口语义](../../crates/octara/src/kernel/semantics.rs)

## Octara 的设计取舍与验收

保留三条责任边界。**应用**拥有业务 ID、数据来源和提交策略。**Kit** 组合可直接使用的行为，并允许调用者配置实际部件，多个组件可以共享内部实现。**Core 与宿主**拥有挂载身份、真实焦点、输入路由、窗口级协调和系统编辑。只有当实际调用方必须复制窗口状态、争抢同一次输入或绕过系统协议时，才评审更通用的 Core 入口。[Octara 原语准入与身份](../ARCHITECTURE.md)

Chat 的工具栏模型选择器在 `combobox` 的触发工厂里设置样式、提示、空状态文案与禁用条件。模型列表为空或有内容时都使用同一个选择器，窗口和 Agent 从实际触发器读取语义与可用动作。模型 ID 表示业务选择，触发器和选项由框架分配各自的挂载身份。[Chat 模型选择器](../../apps/chat/src/ui.rs) · [Kit 选择器](../../crates/octara-kit/src/choices.rs)

| 场景 | 验收条件 |
| --- | --- |
| Chat 模型选择器 | 空集合禁用，模型恢复后可用。样式、提示、角色与 Agent 动作指向实际触发器。重名模型可按稳定 ID 区分。 |
| Combobox 搜索和虚拟选项 | 中文组词的 Enter 只服务当前输入法操作。确认词后再确认选项时，业务提交一次。键盘导航到视口外的选项时，先滚动挂载，再发布可解析的活动后代关系。 |
| Dialog 内的非模态浮层 | Escape 只关闭有权处理的顶层。外部可聚焦节点可以接收焦点，关闭时只恢复仍有效的目标。嵌套浮层的逻辑范围在挂载位置变化后仍成立。 |
| 关闭并重新打开的浮层 | 旧画面可以完成退出，输入和 Agent 操作资格按窗口的当前状态判定。过时的退出完成事件不会卸载重新打开的内容。 |
| 受控字段与手动 Tabs | 修改字段标签后，编辑器保持挂载身份并报告新名称。手动 Tabs 的键盘焦点与已选面板可以独立变化，确认后才切换面板。 |

这些场景用来区分现有组合能力、Kit API 缺口和真正需要窗口仲裁的契约。时序与状态可先用无窗口驱动复核，系统 IME、平台焦点与辅助功能输出需要对应宿主的观察结果。[Octara 的验证边界](../ARCHITECTURE.md#怎样判断架构收敛了)
