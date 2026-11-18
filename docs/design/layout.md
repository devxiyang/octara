# 布局与窗口几何

同一份文档在大屏窄分栏里可比手机更窄。布局以父级约束、可用窗口空间、文本测量和缩放比例为输入。页面可按可用空间切换侧栏或导航，输入方式与窗口大小分别考虑。

## 3 Viewport and window size classes

布局尺寸反馈可影响下一轮布局。若测到的宽度改变自身约束并反复切换，`LAYOUT_LOOP` 会提示。能用父级约束或布局规则表达时，直接使用它们；确实需要反馈时，应为稳定尺寸给出固定结果。[响应式布局测试](../../crates/octara/tests/integration/responsive.rs)

## 4 Layout diagnostics

`OVERFLOW` 通常需要沿父子约束查找无法收缩的内容。`COLLAPSED` 表示有内容的元素宽或高为零，应先检查父级是否有可分配空间。`TEXT_TRUNCATED` 表示截断或行数限制已经生效，按产品意图检查全文是否仍可获取。[布局诊断测试](../../crates/octara/tests/integration/layout_issues.rs)

## 5 Scrolling and overlays

`virtual_each` 的直接父级需要是有界高度的纵向滚动容器，否则报告 `VIRTUAL_LIST_NOT_SCROLLABLE`。滚动改变可见范围，弹层、光标、命中和无障碍边界也要使用相应位置。嵌套消费和手势中断的目标协议见[移动面板时序](interaction-scenarios.md)。[滚动用例](../../crates/octara/tests/integration/scroll.rs)
