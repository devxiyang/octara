# 行的身份与最新数据

文件列表可按修改时间重排。把行 key 设为文件的业务标识，文件名作为可变属性。排序前后的同一行才有机会保留局部状态；文件删除后保留的回调则需要面对该对象已经消失的情形。

## Collection freshness has one contract

行读取须反映数据源的当前值，即使处理函数尚未返回、视图传播还未跑完。行被删除或快照产生重复 key 后，旧行读取不能回退到删除前的值。`LIST_ITEM_UNAVAILABLE` 指向这种错误；若业务允许对象消失，就从模型按 key 做可选查询。数据与所挂载的节点分别检查。[行测试](../../crates/octara/tests/integration/collection_rows.rs) · [集合一致性测试](../../crates/octara/tests/integration/list_consistency.rs)
