# 窗口的一次更新

应用拥有反应式运行时和可共享的资源。窗口拥有自己的保留树、布局、选择和焦点。同一份应用数据可以驱动两个窗口，每个窗口分别计算自己的可见结果。公共值的包与窗口运行时的边界见[架构](../ARCHITECTURE.md)。

## Incremental frames

一次输入可产生多次状态写入。受影响的绑定把新值送到对应节点，需要测量的内容进入布局，绘制器收到该窗口的绘制数据。稳定节点上的一次文本变化应当能保留无关节点和它们的动作目标。可以从输入回放、节点更新和绘制结果三个位置检查。[窗口更新](../../crates/octara/src/app/frame.rs) · [增量帧测试](../../crates/octara/tests/integration/incremental.rs) · [输入回放](../../crates/octara/tests/integration/replay.rs)

窗口先准备布局、焦点、悬停和滚动等结果。App 刷新各窗口引起的反应式反馈，才复核每个准备结果是否仍有效，并交给宿主。一个窗口的测量更新了另一个窗口时，受影响的窗口会再次准备。未收敛的窗口等待之后的操作恢复，宿主在此期间保留已交付的画面和辅助功能状态。等待网络或定时器的任务与窗口准备各有自己的进度。 [跨窗口更新回归](../../crates/octara/tests/integration/windows.rs) · [布局恢复回归](../../crates/octara/tests/integration/size_of.rs)

排查画面与命中不一致时，先固定窗口大小、比例、滚动位置和帧，再比较节点的逻辑边界、屏幕上的像素和事件实际接收者。原生视图与系统候选窗要分别取证。[布局](layout.md) · [捕获](capture.md)

## 窗口结算与实际呈现

App 结算输入、任务、焦点与布局，再向宿主交付有序的原生操作及重绘需求。原生视图的创建、更新和销毁，文本输入会话及系统请求都随这次交付生效。需要与像素同步的位置由宿主放入对应的呈现事务。一次交付成功后，窗口可以发布新的绘制修订。交付、绘制修订和实际呈现有各自的完成时刻。[窗口结算](../../crates/octara/src/app/frame.rs) · [宿主接口](../../crates/octara/src/platform.rs)

每个窗口按需组装最新的已结算绘制。画面固定修订号、App 采样时刻、逻辑尺寸、比例、资源版本和自定义绘制的来源。绘制数据可以交给渲染线程，连续画面共享未变化的局部绘制段。宿主在 drawable 可用时读取最新画面。被合并的中间版本仍然已完成 App 结算。回执使用对应画面记录的绘制来源，即使节点已经替换或窗口已经关闭。后续 Inspector 操作仍按当前的挂载身份校验。[绘制发布](../../crates/octara/src/app/frame.rs) · [绘制证据](../../crates/octara/src/capture.rs)

宿主按窗口管理在途获取和成功提交。等待期间 App 可继续结算，其他窗口可处理自己的呈现机会。缩放、失效和关闭可令旧获取的结果过期。已开始的获取仍持有其位置，直到返回并释放资源。提交回执必须对应当前窗口、表面和设备，后来的兼容提交才能推进观察结果。实屏截图先等待匹配的提交，再在采样前后检查相同的画面与表面。[呈现协调](../../crates/octara-shell/src/presentation.rs) · [截图状态](../../crates/octara-shell/src/capture.rs)

## 找到负责的层

| 要查的问题 | 入口 |
| --- | --- |
| 写状态、任务退休、回调发生 panic | [反应式依赖](reactive-core.md)和[生命周期](runtime-contract.md) |
| 列表重排、行数据或目标过期 | [行的身份与数据](collection-regions.md)和[集合变换](map-join.md) |
| 控件怎样组装、按键发给谁、文本如何测量 | [内容与布局](ui-primitives.md)、[输入](input.md)和[文本](text.md) |
| 目标值怎样成为当前画面、退出节点何时释放 | [动画架构](motion.md) |
| 应用与系统怎样交换事件、窗口、像素 | [winit 职责边界](../ARCHITECTURE.md#平台宿主与-winit-的边界)、[平台协议](platform.md)、[窗口](windows.md)和[原生嵌入](native-island.md) |
| 怎样辨别 Agent 看到的运行进程 | [构建收据](agent-context.md)和[语义检查](agent-knowledge.md) |
