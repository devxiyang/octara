# 示例与窗口验证

从仓库根目录构建并运行。`octara-examples` 提供示例应用、无窗口测试和 CPU 工具。`octara-examples-desktop` 提供桌面与 iOS 共用的原生启动器及 GPU 验证。

| 功能 | 启动命令 |
| --- | --- |
| 计数器与可恢复状态 | `cargo run -p octara-examples-desktop --bin window -- counter` |
| 稳定键、筛选、排序与分组 | `cargo run -p octara-examples-desktop --bin window -- todo` 或 `contacts` |
| 聊天回复、虚拟行与编辑器 | `cargo run -p octara-examples-desktop --bin window -- chat_virtual` |
| Kit 表单、组合框与弹层 | `cargo run -p octara-examples-desktop --bin window -- showcase forms` 或 `choices`、`interaction`、`controls` |
| 发布工作台，含 Navigator、树视图、数据表、拖放排序、命令面板、右键菜单与 Toast | `cargo run -p octara-examples-desktop --bin window -- showcase workspace` |
| 实时 Markdown、可选富文本、画布、地图与原生 Web 视图 | `cargo run -p octara-examples-desktop --bin window -- showcase markdown` 或 `text`、`drawing`、`map`、`web` |
| 多窗口与各窗口独立的撤销历史 | `cargo run -p octara-examples-desktop --bin window -- windows` 或 `undo` |
| 虚拟行尺寸与离屏编辑器 | `cargo run -p octara-examples-desktop --bin virtual_geometry` 或 `--bin virtual_interaction` |
| 系统输入法与标记文本 | `cargo run -p octara-examples-desktop --bin ime`。[IME 操作步骤](IME.md) |
| 交互式 GPU 材质、流场与折射 | `cargo run -p octara-examples-desktop --bin window -- gpu_lab` |
| CPU 参考截图 | `cargo run -p octara-examples --bin screenshot -- /tmp/octara-shots 2 --bundled` |
| CPU 投影基准 | `cargo run --release -p octara-examples --bin bench` |
| GPU 绘制与缓存基准 | `cargo run --release -p octara-examples-desktop --bin gpu_bench -- gpu` 或 `-- gpu-retain` |

CPU 截图默认使用内置字体。使用系统字体时，添加 Cargo 参数 `--features system-fonts` 并省略 `--bundled`。

运行 `cargo test -p octara-examples` 验证无窗口应用行为和 CPU 渲染。运行 `cargo test -p octara-examples-desktop` 验证 GPU 帧规划的性能预算、着色器回退和富文本 CPU/GPU 像素一致性。可设置 `OCTARA_REQUIRE_GPU=1` 要求像素一致性测试使用可用的 GPU 适配器。

`gpu_lab replay 60` 与 `gpu_lab replay 120 stress` 通过应用时钟持续更新着色器。使用 `gpu_lab input-pressure` 时，点击 **Start workload**，随后用系统键盘、点击与拖动操作控件。设置 `OCTARA_GPU_LAB_TRANSITION=1` 会在玻璃面板中显示局部 GPU 过渡。

发布工作台可按平台筛选构建，点击数据表的表头可排序。打开审阅页并编辑备注，使用后退和前进按钮可检验 Navigator 保留的草稿。在审阅队列上拖动，或按空格、上／下方向键和 Enter 调整顺序。**Notify on move** 是用 Headless Checkbox 自定义样式的复选框。右击 **Queue actions** 或在焦点位于该按钮时按 Shift+F10，可重置队列顺序。

Markdown 示例使用本地月食对话。点击 **Replay answer** 可查看流式渐变和引用标签更新。展开 **Observation plan** 查看计划，点击段内缩略图或独立图片打开预览。输入 **@Luna** 可插入联系人，**/replay** 重播回复，**/question** 填入示例问题。用方向键和 Enter 选择补全。

## 检查真实窗口

```sh
cargo build -p octara-examples-desktop --bin window -p octara-cli --bin octara
python3 -B examples/live_inspect.py --window target/debug/window
```

