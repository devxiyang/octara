# 用函数组装一个控件

考虑一个带图标、文件名和状态标记的文件行。应用用普通 Rust 函数组装容器、文字和图像，把“打开文件”动作和可访问名称挂在承担交互的对象上。改变图标或换行属于外观调整；打开哪个文件由业务 key 决定。只有需要独立状态寿命时，才为这一段内容建立所有者。[当前构造器](../../crates/octara/src/view/) · [结构身份](reactive-core.md#4-identity-and-state)

## Element content

当前构造器在创建节点时确定内容类型。容器可用 `.child(..)` 和 `.children(..)` 挂载应用子节点。图片使用 `image(..)` 配合 `.object_fit(..)`；编辑设置用于 `text_input()`。文本内部的富文本链接由对应文字范围拥有。输入框的前缀、图片标题和悬浮按钮可放入外层容器。

`ELEMENT_CONTENT` 表示操作与内容种类或所有权不符。先找出收到配置的实际节点，尤其注意组件返回的是包裹容器还是控件。富文本范围还须有序、互不重叠，并落在有效 UTF-8 边界。[内容约束测试](../../crates/octara/tests/integration/element_content.rs)

## 4 Style

主题提供语义颜色和控件共用的值。使用自定义 Token 时，在初始化的主题中注册值，并核对实际使用的主题。`MISSING_TOKEN` 会提示缺失的名称。对焦点、按下和禁用分别设置可感知的外观；把键盘焦点留在支持相应动作的节点上。[主题测试](../../crates/octara/tests/integration/theme.rs)

## 5 Overlays

锚点由触发控件的几何位置给出。弹层还要决定可用空间、焦点进入与关闭后的恢复。`OVERLAY_CLAMPED` 表示锚点两侧都容纳不下内容，窗口已调整弹层位置。应检查文本放大、较小窗口和锚点临近边缘时的实际可操作区域。[弹层测试](../../crates/octara/tests/integration/overlay.rs)
