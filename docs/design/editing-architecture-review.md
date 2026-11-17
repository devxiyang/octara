# 从状态到呈现：架构复核与编辑协议重设计

状态：2026-11-17 架构评审与实施方案；**R1/R2 已实现，R3 自动化已补齐，真实 OS 验收待办**。评审基线 `59b9bb5`。
本轮按用户要求重新审查已推进的分层、内容、文本呈现、Field、语义和 placeholder；
不以已提交为保留理由，也不以测试总数代替契约完整性。真实窗口验收由用户明确延期。
本次是沿输入链路贯穿各层的复核，不宣称重新审计了 Signals、调度器或所有平台实现。

## 1. 结论

保留 Rust + Signals、保留树、向下的包依赖和现有有界更新循环。
现在的问题主要不是缺少原语，而是**编辑提议、绑定接受、历史提交、几何发布的时序和所有者不一致**。
不要引入 VM、全局事务、第二套调度器或通用的可回滚回调系统。

以一个编辑接受协议贯穿键盘、粘贴、IME、语义 Input/SetValue 和撤销/重做；
把一个编辑器的选区、组合状态和历史放回同一生命周期所有者；
把会修改滚动状态的工作移出几何读取与绘制。
已经完成的代码可以重组或删除，尤其不能保留旧入口作为兼容通道。

## 2. 已复现的问题

在基线增加临时 headless 探针，运行 `cargo test --offline -p octara c3_probe`。
三个用例均失败，日志保存在本地 `/tmp/octara-c3-probe.log`。探针已移除，未将失败或 ignored
测试加入评审提交。R1 已将以下案例恢复为常规通过测试并修复生产代码。

| 场景 | 期望 | 实际 |
| --- | --- | --- |
| 值为 `abcd`，选择字节 `1..3`，输入 `X`，on_input 不接受 | 值与选区均保持原状 | 值不变，选区变为 `2..2` |
| 值为 `ab`，在字节 1 输入 `!😀`，回调去掉 `!` | `a😀b`，光标字节 5 | 文本正确，光标字节 6 |
| 值为 `a`，末尾输入 `b`，Left，再由应用改为 `😀`，Undo | 不应用旧文档的历史 | `Step::apply` 在 emoji 字节中间切片而 panic |

共同原因：

- `app/edit.rs::apply` 在调用应用前就写 selection 和 undo，而 value 的绑定在随后 flush 落地。
- `EditState.last` 同时承担合并连续输入、记录历史适用值。移动光标将它置空，也绕过了历史新鲜度检查。
- 普通输入、Undo/Redo、AX/Agent SetValue 分别修改选区和调用回调，缺少共同的接受结果协调。
- 字节夹紧只能避免其中一次 panic，不能证明旧历史仍适用于新文档。

## 3. 从下到上的保留、重构与边界

下表记录评审基线的差距；R1/R2 的已实施结果和验证见第 9 节。

| 层 / 已完成项 | 保留 | 必须继续收敛 |
| --- | --- | --- |
| `octara-primitives` | 通用值、几何、文本事件等无运行时语义的数据 | 不下沉 Signals、编辑器生命周期或 kit 策略 |
| `octara-text` / C1b | 源范围和 glyph 同源抽取；文档可见片段与省略号；段落复用 | 完整编辑几何和裁剪文档几何保留不同用途，但复用底层 cluster/caret 事实；不得用 glyph x 反推源范围 |
| App / A1–A2 | 资源唯一写入口、权限检查、多窗口失效 | 不为输入新增资源写通道；provider/用户代码继续在内部借用之外执行 |
| Window / A3 | 有界更新、反馈与呈现调度 | 加入明确的编辑接受收尾位置；不在输入回调内递归执行整帧，也不在绘制中完成编辑 |
| View / B | 不透明 View、原子内容约束、结构写入口 | 保持作用域释放与 Kernel 借用分离；编辑协调不按 test id 找替代节点 |
| Kernel 输入 | accepted value、selection、composition 是编辑事实；原生 TextDocument 是 owned snapshot | 历史不能继续仅存在 Window 的旁路表；编辑状态与节点一起创建、释放和保密 |
| Field / C1c | 共用构建、空插入行、reveal 算法；原生当前文档查询的 try-borrow | 构建共用还不等于所有权闭合：`Kernel::field` 仍在查询/绘制时写 `scroll`、`kept`，必须分离计算与落地 |
| 语义 / C2a | node-local label/text 解析，值不充当名称；AX 与 Inspector 各自编码 | 不宣称解决复合控件名称：诊断仍有独立后代文本探测，AX 可另推导后代名称；在 D 阶段明确实际 action root 后统一 |
| placeholder / C2b | 独立输入展示属性、独立 AX/Inspector 字段；不污染输入文档 | 保留与编辑段落缓存的隔离；是否将默认弱化颜色上移 kit 与其余外观政策一起处理，不再开一套提示主题系统 |
| Shell | 原生同步文档、原生事件内解释、owner/expected/epoch 队列校验 | 消费 core 最终接受结果；回调拒绝/变换后不能重放剩余旧 native batch |
| ui-kit / App | kit 组合真实控件，应用拥有业务状态和验证规则 | 不让 kit 各自恢复选区、清 undo 或维护第二份 IME 文档 |

