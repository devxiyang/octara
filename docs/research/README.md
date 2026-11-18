# 架构决策参考

保留能解释设计取舍的来源。下面的“采用”是 Octara 的设计判断，平台覆盖以仓库自己的实现与验证为准。

| 来源 | 可复核的机制 | 对 Octara 的取舍 |
| --- | --- | --- |
| Rune | 普通 Rust 函数组装元素，把角色、状态和事件接到交互对象；同一应用可以无窗口驱动 | 用函数组合和可观察语义检验 API 易用性。更新方式继续由 Octara 的 Signals 承担。[Rune 示例，固定版本](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/README.md) |
| Exact | 同一应用描述在 Apple 平台使用 UIKit 或 AppKit 视图与控件 | 给系统编辑与原生对象明确的宿主边界。语言和 VM 可独立选择。[Exact 的宿主说明，固定版本](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/README.md) |
| SwiftUI | 视图描述、身份、生命周期与依赖有不同职责 | 用“重排后仍是哪个对象”推导状态归属，再考虑 API 拼写。[Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/) |
| W3C ARIA APG | 复合控件内部的键盘导航、键盘焦点与选中状态有各自约定 | 让应用持有业务选择，窗口持有焦点，组件决定两者是否同步。[键盘交互指南](https://www.w3.org/WAI/ARIA/apg/practices/keyboard-interface/) |
| Jetpack Compose | 嵌套滚动按阶段分配可消费位移，fling 也有速度分配 | 先用输入与实际消费验证交接，再决定 Octara 是否需要公开每个阶段。[嵌套滚动](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers) |
| Elliott 与 Hudak | 将随时间变化的值与离散事件分开讨论 | 区分显示中的进度和提交或取消的时刻。可用显式状态和事件时序表达，无需采用论文的整套运行时。[Functional Reactive Animation，ICFP 1997](https://conal.net/papers/icfp97/) |

原语要通过独立产品交互的反例检验。桌面重排和移动拖动各给出一个可以复算或观察失败的样本。[交互时序](../design/interaction-scenarios.md)
