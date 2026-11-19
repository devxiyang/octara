# 示例与窗口验证

在仓库根目录执行命令。`window` 可以打开单项场景，也可以用 `showcase` 打开组件展示。完整参数以[示例入口](src/bin/window.rs)为准。

| 想观察的行为 | 启动参数，跟在 `--bin window --` 后面 |
| --- | --- |
| 点击、绑定与状态更新 | `counter` |
| 按稳定 key 更新行、筛选与排序 | `todo`、`contacts` |
| 焦点、字段说明和单选项 | `showcase forms` |
| 弹层里的可搜索选项、键盘导航 | `showcase choices` |
| 对话框、分段选择与数值控件 | `showcase interaction`、`showcase controls` |
| 多个窗口的创建和关闭 | `windows` |
| [同标题窗口的撤销归属](../docs/design/interaction-scenarios.md#两个同标题文档的撤销) | `undo` |

## 框架输入到真实窗口

```sh
cargo build -p octara-examples --bin window -p octara-cli --bin octara
OCTARA_INSPECT=/tmp/octara-example.sock target/debug/window counter
```

窗口打开后，在另一个终端运行 `target/debug/octara inspect --connect /tmp/octara-example.sock --json`。取出按钮的实际 `id` 和 `target`，再运行 `target/debug/octara run --connect /tmp/octara-example.sock --json`，每行输入一个 JSON 操作。点击后再次 inspect，比对文字、焦点和收到动作的对象。使用未被占用的 socket 路径。

## 重现专项问题

| 问题 | 如何重现 |
| --- | --- |
| 单击、双击、长按、窗口尺寸与绘制结果 | 构建上面的两个二进制后，运行 `python3 examples/live_inspect.py`。[脚本](live_inspect.py) |
| 重复名称、隐藏控件、同名重建、热重载和重开窗口 | 额外构建 `cargo build -p octara-examples --bin targets`。用独立 `OCTARA_INSPECT` 路径启动 `target/debug/targets`，再运行 `python3 examples/live_targets.py --socket /tmp/octara-targets.sock --output /tmp/octara-targets-result`。填入实际 socket 和尚未创建的输出目录。[脚本](live_targets.py) |
| 字段、单选和组件键盘流程 | 运行 `python3 examples/live_forms.py` 或 `python3 examples/live_primitives.py --scene all`。[字段](live_forms.py) · [对话框与数值控件](live_primitives.py) |
| 复合界面的组装、操作与审批反馈 | 启动 `review_queue`，按语义树操作资料审阅队列。[资料审阅](#资料审阅) |
| macOS 拼音候选、重转换和屏幕位置 | 用系统键盘操作，分别记录应用快照与系统候选窗。[输入法验证](IME.md) |
| 同一次触摸联动面板与列表 | 运行 `panel_scroll` 窗口并用 Inspector 驱动固定的触摸序列。[面板联动步骤](#面板联动) |
| macOS 触控板的直接阶段、惯性与嵌套视口 | 运行 `trackpad` 并记录 AppKit 事件与框架前后的偏移。[触控板采样](#macos-触控板) |
| 大量行和布局的工作量 | 运行 `cargo run --release -p octara-examples --bin bench`。随结果记录编译配置、数据量与测量环境。[基准入口](src/bin/bench.rs) |

区分三种证据：无窗口测试控制时间，真实窗口 Inspector 通过框架事件或语义操作驱动，系统键盘和触摸覆盖 OS 路由。一个场景可以同时使用三者，在报告里标明每一步走的路径。[捕获报告](../docs/design/capture.md)

### 资料审阅

```sh
cargo build -p octara-cli --bin octara
review_run=$(mktemp -d /tmp/octara-review.XXXXXX)
target/debug/octara launch -p octara-examples --bin review_queue --socket "$review_run/review.sock" --receipt "$review_run/receipt.json"
```

从另一终端用 `octara inspect --connect SOCKET --receipt FILE --json` 读取当前窗口，再用 `octara run --connect SOCKET --require-build FILE --json` 输入逐行请求。选择「导航说明」，点击「通过」，读回校验说明。找到具有 `input` 动作的「审阅意见」，填入意见后再次提交。在「已通过」列表打开同一文档，核对保存的意见。结束后关闭窗口，清理临时目录。[实测记录](../docs/design/interaction-scenarios.md#资料审阅队列)

### 面板联动

构建验收例程后，用未占用的 socket 路径启动窗口。

```sh
cargo build -p octara-examples --bin panel_scroll -p octara-cli --bin octara
OCTARA_INSPECT=/tmp/octara-panel-scroll.sock target/debug/panel_scroll
```

在另一终端运行 `target/debug/octara run --connect /tmp/octara-panel-scroll.sock --json`。依次发送以下请求，查看两笔 `touch_scroll` 和取消后的列表位置与点击计数。

```jsonl
{"op":"invoke","command":"pan.down"}
{"op":"invoke","command":"pan.forward"}
{"op":"inspect"}
{"op":"invoke","command":"pan.reverse"}
{"op":"inspect"}
{"op":"invoke","command":"pan.cancel"}
{"op":"invoke","command":"pan.late-move"}
{"op":"invoke","command":"pan.up"}
{"op":"inspect"}
```

重开窗口后，先执行 `pan.bottom` 将偏移设为 95，再执行 `pan.down` 和 `pan.forward`，核对布局校正 `−7` 与余量 `+18`。也可在任一进行中的拖动后执行 `pan.replace`，检查同名列表的新目标与迟到位移。命令注入窗口的框架触摸事件。结束后关闭窗口并清理 socket。[无窗口断言](tests/integration/panel_scroll.rs)

### macOS 触控板

构建 `trackpad`，在单独的临时目录中保留这次运行的日志和 socket。

```sh
cargo build -p octara-examples --bin trackpad -p octara-cli --bin octara
trackpad_run=$(mktemp -d /tmp/octara-trackpad.XXXXXX)
OCTARA_TRACE_SCROLL=1 OCTARA_INSPECT="$trackpad_run/trackpad.sock" target/debug/trackpad 2>"$trackpad_run/trace.log"
```

点击 **Near inner edge**，将内层列表置于距底部 12 点的位置。把指针放在内层列表，快速朝底部滚动后抬手。读取 `octara.scroll_trace/1` 的同一窗口记录，配对查看 AppKit 的直接阶段和惯性阶段、窗口选择的 `routing_phase` 以及 `trackpad-inner`、`trackpad-outer` 的偏移。偏移为零的视口会省略。用 **Reset** 从列表顶部重复。可以在惯性开始后从 Inspector 调用 `trackpad.remove`，检查后续事件抵达时外层偏移是否为零。结束后关闭窗口并清理本次运行的临时目录。[窗口样本](src/bin/trackpad.rs) · [采样实现](../crates/octara-shell/src/scroll_mac.rs)
