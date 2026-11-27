# Octara v2 实现导览

[架构总纲](design/architecture-v2.md)和分层规范定义 crate 边界、数据结构、宿主接口与验收门禁。`python3 tools/check_layers.py` 检查 21 个工作区成员及其全部生产边，包括可选、构建和平台条件依赖。

| 层 | 实现 |
| --- | --- |
| 值和协议 | `octara-model` 持有 identity、style IR、绘制值和输入协议。`octara-icons` 提供矢量路径，`octara-svg` 按需解析 SVG。`octara-release` 独立验证签名与发布序号。 |
| 树 | `octara-tree` 持有紧凑节点、稀疏侧表、逻辑编辑器与交互仲裁。 |
| 运行时与视图 | `octara-runtime` 持有类型分区存储、响应式依赖图、任务和 Cycle。`octara` 提供视图和窗口原语。`octara-headless` 把行为和语义附加到调用方提供的元素上。`octara-kit` 组合 Headless 部件并应用主题、尺寸和布局预设。 |
| 文档与编辑 | `octara-markdown` 在公开 authoring API 之上构建流式文档、可替换的块级槽、矢量公式和富文本输入框。 |
| 原生 | `octara-pipeline` 运行 Taffy、Parley、动画、子树绘制和增量 AccessKit 投影。`octara-render` 计划与编码绘制。`octara-gpu` 管理后台队列和原生回读。`octara-host-native` 通过独立窗口能力连接输入、原生视图、系统服务和呈现。 |
| Web | `octara-host-web` 分阶段根据 TreeDelta 修补 DOM，通过撤销日志恢复失败投影，通过批量数值桥按需读取布局。已提交的退出元素由浏览器执行透明度动画。WebGPU 可按需启用。 |
| 验证与交付 | `octara-inspect` 提供 guarded actions、回放、跨窗口确定性模拟和传输。`octara-cli` 提供源码词汇、技能和发布命令。 |

## 一个 Cycle

1. Ingress 核对目标和编辑器会话。交互仲裁器在同一规则下处理 Native、Web 和 Inspector 的输入。
2. React 运行事件与就绪任务，按拓扑依次结算派生值、结构、绑定和 watch 来源。视图把静态和动态属性写入当前窗口的 `TxBuilder`。
3. Commit 通过 `Tree::apply(Transaction)` 预检拓扑、版本和内容，再更新节点和索引。临时交互状态通过 `Tree::apply_runtime(RuntimeOp)` 修改。两条路径都发出 `TreeDelta`。
4. Project 将 delta 交给具体 `HostEngine`，在最多 16 轮内结算布局和反馈。
5. Publish 发布帧与观察回执。投影失败时，宿主回滚其暂存状态。运行时保留等待重新投影的 delta。

按业务键定位列表行和文本编辑会话。派生值与列表都在同一张图里按需追赶。原生排版与浏览器测量共享逻辑坐标协议。可以在多个测试线程里分别创建 App，指定虚拟时钟和随机种子重放输入。

[响应式运行时](design/v2/02-reactive-runtime.md) · [树与输入仲裁](design/v2/03-tree-microkernel.md) · [原生投影](design/v2/04-native-pipeline-render-host.md) · [Web 投影](design/v2/05-web-host.md)

## 窗口与原生视图

运行时在 Ingress 接收每扇窗口的 `ChromeChanged` 和 `GeometryChanged`，更新构建中可订阅的信号。窗口控制命令携带 `WindowId`，由对应宿主执行。`persistence_key` 把几何记录接入 `RestartState`。拖拽标题栏沿用树的交互优先级，以按钮、输入和链接的实际目标为准。

macOS 用 AppKit 布置原生标题栏和系统材质。macOS 与 iOS 的 WebView 使用同一套原生节点命令和事件。macOS 上的 Metal Underlay 在原生视图位置留出透明区，展现 Octara 浮层的区域仍由 Octara 绘制并接收输入。[窗口、富文本和原生 WebView 规范](design/v2/08-native-chrome-rich-text-webview-windows.md)

## 组件行为与外观

Headless 控制器用可复制的句柄组合状态、绑定和集合。例如，Listbox 与 Combobox 共用按键导航、业务键、活动行和选区规则。窗口处理实际焦点、输入法、退出屏障和拖放仲裁。Kit 在这些部件上应用主题和布局，并通过 `octara_kit::headless` 导出同一套行为接口供应用调用。[应用原语与组件规范](design/v2/07-app-primitives-and-headless-kit.md)

## 发布

发布包用 Ed25519 签名，序号保护已接收的发布状态。`octara-release::Store` 先验证安装目标和签名，再调用应用的准备函数。准备函数可用 `octara::declarative::ScenePacket` 解码 `octara.scene/2`，把调色板、布局和由应用解析的命令编译成 `Transaction`，交给树预检。应用持有启动 lease，在首帧达到确认条件后调用 `Session::confirm`。`App::export_persisted` 和 `App::import_persisted` 负责明确声明的持久状态。

[协议、模拟测试和发布规范](design/v2/06-inspect-dst-release-migration.md)
