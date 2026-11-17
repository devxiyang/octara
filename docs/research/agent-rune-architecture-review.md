# Rune 从基础原语到 kit 的独立架构审查

日期：2026-11-17。状态：只读源码调研与提案审查，未实施。

结论：Rune 支持 Octara 的“基础交互机制归窗口、控件组合归 kit、默认外观复用行为”方向，
也提供了测量与发布分开的有用实现。它没有证明独立无样式 crate、通用 Root/Part 注册机制、
公开容器/原子类型划分或整套主题 schema 迁移是必要条件。
尤其不能把 Rune 的应用拥有 TextEdit、构建时 Kit 快照、手工 memo version 和重建重试直接搬到 Signals。
下文的“建议修订”均针对[分层提案](../design/layered-architecture-redesign.md)，不表示实现已改变。

## 证据与验证范围

| 对象 | 核对到的版本与状态 | 本轮操作 |
| --- | --- | --- |
| Octara | HEAD `b117aeea644d2480ff62be0c4a004ac7c7ed0fbc`；已有架构文档修改及未跟踪设计/研究文档 | 阅读 AGENTS、分层提案、前端语义交叉审查、框架调研及相关实现；只新增本文 |
| Rune | HEAD `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`；工作区已有 `rust-toolchain.toml` 修改和一份未跟踪 toolchain changelog，本文所引实现不在该修改列表 | 阅读 AGENTS、实现与已有测试；未 fetch、切分支或修改参考仓库 |

以下 Rune 链接均固定到上述 commit，Octara 实现链接固定到上述 HEAD。
提案目前是未提交文档，其章节和行号以本轮阅读版本为准。
本轮没有编译、运行 Rune 或 Octara，没有新增测试或窗口实验；“测试覆盖”只表示已阅读的测试中存在相应断言，
不表示本轮运行通过，更不表示原生宿主、IME 或辅助技术经过动态验证。

本审查接续[前端语义交叉审查](frontend-native-cross-review.md)和[框架架构调研](ui-composition-frameworks.md)，
专门检查它们尚未证明的边界，不以 README 中的分层名称代替实际调用链。

## 支持、反对与条件采用矩阵

| 提案机制 | 判断 | Rune 的实现证据与反例 | Octara 的最小选择 |
| --- | --- | --- | --- |
| 窗口管理焦点、输入资格、模态栈；kit 声明需求 | 支持 | Dialog → `El::modal` → `sync_modals`；Popover 则使用独立的 `focus_return`，没有注册 modal，见发现 1 | 共享窗口机制，逐类规定关闭、恢复和 portal 归属 |
| 先在 kit 内复用行为，再按需求开放无样式能力 | 支持 | Kit builder 直接使用 core El/输入原语；popover 的放置、外观、滚动是普通组合函数 | 不新增调度器或完整 headless crate；首个自定义外观必须复用实际行为 |
| 所有复杂控件强制 Root/Part、统一 Widget trait | 反对作为默认方案 | Rune 的 Field/Combobox 是 builder，必需信息可以直接传入；Field 允许没有 control，Dialog 没有 close 回调会静默不构建 | 优先明确 control/label/trigger 参数；只有允许动态部件时才需要部件关系诊断 |
| 一个交互根、可组合视觉标签与明确名称 | 条件采用 | Button 保留显式名称，但 custom label 整棵 `a11y_hidden`；custom trigger 的旧 press 会被覆盖 | 保留便捷构造；限制标签内交互；动作冲突由 Octara 自己定义，不能声称 Rune 已解决 |
| 编辑草稿、当前选择、active 分开 | 支持职责，反对照搬所有权 | 应用拥有 ComboboxState/TextEdit；InputSpec 克隆它，并用 access 修改原对象；选择前先关闭 popup | 保留文档拥有编辑状态、Signal 拥有业务值；说明提议和接受的关系 |
| 文本内部 runs/links 不占应用 children | 支持原则，条件采用公开类型拆分 | TextSpec 拥有 runs/links，AX 才投影子节点；但所有 El 都有 children，混合方向段落还转成普通子树 | 先分清所有权与过期目标；公开构造类型以真实组合任务验证，不能援引 Rune 作为既定证明 |
| core 资源解析、kit schema、品牌值分工 | 支持职责，反对机械搬迁 | Rune core 同时有 `theme::Theme` 数据和 `tok::Theme` 通用默认值，neutral kit 由后者生成 | 按消费者区分通用文本/交互 token 与组件专属 token；保留单一资源更新协议 |
| 测量与 apply 分开、缓存声明依赖 | 支持 | Measurement 不持有 Scene 或 `&mut S`，Measured 才进 apply；继承输入参与复用 | 私有能力边界与明确失效规则，保留增量文档 |
| Rune 全树 rebuild、手工 version、重试强制测量所有行 | 反对直接迁移 | measure retry 重新调用 root view，memo 捕获变化必须进 version；超过重试上限强制全量 | Signals 自动依赖与局部结构更新继续承担更新；有界反馈不等于全量退路 |