`tools/check_layers.py` 检查的是依赖方向，不证明这些所有权和时序已经正确。
A/B 的入口值得保留；目前没有证据要求重写 Signals 的传播机制或对象身份体系。

## 4. 最小所有权模型

### 4.1 一个编辑器，一个生命周期

将现有输入事实组织为 Kernel 内部编辑状态（名字不预设为公开 API）：

- accepted value：当前绑定已落地的值；业务 model 是应用的权威来源，Kernel 是 UI 执行所需的投影。
- selection、composition：运行时拥有的编辑状态。
- history：undo、redo、由 value 写入口维护的有效性，以及独立的连续输入合并信息。
- 单次待协调提议：仅跨越 callback→binding settlement 的临时记录；不是第二份长期 value。

将 `Step`、历史有效性和编辑结果归约移到这一私有状态内，删除 `Interaction.edits` 旁路存储。
Window 负责输入资格、调用 mounted handler、安排收尾；Kernel 不调用应用。
渲染/输入配置（multiline、rows、placeholder、secure、布局缓存）可保持独立字段，
不为了“只有一个 struct”把它们混成一个业务状态机。

安全字段仍无 undo/redo。短期提议不得引入可调试输出的明文副本；优先只存必需元数据，
必须保留的短期文本要覆盖拒绝、节点释放、关闭与 panic 的清除路径。
原生 document mirror 是同步 OS 协议所需的投影，不因此升级为业务权威值。

### 4.2 不把 accepted 与 merged 混在一起

历史是否适用，不能取决于 `last typing kind` 是否存在。
移动光标只终止输入合并；外部替换、非原样接受才建立历史边界。
每次应用历史前还要校验源片段与 UTF-8 边界，作为内部不变量验证；
校验失败应丢弃不适用历史，绝不能以夹紧范围将它应用到另一份文本。

## 5. 统一执行时间线

```mermaid
flowchart LR
    I[输入意图及 owner 校验] --> P[构建提议与编辑前选择]
    P --> H[调用 mounted on_input]
    H --> B[现有 Signals flush 落地绑定]
    B --> A[协调最终接受值和编辑元数据]
    A --> L[布局及显式 caret reveal]
    L --> F[发布一致的帧和原生文档]
```

不是所有更改都走业务 callback：应用主动 value 写入直接进入绑定落地；预编辑只修改 draft；
这些入口仍在同一编辑所有者中按其来源协调，而不是伪造 input。

1. 资格先于修改：窗口/节点/handler 所有者必须仍有效。原生队列保留 expected document 校验。
2. 提议阶段记录计算出的 selection，但不把候选历史写为成功历史。回调参数仍是提议值。
3. 回调在作用域执行权限内运行，不持有 Kernel、interaction 或 handler registry 的可变借用。
4. 复用现有 `settle_layout` 中的 flush 边界：只有传播已完成且运行时健康，才协调 accepted value；
   之后再布局/原生发布。不能用“Signal 是否通知”来判定接受与拒绝。
5. 未完成传播、运行时失败、关闭、卸载时，不发布半完成编辑，不重放已经中断的用户回调。
6. 多输入来源共享同一提议/收尾入口；Undo/Redo 在接受后才移动历史栈。
   AX/Agent 的“已投递”与业务“接受该文本”仍是不同概念，不改变现有 delivery 含义。
7. 同一编辑器未收尾时的后继操作必须有明确顺序：利用既有操作/命令边界推进，不能覆写唯一
   待处理记录或在用户回调内嵌套整帧。实施必须覆盖重入、关闭和替换目标用例。

协调是每个编辑器的确定性元数据归约，不是任意业务副作用的事务或回滚。

## 6. 建议冻结的接受与冲突规则

