# 用 Octara 0.0.7 构建和验证应用

读这个版本的[入门指南](references/start-here.md)。`octara vocab` 列出 prelude，`octara vocab <name>` 显示应用所用源码中的签名、导入路径和定义，`octara explain <code>` 说明诊断原因。`octara skill install` 生成按模块组织的 [API 索引](references/api/index.md)。

## 编写视图和状态

- 用普通 Rust 函数组合视图，通过 `AppDefinition` 和 `WindowDefinition` 指定窗口入口。单行编辑用 `input()`，多行编辑用 `textarea()`。把语义角色、标签和处理器放在实际操作的节点上。
- 用短小的 `#[ui]` 函数命名有明确用途的组件，便于在检查结果中追踪来源。将静态样式写成 `const STYLE: TwStyle = tw!(...)`，再用 `.tw(STYLE)` 应用。用 `.child((a, b, c))` 添加一组固定子节点，元组可混合不同的 `IntoView` 类型。用 `.children(iter)` 添加同类型的子节点。
- `BuildCx` 创建 Signal、Memo、集合和任务所需的状态。`ReadCx` 用于动态读取，`WriteCx` 用于处理器中的修改。构造时用 `cx.read(|r| …)` 做一次非订阅读取。受控控件用可复制的 `Binding<T>`，从 Signal 转换或用 `cx.binding(read, write)` 定义读写。
- 用 `cx.persisted(key, init)` 保存需要跨进程恢复的应用状态，并声明状态 schema。为变动列表选择稳定唯一的业务 key。用 `StateList::read_only()` 开始增量派生，或用 `cx.derive_list` 从普通响应式值构造 keyed list，再用 `each` 按 key 挂载行。
- 用 `cx.menus` 声明窗口菜单，`cx.on_chord` 注册快捷键。菜单项和快捷键可以调用同一命令。用 `El::select_none()` 关闭界面文字的文本选择。
- 用 `cx.window_chrome()` 布置自定义标题栏，用 `cx.window_geometry()` 订阅实际窗口位置和尺寸。由 `WindowOptions::persistence_key` 将窗口几何纳入 `RestartState`。用 `window_drag_region(true)` 标记标题栏的拖拽区。
- 用 `native_view("web")` 在 macOS 或 iOS 托管 `WKWebView`。用 `WriteCx::send_native_command` 按 `test_id` 发消息，通过 `on_native` 接收导航和 JavaScript 响应。macOS 使用 `Underlay` 时，Octara 浮层可覆盖 WebView。
- 数字样式值表示逻辑像素，`sp(2.0)` 使用主题间距。用 `FontSizeToken`、`ColorToken` 和 `Radius` 指定主题或形状。
- 用 `octara_kit::button(label, size, variant)` 指定按钮尺寸和强调。组合自定义控件时，从 `octara_kit::parts` 获取主题样式，从 `octara_kit::headless` 获取行为。用 `build.provide(KitLocale::zh_cn())` 设置作用域内 Kit 控件的中文标签。
- 选择器用 `headless::Choice` 提供选项，用 `headless::Selection` 声明单选或多选。通过 `PickerOptions` 和行工厂配置列表，通过 `PopupOptions` 配置浮层。`ComboboxOptions` 组合列表、浮层和输入框配置。
- 用 `markdown_with(build, source, MarkdownConfig::chat())` 定制 Markdown 的解析、排版、链接和代码块。向作用域提供 `Arc<dyn MarkdownRenderer>`，可让多个 `markdown(build, source)` 共享渲染器。`TextBuffer` 支持增量追加，块缓存保留节点与选区。
- 用 `markdown_stream` 呈现模型输出。渐显默认遵循系统的减弱动态效果设置，`MarkdownConfig::streaming_effects_with` 可响应式配置揭示效果。通过 `BlockSlotRenderer` 定制代码或图表，用 `LinkPillContent` 响应式更新链接胶囊的标签和图标。
- 用 `InlinePill::attachment` 把胶囊嵌入富文本。宿主根据真实排版尺寸安放胶囊，选择和复制时使用附件范围内的逻辑文本。
- 为 `octara-markdown` 启用 `math`，以矢量方式排版 `$…$` 与 `$$…$$` 并保留可复制的 TeX 源码。启用 `composer` 后，用 `MarkdownComposer` 配置光标锚定补全。为 `Completion` 添加图标、描述和分类，或用 `editor_with_completion_row` 提供自定义行。