## 发现 1：真正共享的是窗口机制，Popover、Dialog、建议列表仍有不同契约

**已读调用链。** Rune Dialog 在 `build` 中构造标题、关闭按钮、surface/scrim，再把 panel 标成 modal；
应用提供 open 和 dismiss 回调。`El::modal` 只存 modal 声明；commit apply 后，runtime 的模态机收集已挂载结果，
取消旧手势、限制输入和恢复焦点。这里不需要 kit 调用 OS，也不需要每个 Dialog 自己维护焦点陷阱。
见 [dialog.rs:34–65][r-dialog]、[el/focus.rs:262–277][r-modal-el]、
[modal_focus.rs:96–159][r-modal-sync] 和 [commit/apply.rs:874][r-apply]。
已有 [focus.rs:729–749][r-modal-tests] 测试断言嵌套关闭、背景阻断与恢复。

**需要纠正的泛化。** 同一仓库的 Popover 并没有调用 modal：它给内容设置 `role("dialog")`、
`focus_return`、anchor，并通过普通 scrim 关闭；恢复由 `sync_requested_focus` 检查被移除的范围和旧焦点完成。
Combobox 的 Escape 则由搜索输入的 key handler 关闭，不是由 modal 栈完成。
见 [overlay.rs:155–206][r-popover]、[focus.rs:288–318][r-focus-return]、
[combobox.rs:242–258][r-combo-keys]。角色、展示层、输入屏障、模态性不能互相替代。
Rune 的原始 PopupList 也明确不抢键盘焦点，适合外部编辑器的建议列表；见 [popup_list.rs:85–109][r-popup-state]。

**portal 的额外义务。** Rune 的 modal scope 会沿 anchor 关系纳入 root sibling 的弹层；
它不是仅沿布局父链判断归属，见 [focus.rs:72–97][r-modal-scope]。
这支持提案 §8.2 的“展示位置不能改变生命周期”，但仅保留作用域还不足以保证输入资格：
Octara 还需要表达“哪个弹层属于哪个交互边界”，并在 anchor 被移除时失效。
这是小的 owner/anchor 关系，不构成新增全局部件框架的理由。

**建议修订。** 在提案 §7.1、§8.2 增加一句：Dialog、modal popup、非 modal popover、编辑器建议列表分别规定
背景输入资格、Escape、焦点恢复、focus-visible 和 Tab 策略；默认外观不决定这些行为。
“共享选项导航”只共享有效项选择等规则，不强迫它们共用一个焦点政策。

**最小验收。** Dialog 内打开搜索 popup；一层 Escape 只关闭内层，第二层关闭 Dialog；
在内层点击外部、移除 trigger、显式把焦点转向别处时分别检查恢复；
建议列表选择一项后仍保留编辑器 selection/composition。记录输入与语义树，截图只作为外观证据。

## 发现 2：builder 可以明确配置目标，但不能自动保证标签关系、动作组合或禁用

Rune Button 的根是 row，角色、显式 label、focus/disabled 和动作属于根；
`label_with` 只拿字体与 ink，返回视觉内容。改变 label 内容不需要把文本节点当整个布局容器。
见 [actions.rs:266–278][r-button-label]、[actions.rs:348–374][r-button-build]。
[button_label.rs:36–50][r-button-tests] 断言自定义内容收到相同字体/颜色、按钮名称及宽度；这是有针对性的源码证据。