| 结果/来源 | 编辑状态处理 |
| --- | --- |
| accepted 等于提议 | 使用提议选区，提交对应 undo 步；按既有规则合并连续输入 |
| accepted 仍等于编辑前值，且提议不同 | 拒绝提议，恢复编辑前 anchor/caret；不插入成功历史、不清空既有 redo |
| accepted 是第三个值 | 从提议文本映射 selection 到接受文本；建立历史边界，不能留下“原提议”的 undo |
| 外部 value 同值写入 | no-op；不重置 selection、composition 或历史 |
| 外部 value 不同值写入 | 立即采用；按旧 accepted→新 accepted 映射选区，退休 composition 和不适用历史，不发 input |
| IME mark | 更新 draft 与原生查询；不触发普通 on_input，不改 accepted/history |
| IME commit/unmark | 通过统一提议/接受路径；撤销单位是整个已接受提交 |
| IME cancel | 恢复组合前接受文档与选择；不制造历史项 |
| 旧 owner/旧 expected 文档的 native 回调 | 拒绝；从最终 core 文档同步原生 mirror，退休旧 batch |

选区映射：保留共同前缀/后缀中可对应的位置；落入替换区的端点移至新区末端；
最终对齐合法 grapheme 边界，保留 anchor/caret 的方向。先定义纯函数与 Unicode 用例，
不要分别在键盘、AX、IME 各写一份。归一化策略是确定的基础映射，不宣称理解任意领域格式器。

**不引入默认 pending external value 队列。** R1 保留既有外部写入政策：
保留当前不同值外部写入立即生效的规则，先把取消与迟到输入拒绝闭合。
Octara 不把预编辑当作 on_input，因此一般格式化回调不会逐个 Pinyin 字母打断组合；
若应用主动在组合期间重置业务值，立即采用新值有明确含义，也不需要第三份待应用业务值。
以后若有真实需求要求“外部更新等待组合结束”，另行设计显式策略并做 OS 验证，不能偷偷改 setter。

同值 Signal 写入不代表强制重置。现有显式取消/选区操作与 value 写入要保持分离；
此轮不新增 reset 原语。若应用需要“同值且清空全部历史”的公开操作，须先有具体用例。

## 7. 几何与发布：重新审查 C1

评审基线中 `field()` 被 caret、命中、导航、绘制和原生发布调用，同时保存 reveal 结果，
使读取顺序参与滚动状态演进。R2 已移除这条写路径；C1c 的共用算法继续保留。

已实施边界：

- 纯计算：当前编辑事实 + 当前布局约束 + 滚动策略 → field geometry 与建议滚动结果。
- 显式落地：Window 的既有更新阶段应用 caret reveal/clamp；手动滚动与要求显示 caret 的编辑
  有清楚的原因区别。只有实际变化继续触发布局/反馈，沿用既有有界循环。
- 只读消费：paint、Inspector、AX 和普通 hit/caret 查询消费同一个已协调结果；允许内部排版缓存，
  不允许读取顺带修改编辑事实或持久滚动状态。
- 原生同步查询：仍以最新 owned TextDocument + 已发布配置计算预测几何，不能等待下一帧，
  也不能回调窗口或写 core；try-borrow 不可用时保持现有拒绝与恢复行为。
- 不用一个“万能文本快照”强行合并普通文档截断和输入完整导航。底层源片段可复用，策略明确分开。

普通文档的 C1b 可见源片段、synthetic ellipsis、隐藏链接不可操作和 resize target 保留应继续保留。
编辑 `visual_lines` 仍按 byte 整理并去重 caret stops，不能据文档 RTL 测试推断编辑 bidi affinity
已完整。将其列为明确后续验收；本轮不宣称已发现或修复所有 bidi 编辑问题。

## 8. Rune / Exact 交叉验证

本轮读取本地固定源码，不宣称已重新联网核对最新提交：

- Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3`：
  `host/apple/Sources/ExactKit/FieldComposition.swift` 明确预编辑也发送 input；
  不同外部值暂存，结束后先通知最终文本，再应用仍待处理的值。
  `Mac/FieldEditingMac.swift::applyHeldValue` 实现该顺序。
  借鉴同值无扰动、明确宿主写入与用户输入的区别、编辑结束的显式顺序；
  **不复制其预编辑回调契约和 pendingValue，也不复制旧提案的“待处理值优先”变体。**
- Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`：
  `crates/rune/src/el/text.rs::on_edit` 通过 access 返回应用中的 `&mut TextEdit`；
  `runtime/input/keys.rs::composing` 读取该编辑状态的 preedit。
  借鉴编辑事实与历史在同一个编辑对象中的生命周期；不把 Octara 改成应用持有公开 TextEdit，
  也不把它的提交周期移植进 Signals。