## 验证应用

在同一工作区中用 `my-app` 库公开 `startup() -> AppDefinition`，在库的 `[dev-dependencies]` 中添加 `octara-inspect`。将原生宿主依赖放在同级的 `my-app-desktop` 包中，其 `src/main.rs` 调用 `octara_host_native::run_app(my_app::startup())`。

```toml
[dependencies]
my-app = { path = "../my-app" }
octara-host-native = "0.0.7"
```

用 `cargo test -p my-app` 单独运行应用库的测试。`HeadlessDriver::launch(startup())` 启动应用定义，使用布局管线和 CPU 绘制路径。用 `cargo tree -p my-app` 检查测试依赖里是否有 `octara-host-native`、`octara-gpu`、`wgpu` 或 `objc2`。[Cargo tree 参考](https://doc.rust-lang.org/cargo/commands/cargo-tree.html)

为待测节点设置稳定的 `test_id`，用 `Observe` 取得目标，再发送 `Act` 并断言更新后的状态。

1. 在 Cargo 的 `[package.metadata.octara]` 中登记 `entry = "my_app::startup"`。该函数返回 `AppDefinition`。`octara check -p my-app` 检查手机和桌面尺寸、亮色和暗色下的诊断与结算。`octara inspect -p my-app --json` 启动无头宿主并观察窗口。`octara audit -p my-app --json` 检查文本裁剪、无障碍名称和焦点目标。按回执里的 `provenance.file` 和 `provenance.line` 定位问题。错误级问题退出码为 1。
2. 用 `octara launch -p my-app --socket SOCKET --receipt FILE` 在本机启动同一个入口。选择未占用的绝对 Unix socket 路径和新的回执文件。`octara inspect --connect SOCKET --receipt FILE --json` 校验进程和构建身份。
3. 用 `octara run` 在同一次会话中观察并操作，或通过 `--connect SOCKET` 操作已启动的宿主。每行写一个 `octara.inspect/2` 请求。先发送 `{"protocol":"octara.inspect/2","op":"list_windows"}` 或 `{"protocol":"octara.inspect/2","op":"observe"}` 查找窗口和节点。每次 `act` 都原样传回当前快照的 `target`；从节点的几何读取坐标，确认窗口和交互目标。通过 `octara vocab AgentRequest` 和 `octara vocab AgentAction` 查参数。
4. 读取每步的 `delivery`、`progress.settled`、`committed_frame` 和 `presented_frame`。在下一条 `observe` 中核对值和焦点。收到 `STALE_TARGET` 或连接中断时先观察，再决定是否重试。用 `capture` 的 `painted` 来源核对绘制像素，用实际宿主分别验证系统输入法和呈现行为。

## 配置开发与测试构建

在工作区根 `Cargo.toml` 的 `[profile.dev.package."*"]` 和 `[profile.test.package."*"]` 中设置 `opt-level = 2`。下游应用用到的 `octara`、`octara-*` 及 `wgpu`、`parley`、`taffy` 等外部依赖自动使用该设置，工作区成员使用默认的 `opt-level = 0`。[Cargo profile 参考](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)

依赖优化可降低 debug 排版和渲染的 CPU 开销，收益随工作负载而变。在 macOS arm64 上对 `6ea4f49` 的 27 个 Chat 和 examples 测试可执行文件进行同口径比较时，外部依赖从 `opt-level = 1` 调到 `2`，`__eh_frame` 与 `__unwind_info` 的合计体积减少约 6.7%。

CLI 将根清单的 `[profile.*]` 表原样复制到生成程序的独立工作区。应用库及其依赖在这里都是工作区外依赖，通配覆盖也会命中参与构建的原工作区成员。具名包覆盖优先于通配覆盖，例如用 `[profile.dev.package.my-app]` 将应用库设为 `opt-level = 0`。[Cargo profile 参考](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)