但整棵 custom label 被 `a11y_hidden`，而该标记本身仍允许输入。
因此这不是“任意内容可安全放进标签”的证明：带独立意义的状态文字、链接、嵌套按钮都需要自己的约束。
见 [el/a11y.rs:179–183][r-hidden]。

Field 的反例更明确：它给外层 group 设置 label/description、隐藏可视 label，然后直接 child(control)，
没有给实际 editor 设置标签关系；见 [field.rs:48–75][r-field]。
已有 a11y 测试自己同时设置 `Field.label("Token")` 与 `TextInput.label("Token")`，
见 [tests/a11y/controls.rs:210–239][r-field-tests]。
它证明“先配置实际控件再传入 builder”很直白，也证明不能仅凭 Field builder 推定内部语义正确。
Octara 当前 Field 已直接给实际 control 写 label/description/invalid，不能在重设计时退回只命名 group；
见 [forms.rs:32–75][o-field]。

自定义 trigger 也有组合漏洞：Rune Combobox 对传入 El 调用 `on_press_tx`；
该原语只给一个 `Option` 槽赋值，会替换旧动作，并没有一般的 handler 合并或冲突拒绝。
custom trigger 分支也不经过默认 Select 的 disabled 配置。
见 [combobox.rs:197–214][r-combo-trigger]、[el/handlers.rs:5–20][r-press]。
基础 `El::disabled` 说明还要求调用者移除 press handler；kit 通过 Acts 在 disabled 时不附加 handler，
见 [el/a11y.rs:169–176][r-disabled]、[kit/lib.rs:219–236][r-acts]。
这些是已读实现限制，不把未经运行的组合场景当作已复现缺陷。

**建议修订。** 保留提案 §8.2 的明确控制点与单交互根，但区分两类 API：

- 必需 control/label 可由构造参数和专用 builder 表达，不必为了“缺少部件可诊断”先造注册表。
- 允许任意位置、动态出现的部件时，才要求局部关系建立、唯一性检查和缺失诊断。

custom trigger 宜优先表达“换根的视觉内容”；如果开放现成可交互 El，必须规定旧主动作、角色、禁用与 focus 的冲突。
用已有 Behavior 顺序解决复用，不把 Rune 单槽覆盖当作范式。
禁用资格必须在执行边界检查，不能只靠隐藏 handler 或改颜色。

**最小验收。** 动态文字加图标按钮保持一个操作目标和同步名称；
Field 只给一次 label 就能在实际 editor 上读到名称和错误；
自定义 trigger 的旧 press、冲突 role、动态 disabled 均产生明确结果；
标签中的独立交互内容要么受支持且语义合理，要么构造/挂载时明确拒绝。

## 发现 3：Rune 的编辑所有权与“受控选择”并不等于 Octara 的受控值协议

Rune ComboboxState 由应用拥有，其中包含完整 TextEdit 和 Closed/Open phase。
打开时 clear search、请求焦点；关闭时清除 focused、取消 composition、结束 undo group；
highlight 按 option id 保存，筛选后回退首个 enabled 项。
见 [combobox.rs:22–104][r-combo-machine]。
builder 克隆 state 和 options，再为回调保留可回到原应用状态的 access。
见 [combobox.rs:134–164][r-combo-ownership]。

TextInput 构建时克隆 TextEdit，最终 InputSpec 仍持有 clone 和 access；
编辑时 runtime 先通过 access 修改应用的真实 TextEdit，再触发 change，之后构建下一份展示。
焦点变化也会写回应用编辑器并重新克隆镜像。
见 [text_input.rs:174–194][r-text-input-copy]、[el/input.rs:51–96][r-input-spec]、
[keys.rs:164–213][r-edit-route]、[focus.rs:35–61][r-edit-focus]。
这个镜像有明确握手，不应被误读为 kit 有独立编辑器；也不适合直接加到 Octara 的文档编辑器外面。
Octara 的既有契约正好相反：应用提供 value，runtime 按字段拥有 selection/composition/history，
见 [app/edit.rs:1–8][o-edit]。