- 已有 [Exact 架构复核](../research/agent-exact-architecture-review.md) 与
  [Rune 架构复核](../research/agent-rune-architecture-review.md) 继续约束 kit/host 的职责。
  共同机制是所有者与提交边界清楚，而不是表面上都增加同名类或同名属性。

R2 进一步核对上述固定版本：Rune 的 `runtime/commit/text_layout.rs` 分离
`single_line_offset` 计算和 `single_line_scroll` 保存，但 `runtime/paint/helpers.rs::input_caret`
仍调用保存路径。借鉴可共用的计算，不复制绘制时写滚动。Exact 的
`FieldAtoms.swift::atomFrames` 让 TextKit 确保排版后读取 segment geometry，说明排版缓存
填充与编辑/滚动事实写入应分别看待；这不是对 Exact 全部查询路径无副作用的保证。

## 9. 实施、删除清单与完成标准

### R1：统一编辑接受与历史所有者（已实现）

- 将编辑元数据、历史适用基线和合并状态组织到输入生命周期内，删除 Window 旁路历史表。
- 统一普通输入、paste、IME commit、SetValue、Undo/Redo 的提议与接受协调。
- 删除回调前提交成功 undo、独立 SetValue 选区更新、`last=None` 代表历史永远有效等旧路径。
- 三个探针成为正式通过测试；补齐拒绝 redo、不通知 Signal 的拒绝、归一化 Unicode、
  反向选区、回调关闭/移除/重建字段、无处理器、安全输入和多输入顺序。

实现位置：`kernel/editor.rs` 负责节点拥有的提议、历史和结果协调；`app/edit.rs`
只负责输入意图、资格与回调。`settle_layout` 在既有 Signals flush 后、layout 前协调。
`Interaction.edits`、历史 fingerprint 和回调前移动历史栈的旧路径已删除。
不另存长期文档基线：不同值的 `Kernel::set_value` 在没有提议时直接清历史；
移动选区只终止合并。历史应用前仍校验源片段和 UTF-8 边界。

同一字段重入输入返回拒绝并报告 `EDIT_PENDING`，不覆盖待处理提议；原生与 preedit
更新也不得替换它。显式选区操作取消未完成提议并退休历史，以显式操作为准。
关闭、字段替换、panic 均清理临时文本；安全输入不保留 undo。
语义 `Event::Input` 和 AX SetValue 对编辑器使用同一协议；普通非编辑元素的
`on_input` 仍只是回调，不虚构编辑文档。未新增公开编辑原语或调度阶段。

验证覆盖三个原始反例、各输入入口拒绝、拒绝 undo/redo、重入不发布、
关闭/同名字段替换、回调和绑定 panic、同值与安全字段边界。
工作区 **915 passed、0 failed、3 个既有 ignored doctest**；全目标 Clippy、格式、
分层及 skill 新鲜度检查通过，CLI 可解释 `EDIT_PENDING`。真实窗口仍延期。

### R2：让几何读取不修改编辑事实（已实现）

- Field 构建与 reveal 计算保持共用；持久 scroll/keep 落地只在既有更新阶段。
- 删除 paint/caret/hit 查询中的持久滚动写入和重复独立发布计算。
- 同一已完成更新内任意顺序执行绘制、Inspector、AX、caret/hit，状态/几何一致；
  手动滚动不被观察拉回，真实编辑会按策略显示 caret。
- 保留原生当前文档预测、busy-engine 拒绝、字体变化与 resize 约束一致性。

`Kernel::layout` 在布局后执行私有 `settle_fields`；不需要 Taffy 的 caret-only 更新
仍执行收尾，但 `layout` 的返回值继续只表示 Taffy 是否运行。禁用布局的 benchmark
保持几何过期，不暗中恢复布局工作。Window 在读反馈和发布前沿用这个入口。
`InputContent.scroll/kept` 从 `Cell` 改成普通字段，只允许可变内核入口写入。
`Field::new` 读取已协调 offset，`Field::reveal` 只改变临时几何；收尾显式落地其结果。
手动滚动（包括 offset 不变的请求）记录当前 caret/文本长度，直到编辑位置改变前不自动拉回。

