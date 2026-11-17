# D1：不依赖 ui-kit 的组合验证

状态：2026-11-17，D1 已实现；D2–D4 尚未实现。基线 `bd082a7`。
验证代码位于 `crates/octara/tests/integration/foundation.rs`。这是外部 integration crate，
只能使用 core 的公开 API；`crates/octara/Cargo.toml` 的 dependencies/dev-dependencies
均没有 `octara-kit`。不是把 kit 的私有权限通过测试入口导出。

## 结论与范围

已有核心原语足够实现本轮三个场景，无需新增 Button、Field、Suggestion 内核节点，
也无需公开编辑器、部件注册表或第二套事件系统。ui-kit 应负责复用交互政策、内容组合
和默认外观；应用直接使用 core 时，也能获得同一焦点、IME、动作资格和目标身份保证。

这不证明所有未来组件均已具备基础能力。固定两个选项只用于验证机制；动态过滤、
选项删除/重排、重复名称、完整可用性和菜单政策仍由 D3 与既有 kit 测试承担。
真实 OS 输入、候选窗和辅助技术体验没有在本轮验收。

## 验证场景

| 场景 | 使用的公开机制 | 验证结果 |
| --- | --- | --- |
| 具有符号和动态文字的按钮 | 容器 children、Button role、label、focusable、disabled、on_press | 子内容点击、Enter、语义 Press、AX Click 到达同一根；禁用时拒绝；视觉文字变化保持目标身份 |
| 有标签、错误提示的中文输入 | text_input、value、behavior、description、required、invalid、show、live、focus | 标签聚焦真实输入；原生预编辑/提交使用普通编辑协议；错误变化不重建输入；应用回调观察组件 setter 后的值 |
| 输入拥有的建议弹层 | overlay/Placement、active_descendant、preserve_focus_on_press、on_key、is_composing | IME 优先于方向键；组合期间点击建议不提交；键盘/指针选择保留输入焦点；缩放保持输入 target 与弹层边界 |
| Agent/AX 观察 | Inspector target、name/actions、AccessKit 投影 | 名称放在真实交互根；容器不冒充输入；两种输出读取同一显式名称 |

应用业务只保存 value、open、active 与 sent/observed；没有复制 selection、composition、
undo、native mirror 或几何。建议项选中是应用对 value 的普通写入，因此沿用外部替换的历史
边界，不伪造键盘输入。`preserve_focus_on_press` 只维持焦点，**不自动阻止业务选择**；
指针选择使用 `is_composing` 明确让位于 IME，这属于建议组件政策。

## 对 kit 的审查结果

| 当前入口 | 实际返回对象 | 后续工作 |
| --- | --- | --- |
| button / primary_button / ghost_button | 文本叶子，带 Button role | 单一交互根改为可组合内容的容器，默认文字和自定义内容共用行为/样式；名称不依靠任意后代猜测 |
| text_field / text_area / number_field | 真实输入 | 保持原子输入，不允许任意 children；前后缀放在外部容器 |
| popover | 传入的 trigger，附加 overlay 和行为 | 保持 trigger 身份；明确传入者必须是真实交互根 |
| field | 包含 label/control/help/error 的容器 | 明确 control、label、container 的配置位置；不能把返回容器上的 handler/disabled 默认为控制内部输入 |
| collapsible / split | 外层布局容器；真正触发器/分隔条在内部 | 后续改成显式配置交互部位，删除误导性的泛 El 转发习惯 |
| combobox / popup_list / tabs | View；内部目标由组件创建/约定 | D3 再统一选项身份和导航，不能把菜单、建议、选中值当成同一种状态 |

D2 先收敛按钮动作根与复合组件配置位置。保留简单构造的简洁性，按实际内容需要提供
明确入口，不先生成统一 Root/Part 泛型体系。修改时迁移所有调用方与示例，不保留旧结构
的旁路实现。D3 再处理选项镜像；D4 处理默认外观政策，不把三项混成一次内核重写。

## Rune / Exact 的选择性参考

仍使用已记录的本地固定版本，没有声称联网检查了最新提交：

- Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`，
  `crates/rune-kit/src/actions.rs::Button::label_with`：自定义视觉标签与可访问名称分开，
  值得借鉴；不需要一起引入它的 Kit/Acts/Label 类型体系。
- 同版本 `field.rs::Field::build` 给外层 group 名称并插入 control。
  这提醒我们明确名称属于哪个目标；Octara 的验证直接检查真实输入上的 AX/Inspector 名称，
  不由外层标签或 group 推断输入必然已命名。
- Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3`，
  `semantics/corpus/edge/tap-child-of-button.contract` 明确覆盖按钮后代点击。
  Octara 用真实指针路径测试同一契约，不依赖 DOM，也不复制 Exact 的宿主节点分类。

此前的[交叉审查](../research/frontend-native-cross-review.md)继续有效：共享优秀机制，
而不是要求参考框架的所有分层和公开类型都出现在 Octara。

## API 使用体验

本轮没有遇到必须访问私有内核才能实现的功能。普通 El/Signal/闭包即可组合，较多样板
集中在建议键盘政策和显式语义，而非所有权或编辑桥接；这正是 kit 应封装的部分。

需要明确的写法：动态 `description` 返回拥有的 `String`；`.children(iterator)` 可直接
接收 `El`，额外 `.into()` 会让泛型目标不明确。这些是现有类型契约，不为两个调用点新增
转换层。没有以代码行数作为新增原语的理由。

## 验证记录

工作区 **926 passed、0 failed、3 个既有 ignored doctest**；全目标 Clippy、格式、
分层与 skill 新鲜度检查通过。测试实际走 public core 的 headless 指针、键盘、语义、
AccessKit action 和 native document 入口；没有真实窗口截图或 OS 输入验收结论。
