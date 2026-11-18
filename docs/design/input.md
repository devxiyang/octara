# 输入、编辑与命令

用户可以通过鼠标、触摸、键盘和辅助技术发起同一个动作。窗口掌握当前焦点和命中对象，组件负责激活后的行为。按压预览、消费事件和最终执行动作是可分别观察的时刻。[输入路径测试](../../crates/octara/tests/integration/activation.rs)

## Edit acceptance

输入回调收到本次编辑提议。受控模型可接受、转换或拒绝提议；最终显示和保存的业务值要从模型读取。处理本次提议时又向同一编辑器注入一笔编辑会遇到 `EDIT_PENDING`。先更新模型，需要第二笔编辑时让第一笔完成。组合区、选区和已提交文本应分别检查。[编辑测试](../../crates/octara/tests/integration/edit.rs) · [系统输入法步骤](../../examples/IME.md)

## Deferred commands

弹出菜单关闭后，延迟命令在待处理更新和焦点恢复后路由。`COMMAND_UNHANDLED` 表示恢复后的焦点路径和其余处理器均未处理该动作。检查实际窗口、焦点及菜单可用性。`COMMAND_QUEUE_LIMIT` 提示命令反复入队或批次超限，先定位相互触发的动作。[菜单用例](../../crates/octara/tests/integration/menus.rs) · [双文档验收场景](interaction-scenarios.md)
