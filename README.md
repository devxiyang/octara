# Octara 0.0.6

用 Rust 编写原生与 Web 界面。用显式能力上下文构造视图和更新状态，原生宿主用 GPU 呈现，Web 宿主投影真实 DOM。用同一套目标与编辑会话协议测试交互、观察帧进度和重放故障。

- 从 [入门指南](docs/start-here.md) 开始。
- 使用 `octara-kit` 的主题控件，或组合 `octara_kit::headless` 的行为部件和 `octara_kit::parts` 的样式。
- 使用 `octara-markdown` 渲染流式 AI 文档，定制思考过程、代码和图表等块级组件。启用 `math` 支持原生及 Web 矢量公式，启用 `composer` 为输入框添加光标锚定补全、原子提及和 Markdown 序列化。
- 用 `octara audit` 检查文本裁剪、交互控件的无障碍名称和失效的焦点目标。
- 了解 [crate 边界与 Turn](docs/ARCHITECTURE.md) 或查阅 [v2 架构规范](docs/design/architecture-v2.md)。
- 运行 [示例](examples/README.md) 和 [流式 Chat 应用](apps/chat/README.md)。

在工作区根目录使用 Rust 1.100 或更高版本。

```sh
cargo run -p octara-examples --bin window -- counter
cargo run -p octara-examples --bin window -- showcase workspace
cargo run -q -p octara-cli -- vocab BuildCx
cargo run -q -p octara-cli -- audit --manifest-path examples/Cargo.toml --json
```

运行工作区测试、依赖边界、Markdown 特性测试和 Wasm 检查。

```sh
python3 tools/check_layers.py
OCTARA_VERIFY_RELEASE_API=1 cargo test --workspace
cargo test -p octara-cli --doc
cargo test -p octara-markdown --all-features
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target wasm32-unknown-unknown -p octara-markdown
cargo check --target wasm32-unknown-unknown -p octara-markdown --all-features
cargo check --target wasm32-unknown-unknown -p octara-host-web
cargo check --target wasm32-unknown-unknown -p octara-host-web --features webgpu
```

在装有 iOS Simulator SDK 和对应 Rust 目标的 macOS 上检查原生宿主。

```sh
cargo check --target aarch64-apple-ios-sim -p octara-host-native
```

在工作区统一预检 19 个 `0.0.6` crate 的归档。Cargo 会解析同批内部依赖并从归档解包编译。

```sh
cargo publish --workspace --dry-run
```

复用工作区 Cargo 缓存。结束验证后清理临时捕获、socket 和测试进程。[仓库约定](AGENTS.md)

项目按 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 许可发布。