脚本在交互式桌面打开计数器，读取实际窗口回执，用守卫目标点击按钮，并验证计数变化、新帧呈现和 PNG 捕获。退出时清理进程、套接字和捕获。

手动场景用 `OCTARA_INSPECT` 指定尚未使用的 Unix 套接字。另一个终端用 `target/debug/octara inspect --connect "$socket" --json` 获取回执。动作请求引用 `snapshot.nodes` 中的 `target`。应用和窗口重建会产生新目标。[协议操作与回执](../docs/design/v2/06-inspect-dst-release-migration.md)

```sh
socket=$(mktemp -d /tmp/octara-example.XXXXXX)/inspect.sock
OCTARA_INSPECT="$socket" target/debug/window counter
```

触控板样本使用 `trackpad`。快速推动内层视口并抬手，观察直接滚动和惯性，再用 **Reset** 重建视口。打开 `OCTARA_TRACE_SCROLL=1` 可收集宿主的滚动记录。

## iOS 模拟器

已启动 Apple Silicon iPhone 模拟器时，可构建并安装与桌面共用的示例。

```sh
IPHONEOS_DEPLOYMENT_TARGET=17.0 cargo build --locked --target aarch64-apple-ios-sim -p octara-examples-desktop --bin panel_scroll
sim_run=$(mktemp -d /tmp/octara-ios.XXXXXX)
python3 -B examples/ios_panel.py --device "$device" \
  --binary target/aarch64-apple-ios-sim/debug/panel_scroll --output "$sim_run/run"
```

输出的 `run.json` 包含本次进程的构建回执、套接字和日志路径。运行 `target/debug/octara inspect --connect SOCKET --receipt RECEIPT --json` 校验并读取窗口。用系统触摸拖动列表，可从 `pan-list` 的高度和 `scroll_offset` 观察面板扩张和列表滚动。也可构建 `album_back` 并使用 `--scene album` 检查返回手势，或用 `--scene interaction`、`--scene review`、`--scene ime` 检验对应窗口。

结束时用 `xcrun simctl terminate DEVICE BUNDLE` 终止样本，再用 `xcrun simctl uninstall DEVICE BUNDLE` 卸载后清理运行目录。设备和包名在 `run.json` 中。

## 浏览器输入

浏览器样本演示组合输入、UTF-8 选区、键盘拦截、焦点与视口尺寸。用与 Rust 依赖版本匹配的 `wasm-bindgen` 生成浏览器入口。

```sh
cargo build --locked --target wasm32-unknown-unknown -p octara-host-web --example web_input
web_run=$(mktemp -d /tmp/octara-host-web.XXXXXX)
wasm-bindgen --target web --out-dir "$web_run" target/wasm32-unknown-unknown/debug/examples/web_input.wasm
cp crates/octara-host-web/examples/web_input.html "$web_run/"
python3 -m http.server --bind 127.0.0.1 --directory "$web_run" 0
```

从命令输出的本地端口打开 `web_input.html`，查看页面上的断言结果。添加 Cargo 参数 `--features webgpu` 并打开 `web_input.html?webgpu` 可验证浏览器的 GPU 画布。完成后关闭服务器并清理临时目录。[Web 输入示例](../crates/octara-host-web/examples/web_input.rs) · [DOM 更新示例](../crates/octara-host-web/examples/dom_transactions.rs)

## 本地签名发布样本

`signed_review` 是 macOS 原生样本。用 `cargo build -p octara-examples-desktop --bin signed_review` 构建，并用 `OCTARA_REVIEW_PUBLIC_KEY` 绑定发布公钥。应用可用 `signed_review STORE SIGNED_PACKAGE` 暂存包，下次启动后在首帧确认。编辑草稿保存在 `STORE/draft.json`。使用 `package_signed_review.py` 组装并验证本地应用包。[签名发布流程](../docs/design/v2/06-inspect-dst-release-migration.md)
