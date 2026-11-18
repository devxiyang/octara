# 集合变换

对于大目录，可以先按文件属性筛选、排序，再把负责人等关联数据投影到行。为集合提供准确的行变更，可以避免把整个目录的更新当作所有行都发生变化。

## View closures

增量 `filter`、`sort_by`、`group_by`、`map`、`join` 的行变换由集合事件驱动。在这些闭包里额外读取 Signal 会触发 `VIEW_READS_STATE`；后续该 Signal 改变时，行变换缺少重算的触发源。将所需值放进行数据、用关联集合连接，或选用会追踪这些依赖的快照来源。[集合实现](../../crates/octara/src/reactive/list.rs)

## Diagnostics

`JOIN_FANOUT` 表明修改一条关联记录引发了大量行更新。先确认哪些可见属性真正引用变化的值，再缩小投影或调整关联位置。请用实际可见行与变化行数量衡量收益。[一致性测试](../../crates/octara/tests/integration/list_consistency.rs)
