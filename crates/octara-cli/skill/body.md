# 用 Octara 构建和验证应用

先读随此版本发布的[入门指南](references/start-here.md)。`octara vocab` 列出默认导入，`octara vocab <name>` 从应用实际依赖的源码给出签名、导入路径和定义。`octara explain <code>` 给出诊断原因。`octara skill install` 还会生成按模块组织的 `references/api/index.md`。

## 组合与归属

- 用普通 Rust 函数构造视图。`#[ui]` 让 Inspector 记录构造位置。容器挂载子视图，字段、图片和文字通过各自的构造器配置。标签与动作放在实际可操作的节点上。
- `state` 表达所有者拥有的值，`memo` 推导值，Signal 或闭包把属性绑定到来源。构造过程读取会采样一次。显式快照从 `octara::reactive` 导入 `untrack`。在处理器或任务内修改状态。
- 为独立寿命建立 `scope`。兄弟结构 key 须稳定且唯一。可变化的列表用业务 key，集合变换从 `items.read_only()` 开始。文字标签、列表下标和 Inspector 查找名各有不同用途。
- 受控输入的回调接收提议；先读取模型接受的值再保存。组件内部回调用 `behavior` 安装，让应用回调仍可追加。任务和外部订阅绑定到使用它们的 owner，共享任务可放入 `App::run`。
- 平台约定、字体、主题在打开窗口前配置。移动宿主、原生输入法和屏幕呈现的可用范围按实际平台验证。

## 给 Agent 一手反馈

1. 先用 `Window::headless` 驱动同一视图，断言可观察的值、焦点、可用动作、诊断和布局。`octara check` 可检查不同大小与主题。需要单个接口的操作名和参数时查 `octara vocab octara::inspect::Request`。
2. 在 macOS 或 Linux 上通过 `OCTARA_INSPECT` 指定未占用的绝对 socket 路径，或用 `octara launch -p my-app --bin my-app --socket SOCKET --receipt FILE` 启动桌面程序。`octara inspect --connect SOCKET --receipt FILE --json` 可核对运行中的构建。
3. 从当前语义树或 pick 结果读取不透明 `target`，在操作中原样传回。`octara run --connect SOCKET --require-build FILE --script STEPS --json` 按顺序执行 JSONL。例如 `{"op":"tap","id":"save","target":"<observed target>"}`。树重建、控件替换或换窗后重新观察。
4. 记录动作后的状态、截图报告和实际输入途径。`STALE_TARGET` 或连接中断后要先检查送达情况，防止重复执行。原生候选窗、系统快捷键和系统手势用 OS 输入单独验证。
