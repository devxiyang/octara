# 确认正在观察的构建

## Verify the running build

同时开着几个同名窗口时，先确定运行进程。`octara launch` 记录构建收据。以下示例需在仓库根目录执行，启动命令在窗口关闭前持续运行。选择未使用的绝对 socket 和收据路径。

```sh
cargo run -q -p octara-cli -- launch -p octara-examples --bin window --socket /tmp/octara-receipt.sock --receipt /tmp/octara-receipt.json -- counter
```

从另一个终端读取：

```sh
cargo run -q -p octara-cli -- inspect --connect /tmp/octara-receipt.sock --receipt /tmp/octara-receipt.json --json
```

`run --require-build` 在发送脚本操作之前核对收据。`BUILD_MISMATCH` 时先核对目标进程和启动路径；改过源码后用新收据启动新构建。构建匹配只证明运行版本，控件是否仍是同一次观察中的对象由操作的 `target` 另行判断。[语义检查](agent-knowledge.md) · [启动实现](../../crates/octara-cli/src/launch.rs)