键盘保护分两层：runtime 在 preedit 期间优先吞掉 Enter/Escape，Combobox handler 在 preedit 存在时退出；
其余导航、修饰键及 repeat 按控件契约处理，见 [keys.rs:18–34][r-key-route] 和 [combobox.rs:242–299][r-combo-keys]。
已有 [combobox tests:92–165][r-combo-tests] 分别断言 IME、应用快捷键优先级与长按 Enter 只选择一次。
“保留 TextEdit”本身不能代替这些规则。

`current/on_choose` 表示应用控制 committed selection，但 Rune 选择时先 close，再调用 choose，
并没有返回“业务接受/拒绝”的协议。调用者完全可以不更新 current，popup 仍已关闭。
同时 PopupList 将 active 投影为 `focus_selected`，a11y 测试会在仅按箭头后断言 selected 改变；
见 [combobox.rs:253–258、326–330][r-combo-keys]、[popup_list.rs:234–254][r-popup-active]、
[a11y/overlays.rs:218–223][r-combo-a11y]。
Octara 已明确区分 highlight 与 selected，并可保留这个更强契约，见 [octara combobox.rs:165–235][o-combo]。

**建议修订。** 提案 §6、§7.1、§8.3 中的所有权保持不变，补上三项实施义务：
编辑文档与业务 value 的仲裁、被拒绝/变换的选择对 open/draft 的影响、动态 options 下回调读取的有效目标。
不要把 Rune 的 focus 字段回写、access 返回 `&mut TextEdit` 或整份状态快照变成 Octara 新公共状态语言。
closed/open 可以沿现有 scope Signal 表达；只有存在互斥 phase 约束时才需要局部 enum，
不把各自独立的 disabled、focus、checked 全部塞进同一状态机。

**最小验收。** 输入中文时筛选、改 label、禁用当前项、移除/重排 options；保持 editor 身份与 IME；
active/selected 在业务拒绝选择时仍可解释；长按提交不重复；
关闭后延迟到达的选择/原生编辑不能作用于新一轮 popup 或新字段。

## 发现 4：文本拥有内部结构有证据；公开“原子内容类型”是 Octara 自己的设计选择

Rune TextSpec 持有 content、spans、links、blocks、pills、方向与 shape key，
链接命中从同一文字布局提取范围盒子，AX 在输出阶段投影 link/text-run 子节点。
见 [text_spec.rs:8–89][r-text-spec]、[selection/links.rs:134–170][r-link-hit]、
[a11y/text.rs:83–136][r-a11y-text]。
这支持提案 §5.2：内部链接无需借用应用 `.child()` 接口；应用结构树、绘制结构与 AX 树可以不同。

但 Rune 只有一个 El，text 和 children 是并列字段，`child()` 无内容类型限制，
见 [el.rs:259–280][r-el]、[el.rs:430–459][r-el-child]。
更直接的反例是 `dir_by_paragraph`：混合方向的只读文本会被拆成 keyed 段落子树，
自身 text 被取走、角色改 group、原 children 接在内部段落之后；editable/secure/live 等情况保留单文本。
见 [el/paragraphs.rs:12–43][r-paragraphs]。
因此 Rune 没有替 Octara 验证“所有文本/输入/图标都应有不同公开 builder 能力”。
内部表示可以很复杂，同时保持原子公开内容；二者不能混为一谈。

链接身份也需要独立判断。Rune AX link id 由 parent NodeId 加 link 的序号产生，
动作继续携带 path/index，并在执行时取当前 links[index]。
见 [a11y/tree.rs:19–29][r-link-id]、[a11y/actions.rs:36–54][r-a11y-act]、
[selection/links.rs:214–221][r-link-act]。
这里不能推出“删除后在同一位置添加新链接，旧操作必然失效”；本轮没有运行该竞态。
Octara 目前 `sync_links` 也按 existing[k] 复用节点，见 [kernel/rich.rs:254–295][o-rich-owner]。
提案要求的 stale target 保护是需要显式实现与验收的加强，不能由“文本拥有 links”自动获得。

