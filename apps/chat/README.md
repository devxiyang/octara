# 运行 Chat

在仓库根目录运行 `cargo run -p octara-chat`。可以创建会话、流式阅读回复、停止或重新生成、调整模型和输出设置。Chat 是消费框架的一款桌面应用；通用原语的验收场景见[框架架构](../../docs/ARCHITECTURE.md)。

## 选择后端

| 启动环境 | 应用行为 |
| --- | --- |
| 无模型服务配置 | 使用离线 Scripted 后端，可验证界面、队列和取消 |
| 设置 `OPENROUTER_API_KEY` | 使用应用内置的 OpenRouter 预设 `deepseek/deepseek-v4.1-flash` |
| 设置 `OCTARA_CHAT_ENDPOINT` 和 `OCTARA_CHAT_MODEL` | 连接指定的完整 Chat Completions 地址与模型。`OCTARA_CHAT_API_KEY` 是可选的专用凭据，显式配置优先 |
| 设置 `OCTARA_CHAT_DIR` | 将会话与保存的设置放在指定目录，便于隔离测试 |
| 设置 `OCTARA_CHAT_TIMEOUT_SECS` | 配置连接或读取空闲超时，取值 1 至 600 秒，默认 60 秒 |

新会话存储写入 `<thread>.v2.json`。较早的 `<thread>.json` 文件仍留在原处，当前版本只加载新格式。启动配置见[启动测试](src/startup.rs)，网络事件见[网络测试](tests/network.rs)。

## 请求何时确定

模型以稳定 ID 保存。用户按发送或将消息加入队列时，应用捕获模型及当时生效的设置。流式输出途中切换模型只影响之后创建的请求。停止或重新生成会退休旧请求；迟到的增量、错误和结束事件不能再写入新回复或释放新队列。取消设置对话框会丢弃草稿，保存才更新配置。[模型身份测试](tests/model_identity.rs) · [生命周期测试](tests/lifecycle.rs)

## 离线与本地 HTTP 验证

无窗口的 `octara check`、`octara inspect` 与 `octara run` 使用桌面程序的界面、窗口配置和浅色主题。会话保存在内存中，使用离线 Scripted 后端和固定的会话时间。检查矩阵与命令行选项可以覆盖窗口尺寸和主题。

```sh
cargo test -p octara-chat --test lifecycle --test network --test model_picker
cargo test -p octara-chat --test long_session --test responsiveness
```

需要对照真实窗口与 HTTP 服务时，先将本地测试服务指向一个全新目录。它运行在前台，并把完整地址写入目录中的 `endpoint.txt`。

```sh
python3 apps/chat/serve_network_fixture.py --output /tmp/octara-chat-peer
```

另开终端，使用该地址和独立的数据目录。测试服务不需要凭据。

```sh
OCTARA_CHAT_ENDPOINT="$(cat /tmp/octara-chat-peer/endpoint.txt)" OCTARA_CHAT_MODEL=local-fixture OCTARA_CHAT_DIR=/tmp/octara-chat-session cargo run -p octara-chat
```

窗口验证脚本各有专用 fixture 和 `--help`。请求停止与队列见[生命周期验证](live_lifecycle.py)，请求使用的实际模型见[模型选择验证](live_model_selection.py)，设置持久化见[设置验证](live_settings.py)，长段落、滚动位置和窗口缩放见[长会话观测](observe_long_session.py)。性能记录应保留输入途径、样本量和窗口尺寸。