原生 `TextInputLayout::layout` 继续显式调用同一 reveal 算法，在自己的文档上预测几何；
不会写回 core，也没有新增持久 Field 快照或第二份编辑文档。纯查询仍允许排版缓存填充。
验证包含 pending caret 状态下交错 paint/AX/hit/caret 读取无写入、caret-only 收尾、
手动滚动保持、resize clamp、原生 busy-engine 拒绝与恢复；Window 层交叉检查 Inspector、
绘制、AX 与原生文档几何的一致性。真实窗口验收仍归 R3。
工作区 **918 passed、0 failed、3 个既有 ignored doctest**；全目标 Clippy、格式、
分层和 skill 新鲜度检查通过。Chat、HTTP、shell 和 renderer 回归均通过。

### R3：关闭行为矩阵与平台验收（自动化已补齐，OS 待验收）

- 同值 no-op、不同外部值立即覆盖、旧 draft 不复活、cancel/commit/blur 顺序都有用例。
- 与 native mirror 的 batch/epoch/owner/expected 协议交叉验证；失败后不重放输入。
- 补真实窗口中文 IME、候选位置、滚动和选择；用户不在电脑旁时明确保持待办。
- 重跑工作区、Clippy、分层和 skill；不新增 VM、通用事务或新的公开编辑器原语。

自动化矩阵（测试位于 `crates/octara/tests/integration/edit.rs`，除另注明）：

| 契约 | 覆盖用例 |
| --- | --- |
| 同值绑定重算保留 draft/选区；取消不清 redo | `same_value_binding_refresh_preserves_composition_and_cancel_preserves_redo` |
| 不同外部值立即取代 draft；旧 expected 不得提交 | `queued_native_edits_reject_a_changed_owner_or_asynchronously_changed_value` |
| 拒绝/归一化 Insert 和 Unmark 后，不接受旧 native continuation；新文档可继续 | `native_batch_continuations_require_the_accepted_document` |
| blur、cancel→blur、commit→blur 的文档与撤销边界 | `composition_terminal_order_and_blur_preserve_only_accepted_history` |
| 回调中显式选择优先于提议，取消的提议不产生历史 | `explicit_selection_in_an_input_callback_takes_precedence_over_its_proposal` |
| 热重载保留 model，不继承 editor；关闭后拒绝迟到 native 输入 | `reload_and_close_retire_native_drafts_and_editor_history` |
| 回调关闭/同名替换与 callback/binding panic | R1 的 integration/edit 与 app/edit 单测 |
| 已取出的 batch、重入队列、相同 owner/document 的 epoch 退休、无 session 时的 discard 回调 | shell `text_input_mac::tests::retiring_a_native_batch_invalidates_detached_edits_even_when_owner_and_text_return` |

这一轮未发现需要改变接受政策的新反例。shell 仅将既有 `is_current` 与 Rust 状态退休
集中到私有 `State`，让测试调用生产判断；InputView 继续在释放状态借用后调用 AppKit。
保留现有 epoch 回绕政策、owner/expected 校验和拒绝后同步，不增加公开原语。
headless 的 `apply_text_edit == true` 仍只表示投递；业务拒绝/归一化后的旧 continuation
依靠后继 expected 检查拒绝，shell 不重放剩余批次。单个投递完成后以最终 core 文档同步。

自动化验证：工作区 **924 passed、0 failed、3 个既有 ignored doctest**；
全目标 Clippy、格式、分层和 skill 新鲜度检查通过。未新增忽略测试。

**仍未完成的真实 OS 验收：**

1. 中文 IME 连续预编辑、候选选择、取消、提交后切焦点；确认只提交接受文本且 undo 单位正确。
2. 组合期间同值刷新和不同外部更新；确认 IMKit 旧候选/迟到回调不恢复退休 draft。
3. 拒绝与归一化后继续输入；确认 native mirror 恢复后可以开始新组合，候选内容/位置正确。
4. 上翻多行字段、查询/截图、窗口缩放，再编辑；观察手动滚动保持及 caret reveal。
5. 热重载、同名字段重建、关闭重开；保留窗口截图、原生 trace 和最终文档作为证据。

这些必须等用户可进行真实窗口操作后执行。协议状态单测没有创建 AppKit 窗口，
不能作为 IMKit 候选内容、位置或 OS 事件时序已经验收的证据。

每个阶段包含迁移、旧路径删除、验证、文档和提交。不得仅将旧函数搬入新模块后宣称 R1/R2 完成。
C2 的属性隔离可以保留，复合语义在 D 阶段处理。收敛完成后回到基础组件及应用验证，
不再用没有实际反例的微观内核重构延长此轮。