**建议修订。** §5.1 保留容器与原子内容的结构契约，但把“公开 builder 在类型能力上区分”标为待验证的 API 选择：
先证明应用不能把 child 挂到错误所有者、文本更新不会删应用孩子；然后比较少量公开 wrapper 与其他受控构造是否都能完成任务。
不预先生成每种控件一套泛型状态或通用子树分区框架。
§5.2 明确链接身份连续性规则：更新同一对象、移除、重新创建分别如何处理；范围或序号不能单独证明对象仍相同。

**最小验收。** 内部 link 插删与富文本替换不影响相邻布局内容；旧 Inspector/AX target 不点击替换后的新链接；
RTL/LTR 混合段落的绘制、命中、选择、复制和 AX 共用几何；
候选公开类型可自然支持异构 children、共享样式 helper、自定义 Button label、输入前后缀和原生视图叠层，
再确定它值得保留的类型数量。原子结构限制不能被挂载 hook 绕过。

## 发现 5：主题 schema 的分工值得采用，快照 Kit 与通用 token 整体迁移不值得照搬

Rune 有两层不同数据：core `theme::Theme` 保存字符串颜色/数值表，kit 定义 Palette/Type/Radius/Motion，
品牌 crate 把数据填成 Kit；见 [theme.rs:28–36、106–146][r-theme]、[kit.rs:1–23、229–248][r-kit]、
[theme-opencode/lib.rs:81–105][r-brand]。
但 core 也保留 `tok::Theme` 的通用背景、文字、hover、press、knob 等默认设计值，以及基础 type/space/radius；
`Kit::neutral` 就使用这些值，见 [tok.rs:136–208][r-tok]、[neutral.rs:16–97][r-neutral]。
不能从“kit 有 schema”推导出“core 只能有无语义颜色”。

Octara 的 SurfaceHover/Accent/AccentHover 也有低层消费者：inline code 的默认背景和链接颜色在 rich.rs，
框架内置 ask 菜单/提示用同一批槽位；见 [rich.rs:116–122][o-rich-code]、[rich.rs:456–462][o-rich-paint]、
[asks.rs:469–482、508–527][o-asks]。
把这些名称全部移进 kit，会迫使 core 依赖 kit、到处传默认配方，或丢失不使用 kit 时的默认外观。
这些都是需要证明收益的真实成本。

建议按消费者列三类：通用资源与尺度解析留 core；通用文字/链接/焦点/选择的默认样式可继续由下层提供；
只为某个 Button 变体、Dialog header、Combobox row 等服务的设计 token 归 kit。
当前 SurfaceHover 名称是否适合表达 code 背景可以单独审计，但改名与迁层不是同一个决定。
App 仍是资源变更的唯一所有者，schema 所在模块不能成为第二套主题运行时。

更重要的是更新时间：Rune builder 用 `&Kit` 读取具体 Color 和尺寸，下一次重建才重新读取；
Kit token 的 Hash 可以参加手工 memo version，品牌 kit 还按 dark/blend 缓存。
见 [kit.rs:17–21][r-kit]、[actions.rs:55–66、353–374][r-button-build]、[brand lib.rs:84–87][r-brand]。
Octara 当前 Theme 则在 kernel 解析符号 token，切主题不重跑闭包，见 [theme.rs:1–4][o-theme]。
若新 schema 只在一次 build 时转成 Color，会破坏这一能力。
应保留 token 到样式解析的依赖，或明确地注册响应式外观绑定；不为迁 schema 强制全树重建。

样式状态同样不能机械复制：Rune `State` 是 gallery 强制视觉状态，Rest 才把 hover/press 交给 runtime；
它不是 checked、open、focused 等全部事实的统一所有者，见 [kit/lib.rs:193–202][r-forced-state]。
Octara 可继续由真实交互/语义 facts 驱动状态 bitflags，继承和覆盖规则由一套解析器解释，
见 [kernel/style.rs:22–45][o-style]。不需要为了熟悉的前端词汇加一份视觉 open/checked。

**建议修订。** §8.3 的主题段明确上述三类消费者和动态失效要求；
将“组件专属 token 不继续进入 core”保留为规则，将“已有通用槽位迁出”保留为逐项决定。
共同外观解析可以是现有样式设施的整理，不先建立新的主题协议、全局 style engine 或完整 CSS 级联。

