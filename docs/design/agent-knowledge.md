# 通过 Agent 检查界面

## Inspector protocol and checks

在 macOS 或 Linux 连接桌面 Inspector 后，先读语义树中的窗口、角色、名称、状态、边界和动作。使用窗口编号选定窗口，再用不透明的 `target` 操作观察到的节点，例如 `{"op":"tap","target":"<observed target>"}`。同时提供 `id` 时，窗口会核对该实例的名称。只提供 `id` 的脚本要求名称在该窗口的挂载树内唯一，隐藏但保留挂载的节点也参与匹配。窗口操作使用窗口目标。无 `id` 的 `type` 和 `key` 把节点目标用作当前焦点的断言。需要向其他观察到的字段输入时，先用 `focus` 操作该字段。

可见标签和它命名的输入框可能有同一个名称。结合 `actions` 的 `input` 选择单行或多行编辑器。实时 `run --json` 从标准输入读取时，每一行的 `ok` 与 `delivery` 报告该步结果，退出码报告整段会话。带 `--script` 的运行会在首个失败步骤停止。动作已送达后，从控件值、校验说明或状态节点确认应用接受了什么。[资料审阅实测](interaction-scenarios.md#资料审阅队列)

屏外控件需要先滚动。在 `actions` 含 `scroll` 的视口上发送 `{"op":"scroll","target":"<viewport target>","dx":0,"dy":120}`。位移以逻辑像素计，正值增大偏移。回执中 `scroll.offset` 是当前偏移 `[x,y]`，`scroll.delta` 是本次实际变化，包含布局校正。到达边界后继续向外滚动可返回 `[0,0]`。滚动后重新读取树，再操作进入视口的控件。[待办实窗验收](../../examples/live_inspect.py)

`octara.inspect/2` 的 `selection` 提供文档选区预览。`preview` 从当前帧已挂载的选区按文档顺序读取，最多 2,000 个 Unicode 标量值。`complete` 为 `false` 表示预览截断或选区包含未挂载行。选区全部位于未挂载行时，`preview` 为空且 `complete` 为 `false`。显式 `copy` 请求按复制规则提取选区，可以临时挂载屏外行。旧版 `octara.inspect/1` 的 `selection` 是字符串。[预览与复制测试](../../crates/octara/tests/integration/selection.rs)

节点上的 `labelled_by`、`controls` 和 `active_descendant` 都是当前语义树中目标的 `id`。关系未能解析时，该字段省略。根据关系找到实际节点后，操作时使用它的 `target`。控件的 `name` 已反映当前可用的标签。[关系与名称](ui-primitives.md#语义关系)

模态打开时，Inspector 仍会报告已显示的背景节点、位置和 `target`。这些节点的 `actions`、局部 `commands` 与关系会省略，使用旧快照操作仍挂载的背景节点会收到当前模态范围的拒绝。关闭模态后，同一挂载实例恢复可操作，期间重建的实例按原有规则获得新 `target`。[模态范围](ui-primitives.md#5-overlays)

| 返回结果 | 下一步 |
| --- | --- |
| `AMBIGUOUS_TARGET` | 查找名匹配了多个挂载实例。用树中的角色、标签和位置确定对象，再传入该对象的 `target` |
| `STALE_TARGET` 或 `NO_SUCH_ELEMENT` | 重新观察，确定控件是否已退出、替换或换焦点。对同名新控件重新作出操作决定 |
| `STEP_FAILED`、`EXPECTATION_FAILED` 或 `BAD_REQUEST` | 比对实际值、支持的动作和请求字段。若已送达部分输入，先核对效果，避免重复执行 |
| `NOT_SETTLED` | 窗口更新未结算时，`inspect`、`expect`、`pick` 和坐标手势暂不可用。用 `issues` 查看原因，必要时执行已知的恢复命令。遇到安静等待超时时检查剩余任务和动画 |
| `UNNAMED_CONTROL` 或 `UNLABELED_CANVAS` | 给交互对象或有含义的画布提供名称；装饰内容可在语义层隐藏 |
| `SMALL_TARGET` 或 `LOW_CONTRAST` | 核对实际命中区域及前景与背景颜色，在窄窗、触摸条件和不同主题中再次检查 |

无窗口速测可检查状态与顺序。真实窗口 Inspector 的语义或框架注入可检查窗口内发生的效果。要验证 OS 路由、输入法面板或屏幕最终成像，还需要相应平台的证据。[构建收据](agent-context.md) · [截图报告](capture.md) · [可运行验证](../../examples/README.md)
