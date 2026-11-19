# 在 macOS 验证中文输入法

这个用例检验真实的原生编辑会话。需要可见并获焦的 macOS 桌面窗口、ABC 和简体拼音输入源。使用一次性文字。准备脚本会生成应用包和 manifest，打印窗口与 socket 位置。

```sh
cargo build -p octara-examples --bin ime -p octara-cli --bin octara
python3 examples/ime_window.py
```

打开生成的应用包，记录当前输入源。单行字段的初值是 `A😀Z`，多行字段和密码字段分别用于验证换行及敏感内容。用系统键盘完成以下操作，在每次转折后通过 Inspector 只读采样。[准备脚本](ime_window.py) · [观察脚本](ime_observe.py)

| 操作 | 要核对的事实 |
| --- | --- |
| 在单行框内选择 emoji | 同一选区的 UTF-8 字节范围是 `[1,5]`，UTF-16 范围是 `[1,3]` |
| 切到拼音，输入音节，移动组合光标，再按 Escape | 草稿和候选变化，业务值保持原值；取消恢复原选区 |
| 选中一个候选 | 只产生一次已提交编辑；随后执行撤销可还原文本与选区 |
| 在 ABC 下插入 `nihao`，选择这五个字母，调用系统输入法菜单的“显示替换候选字” | 打开候选时原文还在。选择后只替换该范围。候选排序随输入法词库变化，记录实际文字 |
| 调整窗口大小和位置，在另一位置重新开始组词 | 用同一屏幕坐标系比较光标和候选窗，记录移动前后的两个位置 |
| 切到多行或密码字段 | 上一字段的组合不能泄漏。分别观察输入法确认键、普通 Return、Command+Return。密码字段的 Inspector 快照应省略 `text_input` |

一次只读观察的命令如下。将 socket 换成 manifest 的值，每次使用新的证据名。

```sh
python3 examples/ime_observe.py --socket /absolute/path/ime.sock --output /tmp/octara-ime-result --name selection --expect '{"focus":"ime-single","text_input":{"selection":[1,5],"selection_utf16":[1,3]}}'
```

应用自己的绘制捕获可以验证组合下划线、选区和光标。系统候选窗另取屏幕证据。已识别候选窗的实际拥有进程后，可用 `swift examples/ime_candidates.swift --pid PID --output NEW_DIRECTORY` 只读采样；窗口归属取决于系统与输入法。将采样与同一时刻的应用快照配对，比较前先统一屏幕坐标轴方向。结束后恢复原输入源。[候选窗采样](ime_candidates.swift) · [原生文本客户端](../crates/octara-shell/src/text_input_mac.rs)

历史 macOS 复验曾观察到 `A😀nihaoZ` 中仅 `nihao` 被替换成 `👋`，撤销恢复原选区。原记录的一步把清空后的 composition 误预期为 JSON null，实际为一个空字符串。复验以当前快照和实际候选文字为准。

## 文件重命名

同一个验收应用也能运行行内重命名场景。A 和 B 位于不同目录，初始名称都是 `A😀Z`。目录操作只更新内存中的测试记录。

```sh
cargo build -p octara-examples --bin ime -p octara-cli --bin octara
python3 examples/ime_window.py --scene rename
```

打开 manifest 指向的应用包。点击 `Select A and B`、`Rename B`。用系统键盘反向选中 emoji，切到已配置的简体拼音并输入 `nihao`。在每一步读取 `rename-editor` 的目标令牌、`text_input`、`rename-counts` 和 `rename-records`。`live.ime.screen_anchor` 是上一次系统几何查询的回答，候选窗的实际位置从屏幕采样读取。[原生文本桥](../crates/octara-shell/src/text_input_mac.rs) · [系统候选窗采样](ime_candidates.swift)

将 manifest 的 socket 传给 `target/debug/octara run --connect SOCKET --json`。以下 `invoke` 命令只提供可重复的目录事件。用 OS 键盘确认候选、提交和取消。

| 目录事件 | 核对内容 |
| --- | --- |
| `rename.reorder`、`rename.refresh` | B 移到首行。编辑器目标、选区方向和组合保持。分别记录新的光标位置和候选窗实测坐标 |
| `rename.scroll-away`、`rename.scroll-back` | 聚焦行离开视口再返回时保留编辑会话 |
| `rename.external` | B 在组词期间从外部改名。确认候选后按 Return，提交报告冲突并保留草稿 |
| `rename.replace` | 在 B 组词时用同名 C 替换它。旧候选窗关闭，旧 `target` 被拒绝，C 能开始自己的拼音会话 |
| `rename.reset` | 恢复初始目录、编辑状态和计数，便于重复用例 |

候选确认先产生一次草稿编辑。下一次 Return 才提交名称。用两次 Escape 分别验证取消组合与取消重命名。当前实窗记录和候选位置差异见[桌面重命名验收](../docs/design/interaction-scenarios.md#macos-实窗记录)。

### 同窗 AppKit 对照

先在 B 的行内编辑器里输入 `nihao`，保持系统候选窗开启，记录 `rename.reorder` 前后的插入点和候选窗位置。再重排一次，把 B 放回原位。

向同一 socket 发送 `{"op":"invoke","command":"rename.native-control"}`。验收程序结束 Octara 编辑会话，在原编辑器位置放置标准 `NSTextView`，选中 `A😀Z` 中的 emoji。保持简体拼音，在新控件中输入 `nihao`。查看 manifest 的 `native_control` 指向的报告。有效对照要求 `native_focus`、`input_context_current`、`has_marked` 都为 `true`，且屏幕上有实际候选窗。

记录候选窗后发送 `{"op":"invoke","command":"rename.move-native-control"}`。该命令在简体拼音正在组词时将同一个视图向上移 44 点。比较 `movement.first_rect_before`、`movement.first_rect_after`、标记范围、选区及候选窗的实际位置。AppKit 屏幕坐标向上为正，候选采样坐标向下为正。结束后关闭验收窗口。[同窗验收实现](src/bin/ime/native_control.rs)

## 受控文本值

构建后运行 `python3 examples/ime_window.py --scene controlled`，打开生成的应用包。第一框初值为 `A😀Z`，第二框为 `B😀Y`。第一框反向选中 emoji，用系统简体拼音输入 `nihao`。应用值的变化经下列 Inspector `invoke` 命令触发。

| 操作 | 核对内容 |
| --- | --- |
| 组词中执行 `ime-control.refresh` | 绑定再次读取同一个值。记录刷新前后候选窗、反向选区和光标 |
| 接着执行 `ime-control.replace` | 应用值变为 `R😀S`，组合结束，候选窗消失 |
| 执行 `ime-control.reset` 和 `ime-control.reject`，重新选中 emoji 组词并确认「你好」 | 应用收到提议 `A你好Z`。已接受值和反向选区恢复为输入前状态 |
| 再次组词并执行 `ime-control.capture`，用指针切换到第二框，执行 `ime-control.deliver-retired` | 系统关闭旧候选窗。夹具用实测候选词和保存的旧文档在平台交付入口重放提交，结果为 `Refused`。第二框可以独立组词并提交 |

用 manifest 的 socket 读取 `text_input` 和 `live.ime`。候选窗采样可指定 `--field ime-controlled` 或 `--field ime-controlled-other`，配对查看 AX 字段边界、选区和范围查询结果。选区不可读时采样器也会查询首字符的范围，记录实际状态码。结束后关闭验收窗口。 [验收实现](src/bin/ime/controlled.rs) · [macOS 实测记录](../docs/design/input.md#受控文本框实测)
