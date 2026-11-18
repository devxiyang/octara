# Octara

Octara 是一个用 Rust 编写原生界面的框架。视图由普通函数组合，Signals 驱动属性更新，窗口保存可跨帧复用的节点。应用可以在无窗口环境中运行交互测试，也可以让 Agent 连接正在运行的桌面窗口，读取控件语义并验证操作结果。

## 从哪里开始

| 如果你想 | 从这里读起 |
| --- | --- |
| 写一个可运行、可测试的界面 | [入门指南](docs/start-here.md) |
| 判断一个能力应该放在内核、宿主还是组件库 | [架构设计](docs/ARCHITECTURE.md) |
| 看桌面和移动交互如何推导出原语 | [交互时序](docs/design/interaction-scenarios.md) |
| 区分已有实现、设计目标和待验收项 | [路线图](docs/ROADMAP.md)与[工作清单](docs/TODO.md) |
| 运行桌面示例或配置流式模型服务 | [示例与验证](examples/README.md)或 [Chat 使用说明](apps/chat/README.md) |

## 运行

在仓库根目录使用 stable Rust，工作区最低版本为 1.100。第一条命令打开桌面窗口，第二条查询当前源码里的 API。

```sh
cargo run -p octara-examples --bin window -- counter
cargo run -q -p octara-cli -- vocab state
```

桌面宿主使用 winit，macOS 另有系统文本输入、菜单和原生视图适配。移动交互用于推导设计，iOS 和 Android 宿主仍待实现。各层依赖由仓库脚本检查。[桌面宿主](crates/octara-shell/src/) · [分层检查](tools/check_layers.py)

## 本地构建

重复验证时，使用同一套工具链、编译配置和 Cargo 缓存路径。更换 `CARGO_TARGET_DIR` 时，也要把新的二进制路径交给默认从 `target/debug` 读取的窗口验证脚本。磁盘空间紧张时，可用 `du -sh target` 看占用，再用 `cargo clean --release --dry-run` 预览清理范围。每次验证生成的截图、日志和 socket 有各自的生命周期。[仓库约定](AGENTS.md) · [Cargo 构建缓存](https://doc.rust-lang.org/cargo/reference/build-cache.html)

项目按 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 许可发布。
