# 用函数组装一个控件

考虑一个带图标、文件名和状态标记的文件行。应用用普通 Rust 函数组装容器、文字和图像，把“打开文件”动作和可访问名称挂在承担交互的对象上。改变图标或换行属于外观调整；打开哪个文件由业务 key 决定。只有需要独立状态寿命时，才为这一段内容建立所有者。[当前构造器](../../crates/octara/src/view/) · [结构身份](reactive-core.md#4-identity-and-state)

## Element content

构造器在创建节点时确定内容类型。容器可用 `.child(..)` 和 `.children(..)` 挂载应用子节点。图片使用 `image(..)` 配合 `.object_fit(..)`。编辑器返回 `TextInput`，它提供行数、提示、密码和提交配置，也能使用通用元素修饰器。一个编辑器可直接作为列表的一行，也可以传给 `field` 和 `popup_list`。文本内部的富文本链接由对应文字范围拥有。输入框的前缀、图片标题和悬浮按钮放在外层容器。[编辑器组合回归](../../crates/octara/tests/integration/collection_rows.rs#L12)

`ELEMENT_CONTENT` 表示操作与内容种类或所有权不符。先找出收到配置的实际节点，尤其注意组件返回的是包裹容器还是控件。富文本范围还须有序、互不重叠，并落在有效 UTF-8 边界。[内容约束测试](../../crates/octara/tests/integration/element_content.rs)

## 语义关系

用 `.label(..)` 为控件直接提供名称。已有可访问的标签时，用 `.labelled_by(label_id)` 取该节点的直接名称或文字。标签不可达、名称有歧义或文字为空时，使用控件自己的名称。一个 `labelled_by` 读取一个标签的内容。这里与 Exact 的直接命名来源一致，关系解析只走一层。用 `.controls(panel_id)` 表示控件与面板的关系，选择状态和显隐仍由应用绑定。[Exact 的命名实现](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Accessibility.swift#L74-L89) · [W3C 标签页模式](https://www.w3.org/WAI/ARIA/apg/patterns/tabs/)

这两个关系与 `active_descendant` 共用窗口的唯一名称查找。AccessKit 与 Inspector 都只发布指向当前语义树中的目标。挂载、隐藏、改名和重建可以改变关系。标签文本的变化还会更新引用它的控件名称。Rune 的 `active_descendant` 也从当前树解析目标，并把目标身份纳入访问树更新。[Rune 的关系解析](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/node.rs#L239-L246) · [Rune 的语义更新](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/node.rs#L54-L64) · [增量语义测试](../../crates/octara/src/kernel/a11y.rs)

Kit 的 `field` 用可见标签为实际控件命名。[`field` 实现](../../crates/octara-kit/src/forms.rs)

`select`、`combobox` 和 `tabs` 接收同一种 `Choice`。应用用稳定 ID 标识选项，可更新标签、顺序和禁用状态。`tabs` 的每个标签页指向 `{id}-panel-{option.id}`。应用可用 `.labelled_by` 从对应标签页为面板命名，并按需隐藏或卸载面板。[Choice](../../crates/octara-kit/src/choices.rs) · [Tabs](../../crates/octara-kit/src/navigation.rs) · [完整组合](../../examples/src/showcase/interaction.rs)

## 4 Style

主题提供语义颜色和控件共用的值。使用自定义 Token 时，在初始化的主题中注册值，并核对实际使用的主题。`MISSING_TOKEN` 会提示缺失的名称。对焦点、按下和禁用分别设置可感知的外观；把键盘焦点留在支持相应动作的节点上。[主题测试](../../crates/octara/tests/integration/theme.rs)

## 5 Overlays

锚点由触发控件的几何位置给出。弹层还要决定可用空间、焦点进入与关闭后的恢复。`OVERLAY_CLAMPED` 表示锚点两侧都容纳不下内容，窗口已调整弹层位置。应检查文本放大、较小窗口和锚点临近边缘时的实际可操作区域。[弹层测试](../../crates/octara/tests/integration/overlay.rs)

在 `select` 和 `combobox` 中，当前活动项由列表或独立搜索框报告，已选项来自应用模型。提交时读取当前可用项，先关闭弹层，再调用应用 setter。选择回调可以关闭窗口，框架随后按窗口和作用域结算焦点恢复。[选择器实现](../../crates/octara-kit/src/choices.rs) · [选择器回归](../../crates/octara-kit/tests/integration/option_snapshots.rs)
