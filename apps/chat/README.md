# 运行 Chat

在仓库根目录运行 `cargo run -p octara-chat-desktop`。Chat 使用 Octara v2 的原生窗口、编辑器和组件。可以创建并管理会话、流式阅读回复、停止或重新生成答案、调整模型和输出设置。

## 选择后端

| 启动环境 | 应用行为 |
| --- | --- |
| 无模型服务配置 | 使用离线 Scripted 后端，可验证界面、队列和取消 |
| 设置 `OPENROUTER_API_KEY` | 使用应用内置的 OpenRouter 预设 `deepseek/deepseek-v4.1-flash` |
| 设置 `OCTARA_CHAT_ENDPOINT` 和 `OCTARA_CHAT_MODEL` | 连接指定的完整 Chat Completions 地址与模型。`OCTARA_CHAT_API_KEY` 是可选的专用凭据，显式配置优先 |
| 设置 `OCTARA_CHAT_DIR` | 将会话和设置保存在指定目录，便于隔离测试 |
| 设置 `OCTARA_CHAT_TIMEOUT_SECS` | 配置连接或读取空闲超时，取值 1 至 600 秒，默认 60 秒 |

会话保存在 `<thread>.v2.json` 中。用编辑区的附件选择器添加文件，或将文件拖入编辑区。附件以本地文件引用保存，发送给模型的是对话文本。

模型选择以稳定 ID 保存。应用在发送或排队时记录模型和生成参数。流式输出途中切换模型只影响之后创建的请求。停止或重新生成会取消旧请求，迟到的增量和结束事件无法修改当前回复。取消设置对话框会丢弃草稿，保存才更新配置。

使用 Scripted 模型时，在输入框键入 **/transit**，选择命令并发送预填的问题。回复包含可展开的分析笔记、引用、图片、公式、亮度曲线和 Rust 代码。点击图片可放大。键入 **@** 可插入可整块删除的示例联系人，**/attach** 打开文件选择器。

## 确定性检查

CLI 使用相同的界面、离线模型和固定时钟，并将会话保存在内存中。可用 `--size` 和 `--theme` 覆盖检查窗口的尺寸和主题。

```sh
cargo run -p octara-cli -- check --package octara-chat
cargo run -p octara-cli -- inspect --package octara-chat --size 390x844
cargo test -p octara-chat
```

[HTTP 测试](tests/network.rs)使用环回服务覆盖真实 SSE、取消和超时。[界面测试](tests/chat.rs)通过 Inspect v2 验证编辑器和对话框。[长会话测试](tests/long_session.rs)覆盖虚拟列表和流式输出时的位置保持。

## 实窗与本地 HTTP

通过 CLI 启动带 Inspector 的离线 Chat。下列命令适用于 Unix。

```sh
chat_tmp="$(mktemp -d /tmp/octara-chat.XXXXXX)"
cargo run -p octara-cli -- launch --package octara-chat-desktop --socket "$chat_tmp/inspect.sock" --receipt "$chat_tmp/build.json"
```

在另一个终端，使用前一个终端的临时目录运行脚本，或查看语义树。关闭窗口后可删除临时目录。

```sh
python3 -B apps/chat/live_chat.py --connect /tmp/octara-chat.XXXXXX/inspect.sock
cargo run -p octara-cli -- inspect --connect /tmp/octara-chat.XXXXXX/inspect.sock
```

需要连接本地 HTTP 时，先运行隔离的 [SSE 测试服务](serve_network_fixture.py)，再以它输出的完整地址和独立会话目录启动 Chat。测试服务不需要凭据。

```sh
python3 -B apps/chat/serve_network_fixture.py --output /tmp/octara-chat-peer
```

```sh
OCTARA_CHAT_ENDPOINT="$(cat /tmp/octara-chat-peer/endpoint.txt)" \
OCTARA_CHAT_MODEL=local-fixture OCTARA_CHAT_DIR="$chat_tmp/session" cargo run -p octara-chat-desktop
```

在界面中发送消息，确认收到标题为 “Local HTTP stream” 的回复。关闭窗口后结束测试服务。

## 长会话观测

使用 `cargo run -p octara-chat-desktop --example long_session` 打开长会话窗口。通过 `OCTARA_CHAT_DIR` 指定空目录以生成带有 1,000 条消息的历史。将 `OCTARA_CHAT_MESSAGES` 设为 `10000` 可检查较大的历史。在这个窗口中发送消息会开始一段较长的离线回复。

```sh
cargo run --release -p octara-chat --example long_bench -- --samples 40
cargo run --release -p octara-chat --example stream_bench -- --samples 20
```

基准输出记录进程内的流式接收和提交耗时、事务数量、帧编号和已挂载消息数量。
