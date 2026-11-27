# 在原生窗口验证输入法

构建 IME 样本和 CLI。

```sh
cargo build -p octara-examples --bin ime -p octara-cli --bin octara
python3 -B examples/ime_window.py
```

打开输出的 `bundle`，点击 `Single line`。输入包含 emoji 的文本，切换至已启用的拼音输入法，再输入拼音。用候选窗口提交后切到多行编辑器，确认标记文本归属。比较单行、多行和安全输入框时，先确认各自获得焦点。安全字段回执仅公开安全属性和会话标识。

每个步骤可把焦点编辑器的会话、文本修订、选区和组合范围保存为 JSON。每次用独立文件名。

```sh
target/debug/octara inspect --connect "$socket" --json
python3 -B examples/ime_observe.py --socket "$socket" --output composing.json
```

`snapshot.editor` 的选区按 UTF-8 字节计数。点击 **Remount single-line field** 后重新聚焦，比较新旧会话与目标。应用快照用于核对框架文本。目标进程的候选窗可用 `swift examples/ime_candidates.swift --pid PID --output NEW_DIRECTORY` 另行采样。[macOS 文本客户端](../crates/octara-host-native/src/os/macos/editor.rs)

行内文件重命名用 `python3 -B examples/ime_window.py --scene rename`。选择 **Rename B** 后，用系统输入法替换文件名。组合期间可发送 `rename.reorder` 或 `rename.refresh`，通过 `snapshot.editor` 查看同一会话是否保留。用 `rename.external` 模拟同时写入，提交后从 `rename-status` 验证冲突处理。

在 iOS 用 `--scene ime` 安装同一个样本，再用模拟器系统键盘输入。[iOS 运行方式](README.md#ios-模拟器) · [iOS 文本客户端](../crates/octara-host-native/src/os/ios/editor.rs)
