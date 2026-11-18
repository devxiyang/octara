# 通过 Agent 检查界面

## Inspector protocol and checks

在 macOS 或 Linux 连接桌面 Inspector 后，先读语义树中的窗口、角色、名称、状态、边界和动作，再选择操作对象。`.id(..)` 提供查找名；不透明 `target` 标识实际观察到的对象。节点操作使用节点目标，窗口操作使用窗口目标。焦点发生变化后，重新确认接收键盘输入的对象。

| 返回结果 | 下一步 |
| --- | --- |
| `STALE_TARGET` 或 `NO_SUCH_ELEMENT` | 重新观察，确定控件是否已退出、替换或换焦点。对同名新控件重新作出操作决定 |
| `STEP_FAILED`、`EXPECTATION_FAILED` 或 `BAD_REQUEST` | 比对实际值、支持的动作和请求字段。若已送达部分输入，先核对效果，避免重复执行 |
| `NOT_SETTLED` | 检查仍在运行的任务、定时器和动画，再决定事件应该推进到哪个时刻 |
| `UNNAMED_CONTROL` 或 `UNLABELED_CANVAS` | 给交互对象或有含义的画布提供名称；装饰内容可在语义层隐藏 |
| `SMALL_TARGET` 或 `LOW_CONTRAST` | 核对实际命中区域及前景与背景颜色，在窄窗、触摸条件和不同主题中再次检查 |

无窗口速测可检查状态与顺序。真实窗口 Inspector 的语义或框架注入可检查窗口内发生的效果。要验证 OS 路由、输入法面板或屏幕最终成像，还需要相应平台的证据。[构建收据](agent-context.md) · [截图报告](capture.md) · [可运行验证](../../examples/README.md)