**最小验收。** 不依赖 kit 的文本/链接/内置 ask 仍有合理默认样式；
双窗口切 theme、字体、密度时，默认 kit 与第二种外观同步更新且不重建业务 scope；
Inspector 能解释符号来源、状态和最终覆盖，disabled/focus/checked 同时存在时仍一致。

## 发现 6：Measurement 的能力边界值得借鉴，重建重试与 memo 快照不是 Signals 的新基础

Rune Measurement 持有 `&S`、text/layout/memo/virtual 缓存等，但不持有 Scene、动画增长/reveal tracks 或 `&mut S`；
Measured 才成为 apply 输入，重试不返回可 apply 的中间结果。
见 [commit.rs:48–118][r-measurement]。
commit 在几何、focus 请求和 caret read 稳定后调用 apply，见 [commit.rs:139–153][r-commit]。
这支持提案 §3、§6 的私有能力边界与“几何重试不能重复业务动作”。

但 Rune 允许一次 commit 中多次运行 view：每次 measure_view 新建 Cx，再 root_view/build_tree；
虚拟窗口不足时循环测量，超过八轮进入 forced pass，挂载全部行。
见 [measure.rs:169–209][r-measure-loop]、[measure.rs:369–430][r-measure-view]、[commit.rs:6–44][r-pass]。
`&S` 只禁止经该借用直接写 S，不禁止闭包通过内部可变性或外部服务产生副作用。
所以不能由这些类型宣称一般 Rust 闭包纯净、任意副作用可回滚，或将这套循环包进 Octara 构建作用域。

Rune memo 缓存整个已构建 El 及捕获的 handler，变化输入必须进入 version；
部分环境、布局读取等另行参与失效。flatten reuse 还比较继承的方向、inert、scale、foreground 等，
之后是否能跳过 apply 又看布局、动画、presence 和 reveal 等。
见 [memo.rs:11–32、596–611][r-memo]、[memo_reuse.rs:6–20、65–77][r-memo-reuse]。
“身份稳定”不能替代依赖有效性，Rune 在这里恰好是反例。
同名 `memo` 也不能掩盖区别：Octara 的 memo 是 Signals 派生值，Rune 的是手工输入版本控制的视图快照。

**建议修订。** §6、§8.1 的方向保持：先用私有函数/类型约束几何阶段可做的事，
保留一次 build 与属性/结构的增量更新。缓存逐个声明输入和失效，而不统一套一个 Revision。
§12 的性能验收补充“退化路径的驻留上界”：不得因有界重试而静默创建所有虚拟行。
公开 Executor/Transaction、第二个 kit 调度器或整体 memo 搬运，都不是借鉴 Measurement 的前置条件。

**最小验收。** 人为触发一次以上布局反馈，业务回调、cleanup、动画启动只发生预期次数；
移除 scope 后迟到结果不发布；主题/方向/inert/scale 改变使相关缓存失效；
大虚拟列表的退化场景仍有明确行数和内存上界。统计绑定/构建/布局次数和响应时延，不能仅凭输出像素相同宣称无退化。

## 应回写总提案的六处内容

| 位置 | 需要补充或收窄的表述 | 完成判据 |
| --- | --- | --- |
| §7.1、§8.2 | portal 需要局部交互归属；Dialog/modal popup/非 modal popover/建议列表各自规定焦点和关闭政策 | 嵌套与 anchor 移除场景的输入/AX 证据 |
| §8.2 | builder 的必需参数与动态部件注册不是同一问题；不用所有控件承担通用部件框架 | 默认和自定义标签/trigger 共享行为，实际 control 自动获得关系 |
| §6、§8.3 | 明确编辑文档仲裁与选择拒绝；不引入 Rune TextEdit 快照/access 公共所有权 | IME、动态选项、业务拒绝、长按与迟到目标全部可解释 |
| §5.1、§5.2 | 结构所有权先定；公开容器/原子类型签名待组合实验；链接身份不按序号推定 | 应用 child 与内部 runs 隔离、旧 target 失效、候选 API 可用 |
| §8.3 | 区分通用默认 token 和组件专属 schema；保留一次构建下的动态主题依赖 | 无 kit 使用仍完整；双窗口主题更新不重建 scope |
| §6、§12 | 能力边界不等于纯 Rust 事务；虚拟列表退化也需资源上界 | 多轮反馈无重复业务动作，驻留/内存与失效证据成立 |

