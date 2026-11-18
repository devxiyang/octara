# Signals、身份与结构

Signal 可以保存结构体、向量或其他 Rust 数据。用 `with` 读取需要的字段，用 `update` 成组修改；多个控件都需要的派生值可以用 `memo` 表达。独立更新的字段适合独立 Signal，需要一起提交的字段适合保存在同一状态中。[可执行示例](../start-here.md)

## 2 Layers

挂载时执行视图构造函数。属性接收 Signal 或读取 Signal 的闭包时建立依赖。构造函数内的一次 `get` 只取快照，意外的静态读取会报告 `STRICT_READ_UNTRACKED`。用 `octara::reactive::untrack` 显式采样，构造、初始化和派生计算按各自的权限执行。请在输入处理、watch 回调或任务中写状态。

`with` 的共享借用与 `update` 的独占借用持续到同步闭包返回。借用期间对同一值做冲突的访问会产生 `SIGNAL_BORROW`。先提取所需值，再调用可能重入的代码。

## 3 Graph and scheduling

将派生值表达为单向计算。`CYCLE` 表示依赖成环；`FLUSH_RUNAWAY` 应从不断写回自身依赖的回调查起。比较结果或丢弃旧值时，框架还需要保持执行权限和依赖收集的边界。[图实现与测试](../../crates/octara/src/reactive/graph/)

## 4 Identity and state

命名状态属于当前 App 和所有者。父级内的稳定结构 key 标识同一段内容；兄弟顺序、显示文字和 UI 函数名都可能重复。同一 owner 内冲突的状态名或 scope key 应消除。热重载时，类型变化也可能导致状态重建。跨 App 传数据，任务回调在所有者关闭后放弃旧句柄。[结构身份测试](../../crates/octara/tests/integration/structural_identity.rs)

## 5 Incremental collections

集合按稳定、唯一的业务 key 维护行。列表需要每个工厂产生一个根元素，以便整体插入、移动与退出。重复 key 会触发诊断；非法快照之后以有效快照恢复时，须重新观察已退休的行。虚拟化行的挂载寿命可以短于业务对象。[行读取](collection-regions.md) · [集合变换](map-join.md)
