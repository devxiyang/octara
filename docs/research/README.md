# 架构决策参考

保留能解释设计取舍的来源。下面的“采用”是 Octara 的设计判断，平台覆盖以仓库自己的实现与验证为准。

| 来源 | 可复核的机制 | 对 Octara 的取舍 |
| --- | --- | --- |
| Rune | 普通 Rust 函数组装元素。保留树、区域缓存、平台帧顺序和 GPU 绘序插槽各有条件 | 保持写法直接，由已知的变化范围选择布局和绘制工作。让作者扩展使用正常的绘序与生命周期。[Rune 架构与源码](rune-architecture.md) |
| Exact | 编译器、执行器、宿主、数据权限和构建回执围绕可验证的应用程序协作 | 用权限、版本和回执确定哪些窗口工作可独立推进，并将真实运行证据交给 Agent。[Exact 完整架构](exact-full-architecture.md) · [运行时与时序专项](exact-architecture.md) |
| SwiftUI | 视图描述、身份、生命周期与依赖有不同职责 | 用“重排后仍是哪个对象”推导状态归属，再考虑 API 拼写。[Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/) |
| Headless UI React | 把行为和语义附着到可替换的控件部件，复合控件共享集合、焦点与弹层机制 | 让 Kit 配置实际交互节点，Core 统一身份、输入路由和跨节点协调。[完整导出与源码调研](headless-ui-react.md) |
| W3C ARIA APG | 复合控件内部的键盘导航、键盘焦点与选中状态有各自约定 | 让应用持有业务选择，窗口持有焦点，组件决定两者是否同步。[键盘交互指南](https://www.w3.org/WAI/ARIA/apg/practices/keyboard-interface/) |
| Jetpack Compose | 嵌套滚动按阶段分配可消费位移，fling 也有速度分配 | 先用输入与实际消费验证交接，再决定 Octara 是否需要公开每个阶段。[嵌套滚动](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers) |
| Elliott 与 Hudak | 将随时间变化的值与离散事件分开讨论 | 区分显示中的进度和提交或取消的时刻。可用显式状态和事件时序表达，无需采用论文的整套运行时。[Functional Reactive Animation，ICFP 1997](https://conal.net/papers/icfp97/) |

原语要通过独立产品交互的反例检验。桌面重排和移动拖动各给出一个可以复算或观察失败的样本。[交互时序](../design/v2/06-inspect-dst-release-migration.md)

Exact 的动作在开始时读取状态，写入集中生效。Rune 的处理器可以直接修改 Rust 状态。Octara 在 Rust 读写语义上明确布局反馈与窗口发布的顺序。[目标架构](../design/architecture-v2.md)