这些修订不要求新 crate，也不要求先重做所有公共名称。
最小实施入口应是一条贯穿实际控件的切片：组合 Button label、自动命名的 Field、带 IME 的 Combobox 及嵌套弹层，
用它检验下层所有权、公开原语、默认与第二种外观；通过后才扩大公开部件和类型范围。
本文没有证明任何尚未实施的候选 API 已更易用或更快。

[r-dialog]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/dialog.rs#L34-L65
[r-modal-el]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/focus.rs#L262-L277
[r-modal-sync]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/modal_focus.rs#L96-L159
[r-apply]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/apply.rs#L874
[r-modal-tests]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/focus.rs#L729-L749
[r-popover]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/overlay.rs#L155-L206
[r-focus-return]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/focus.rs#L288-L318
[r-combo-keys]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L242-L330
[r-popup-state]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/popup_list.rs#L85-L109
[r-modal-scope]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/focus.rs#L72-L97
[r-button-label]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/actions.rs#L266-L278
[r-button-build]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/actions.rs#L55-L374
[r-button-tests]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/tests/button_label.rs#L36-L50
[r-hidden]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/a11y.rs#L179-L183
[r-field]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/field.rs#L48-L75
[r-field-tests]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/tests/a11y/controls.rs#L210-L239
[r-combo-trigger]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L197-L214
[r-press]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/handlers.rs#L5-L20
[r-disabled]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/a11y.rs#L169-L176
[r-acts]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/lib.rs#L219-L236
[r-combo-machine]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L22-L104
[r-combo-ownership]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L134-L164
[r-text-input-copy]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/text_input.rs#L174-L194
[r-input-spec]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/input.rs#L51-L96
[r-edit-route]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/keys.rs#L164-L213
[r-edit-focus]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/focus.rs#L35-L61
[r-key-route]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/input/keys.rs#L18-L34
[r-combo-tests]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/tests/combobox.rs#L92-L165
[r-popup-active]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/popup_list.rs#L234-L254
[r-combo-a11y]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/tests/a11y/overlays.rs#L218-L223
[r-text-spec]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/text_spec.rs#L8-L89
[r-link-hit]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/selection/links.rs#L134-L170
[r-a11y-text]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/text.rs#L83-L136
[r-el]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el.rs#L259-L280
[r-el-child]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el.rs#L430-L459
[r-paragraphs]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/paragraphs.rs#L12-L43
[r-link-id]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/tree.rs#L19-L29
[r-a11y-act]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/a11y/actions.rs#L36-L54
[r-link-act]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/selection/links.rs#L214-L221
[r-theme]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/theme.rs#L28-L146
[r-kit]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/kit.rs#L1-L248
[r-brand]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/themes/opencode/src/lib.rs#L81-L105
[r-tok]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/tok.rs#L136-L208
[r-neutral]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/neutral.rs#L16-L97
[r-forced-state]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/lib.rs#L193-L202
[r-measurement]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit.rs#L48-L118
[r-commit]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit.rs#L139-L153
[r-measure-loop]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/measure.rs#L169-L209
[r-measure-view]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/measure.rs#L369-L430
[r-pass]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit.rs#L6-L44
[r-memo]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/memo.rs#L596-L611
[r-memo-reuse]: https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/memo_reuse.rs#L6-L77
[o-field]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara-kit/src/forms.rs#L32-L75
[o-edit]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/app/edit.rs#L1-L8
[o-combo]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara-kit/src/combobox.rs#L165-L235
[o-rich-owner]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/rich.rs#L254-L295
[o-rich-code]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/rich.rs#L116-L122
[o-rich-paint]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/rich.rs#L456-L462
[o-asks]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/app/asks.rs#L469-L527
[o-theme]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/theme.rs#L1-L4
[o-style]: https://github.com/devxiyang/octara/blob/b117aeea644d2480ff62be0c4a004ac7c7ed0fbc/crates/octara/src/kernel/style.rs#L22-L45
