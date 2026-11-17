# Exact 架构与语义控件独立复核

日期：2026-11-17。用途：交叉审查 [Octara 分层提案](../design/layered-architecture-redesign.md)、
[前端语义审查](frontend-native-cross-review.md)与[框架调研](ui-composition-frameworks.md)。
本轮只读实现、测试与 issue，未构建、未运行参考应用或测试；下文的“测试覆盖”均指已有测试源码。
建议与最小验收案例尚未实施。

| 项目 | 固定源码基线 | 工作区与证据限制 |
| --- | --- | --- |
| Exact | `08c889a9de21a30b7c3b308422fc588cf1a480d3` | 本地 `/Users/devxiyang/code/archive/reference/exact`；已有 `rust-toolchain.toml` 修改，本轮未改动、未作为结论依据 |
| Octara | `b117aeea644d2480ff62be0c4a004ac7c7ed0fbc` | 本地当前源码；上述设计/调研文档属于本轮已有工作区草稿，并非该提交已实施的能力 |

未 fetch、切换或修改分支。Exact 的私人 LLP 仅作为源码注释中的决策编号，不把未读到的设计正文当作证据。
历史 issue 的记录版本、当前源码、已有测试与实际运行结果分开陈述。

## 1. 熟悉的 HTML/CSS 词汇由多层兑现，不是 kit 或 schema 的单层承诺

Exact 的作者契约是：布局采用受支持的 CSS 名称、值和默认模型；未声明的控件外观采用平台政策，
作者声明的内容优先。它也明确不是无限制的浏览器样式表，不提供完整选择器级联。
因此，“写得像网页”不能推出“每个宿主都执行完整 HTML/CSS 语义”。
来源：[AGENTS.md 的作者/平台优先规则](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/AGENTS.md#write-the-web-ship-the-platform)、
[样式契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#styling-and-layout)。

| 边界 | 当前可见的保证 | 不能由这一层推出的保证 |
| --- | --- | --- |
| schema / 生成代码 | 节点、属性、样式行、枚举、wire 编码与默认值的声明来源 | 全部结构合法性、业务校验、宿主交互已实现 |
| Contract compiler | 名称/类型/内容限制、样式展开、原生控件可接收的外观与内容 | OS 编辑协议、辅助技术实际读取、上屏完成 |
| kernel | 文档身份、内容结构校验、样式/测量规则；Range、Select 等共享语义规则 | 所有平台的焦点、键盘、输入过滤与原生控件行为 |
| runner | 事件载荷资格、动作与更新；例如拒绝不存在或禁用的 Select 值，Range 载荷统一规范化 | 一个角色字符串足以使宿主拥有步进或数字编辑能力 |
| native / painted / web host | OS 控件、文本输入、命中与焦点、平台 AX、布局回报、呈现 | 只凭共用 kernel 或 headless 测试就获得所有宿主一致性 |

定位：[schema 声明范围](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/tables/schema.json#L1-L4)、
[ControlKind / Range](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/control.rs#L13-L45)、
[runner 的事件载荷校验](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/control.rs#L10-L114)、
[painted host 的职责](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/lib.rs#L7-L30)。
这张表是职责图，不要求所有目标都经过同一运行路径；Exact 的 web JS 目标还在自己的
[DOM runtime](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/web-js/rt.js#L679-L685)兑现受控输入规则。

**对提案的修订：**保留 §7–9 的共享语义规则与宿主适配边界，但对每项熟悉词汇列明保证所在层和宿主能力。
“Host 不另造控件语义”应指不独立定义另一套规则，不能理解为宿主无需实现该规则的 OS 行为。
重复且纯粹的选择、范围、动作资格规则适合 Rust 共用；系统键盘、IME 协议、原生测量与 AX 映射留在宿主。

## 2. 内容所有权必须明确；两类公开 Rust builder 仍是候选方案

Exact 对“能否持有 children”“是否测量叶子”“是否 replaced element”分别判断：

- Text 是测量叶子，仍可持有文本 run 和行内图片；它们由段落测量，不作为普通布局子节点测量。
- Control 是测量叶子，仍持有 Select 的 options 或原生按钮的 title/image。
- Canvas 是 replaced element，允许在自身盒内布局 children，但 children 不决定 Canvas 自身尺寸。
- TextInput、Image、WebView 则拒绝普通 children。

这些规则写在 [node.rs:67](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/node.rs#L67-L138)，
并有 [rules_cover_every_type 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/node.rs#L184-L218)。
它是“用不同问题描述内容”的证据，不支持把 leaf、原子内容、无后代、无语义子对象合并成一个概念。

内容合法性也不全由 schema 保证。[compiler](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/lower/src/nodes.rs#L825-L844)
和 [kernel txn](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/txn.rs#L400-L433)
共用 `NodeType` 语义判断；kernel 还验证重复 child、循环、存活身份及 Text 的内容种类。
这是作者入口及内部写入口共同守住约束，不是依赖公开 Rust 泛型把所有错误挡在编译期。

Octara 已有单一私有 Content 枚举与
[wrong_content / ELEMENT_CONTENT](../../crates/octara/src/view/element.rs#L251)，
但 [child / children](../../crates/octara/src/view/element.rs#L1061)仍无条件接受内容。
已有 wrong_content 证明运行时诊断模式存在；它目前没有证明应用 children 与内部链接已隔离。
[sync_links](../../crates/octara/src/kernel/rich.rs#L254)把整份节点 children 当作链接列表，按位置复用，
这正是应修订的所有权边界。

**对提案 §5.1 的修订：**把硬契约写成“每种内容声明可接受的应用子内容及内部对象归属；
不合法组合在进入挂载/协调前拒绝，内部写入口仍验证”。
容器/原子内容可以是 Octara 有意选择的简化政策，但“公开 builder 必须用类型能力区分”应降为候选。
实施切片比较少量具体 builder 类型与保留 El、补足诊断两种成本，不能由 Exact 推导必须引入类型状态或大量泛型。
即使选择静态限制，View 擦除类型后的挂载和内部更新仍需要检查。

## 3. 插槽、语义名称与内部文本对象分别有契约

Exact 的普通 slot 只有一个调用者内容入口；内容保留调用者词法作用域与 provider。
[compose 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/cli/tests/it/compose.rs#L39-L86)
覆盖作用域、provider 以及转发时的类型校验。
它既不是命名部件协议，也不自动建立 Field 的 label/control/description 关系。
不能拿 slot 支持组合，来证明任意 render 合并、所有权转移或所有控件都应拆 Root/Part。

另有一个更窄的规则值得区分：
[anatomy/slot.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/lower/src/anatomy/slot.rs#L1-L13)
约束导出的特定 library component：slot 恰放一次、无条件、不转发，沿途不可隐藏内容，
不能折叠的相关值直接拒绝。
[对应测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/cli/tests/it/anatomy_slot.rs#L1-L7)
明确排除普通 app component 和不满足 library 条件的 package。
这是受限声明语言及库出口政策，不宜泛化成 Octara 所有动态子内容都必须常驻可见。

按钮名称与后代暴露也必须分开。Exact 的
[press_face](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/arena/button.rs#L125-L189)
同时记录显式 label 与 title/image，且可映射的 face 形状有边界；不是把任意子树拼接后就得到等价原生按钮。

| 证据 | 记录版本与当前状态 | 能支持的结论 |
| --- | --- | --- |
| [按钮内 heading issue](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261010-macos-a-heading-inside-a-button.md) | 旧实测基线 `d413487a8`；文件 Closed，resolution 为 #451 | 历史 macOS 曾把所有按钮当作 AX 叶子；不能据此称当前 macOS 仍未修复 |
| [当前 NodeViewMac:408](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/NodeViewMac.swift#L408-L436)及 [回归测试:146](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Tests/ExactKitTests/AccessibilityTests.swift#L146-L183) | 当前源码有 controlAccessibilityChildren；测试要求保留 heading、名称、状态与动作，普通 text/image 仍折叠 | 已有针对性修复实现和测试源码；本轮未运行 XCTest 或 VoiceOver |
| [当前 painted AX walk:84](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/presenter/access/walk.rs#L84-L113) | pressable 后设置 inside，Text/Image/Svg 在 inside 时提前返回，早于 heading 判断 | 当前这条源码路径仍有宿主差异线索；未运行，不能称所有 painted 平台已复现同一缺陷 |

**对提案 §5.2、§7.2、§8.2 的修订：**保留内部文本所有者、共享名称规则与具体部件关系，
补充“装饰内容折叠、作者显式独立语义保留”的边界验收。
不要把应用 children、内部 link、AX run 强制收成一棵同构树；也不要把外部名字当作对象身份。
Slot 的作用域保证可借鉴，Exact 的库出口静态可见性限制不应成为 Octara 动态 UI 的默认政策。

## 4. 受控值不会消除编辑中的事实；数字语义不能由键盘或角色代替

Exact 的 [输入契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#input-events-and-commands)
明确：input 更新草稿，change 在 Enter/blur 规范化，submit 是显式提交；number 字段仍走字符串编辑，
Range 才是数值载荷。其 [number-field recipe](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/recipes/number-field.md#L60-L68)
保留原始草稿，强调输入期间重写会扰动后续输入位置。

IME 的优先级则另有明确规则：同值回写不动 selection/composition；不同值在预编辑时暂存最后一次，
待 composition 结束、最终 input 已通知后再应用。
Apple 的 [FieldComposition.swift](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/FieldComposition.swift#L1-L28)
说明这一契约；[painted 实现](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/presenter/ime.rs#L7-L88)
还绑定 NodeKey、runtime 与 composition epoch，拒绝旧流。
[painted 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/presenter/ime_tests.rs#L121-L155)
明确区分 bound_text 与 field_text，并要求应用的小写改写等 composition 结束再显示。
这是不同事实有不同所有者，不是要求再造一份可随意修改的业务模型。

数字宿主差异需要按版本解释：

- [Open issue](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/20261110-number-field-is-plain-text-on-painted-hosts.md)
  的行号基线是 `dd6f5dcd`，记录临时 Linux CPU 测试；该记录明确没有运行 Windows、Android 或 Chrome。
- 当前 [ControlKind](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/control.rs#L13-L45)
  无 Number；[runner 载荷分支](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/control.rs#L26-L38)
  允许非 Control 的文本编辑；[field_key:989](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/linux/src/presenter/field.rs#L989-L1072)
  可见通用 TextInput 路径，没有按 number/min/max/step 做步进的分支。当前静态线索与旧记录方向一致，仍不是本轮复现。
- Apple 当前有独立 [NumberStepping.swift](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/NumberStepping.swift#L12-L119)：
  非 marked text 的数字字符策略、Decimal 步进、边界处理和提交 clamp；
  [Mac 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Tests/ExactKitTests/NumberStepperMacTests.swift#L194-L225)
  要求 stepper 发出 input 再 change。
  clamp 的注释也明确指出浏览器将超界交给校验；这本身是产品政策差异，不能把 HTML 名称当成完全相同的提交算法。

**对提案 §6、§7.1、§8.2 的修订：**在实施前明确“外部接受值、编辑草稿、selection、composition”的冲突政策，
规定 Enter、blur、Save、step、AX 数值动作与应用拒绝的顺序。
共享纯数字解析、步进、范围与提交政策；宿主实现 OS 编辑和 IME 协议。
不要把 Apple 的字符过滤搬成 Octara 通用文本规则：Octara
[number_field](../../crates/octara-kit/src/range.rs#L170)有意保留空串、符号、指数及非法草稿，
这与 Exact 的某些 host 过滤政策并不等价。
键盘、Inspector、AX 动作应走同一可解释的数值意图与接受规则；有效性结果由业务派生并同时控制字段语义与 Save。
现有 [FieldInfo / field](../../crates/octara-kit/src/forms.rs#L8)已能承载这类组合，不需要全局表单引擎。

## 5. 样式覆盖还可能影响原生控件资格，不能只审查最终颜色

Exact named style 是一份常量属性集合，节点 own 属性覆盖它；切换 class 时，另一支没有的行必须清除。
[styles 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/cli/tests/it/styles.rs#L20-L102)
覆盖 own 覆盖及切回默认，[动态装饰测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/cli/tests/it/styles.rs#L1471-L1495)
也检查清除旧行。这不证明 Tailwind 的 class 字符顺序、CSS specificity 与 Exact 的顺序相同。

更关键的是 [button_appearance](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/lower/src/buttons.rs#L50-L147)：
编译器读取 class 与 own、内容形状及相关动画声明，决定是否可用原生按钮；
显式 appearance=auto 时不合法会拒绝，默认模式可能选择 bare。
[输入契约](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md#input-events-and-commands)
也说明 background、border、radius 及条件分支会影响原生字段资格。
因此“换样式仍使用相同语义行为”需要检查实现路径、测量和 AX，不能只看属性合并后的值。

Exact 的原生按钮缓存还检查 env、子树身份/种类、props、style、继承样式、段落 stamp 与图片 intrinsic，
见 [ButtonInputs::matches](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/arena/button.rs#L5-L55)。
同一节点身份、同一文本都不足以保证可复用所有派生结果。

**对提案 §6、§8.3、§9 的修订：**为默认外观、变体、调用者覆盖、条件撤销和继承定义一套解析规则；
若平台会在 native/painted 之间选择，Inspector 应解释选择与限制，验收身份、输入、测量与语义连续性。
无需引入完整 CSS 级联、字符串 class 语言或另一套主题状态机。
动态原生路径是否能无损切换应独立论证，不能由保留节点 id 推定。

## 6. 写前验证与失败后禁发值得借鉴，VM 不是这些保证的前提

Exact 的 [普通 apply](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/kernel/document.rs#L28-L82)
在整批写入前验证；拒绝保持原状。
[多阶段 document commit](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/kernel/document.rs#L85-L153)
仅承诺每阶段写前验证，后阶段失败不回滚此前阶段，且 finish 被拒绝，调用者必须停止使用并 reset/re-snapshot。
[continuation 测试](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/txn/tests.rs#L555-L612)
明确检查前阶段的 height=30 仍在，但 epoch/receipt 未发布且不能继续。
“没有发布新 receipt”不是“整个系统任意状态均已回滚”，文档注释也明确要求在 finish 前不得布局或呈现。

VM 另外提供有限读取集合和写集合：
[Env](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/vm.rs#L94-L136)
列出求值输入，[Outcome / allowed_writes](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/vm.rs#L269-L286)
把结果、slot writes 和 commands 分离。
这解释了 Exact 可以静态限制/计量的范围；普通 Rust 闭包不能因此获得可中断执行和任意副作用回滚。

| 值得迁移的验证义务 | Rust + Signals 中的最小归属 | 不需要的装置 |
| --- | --- | --- |
| 内容种类、唯一部件、合法父子关系 | builder/组件入口的早诊断 + 挂载/内部写入口的校验 | 通用 schema DSL、每种状态的泛型参数 |
| 数字范围、事件载荷、禁用/隐藏后的动作资格 | 共享小函数与现有事件入口；调用业务后重新确认目标存活 | 第二个 kit 调度器、VM |
| Observe/Build/执行能力及 scope 存活 | 既有响应式执行政策与 App 入口 | 模拟 VM 全部读写日志、任意 Rust 事务 |
| 外部输入和旧目标 | 现有 typed 请求、代次、window/scope 身份验证 | 把名字当身份、全局 UUID/Revision 框架 |
| 失败后的发布资格 | App/Window 更新出口共同检查；框架释放仍有途径 | 为回滚而复制每帧整棵应用树 |
| 派生缓存正确性 | 每个拥有者记录实际资源/内容/继承依赖 | 把节点身份或单一全局 epoch 当全部有效性依据 |

**对提案 §3.2、§7.3 的修订：**具体写清失败后哪些输出被冻结、哪些清理能继续，
让 native placement、绘制、AX 与 Inspector 都遵守同一发布资格。
保留现有 [App 失败准入](../../crates/octara/src/app.rs#L303)和
[prepare_window 发布位置](../../crates/octara/src/app/frame.rs#L112)继续收敛；不因借鉴 Exact 而先引入 VM、wire batch 或公开事务协议。

## 最小验收案例

以下是后续切片的验收范围，不是本轮新增测试或通过记录。先核对框架权威事实，再补真实目标宿主输入、AX 和截图证据。

| 案例 | 必须观察的结果 | 主要验证边界 |
| --- | --- | --- |
| 内容与类型擦除 | 原子输入/图片加入应用 children 明确拒绝；合法容器、富文本链接及动态内容可挂载；转换成 View 后不能绕过 | 公开能力与内部校验；不把具体类型写法当验收结果 |
| 链接替换 | 富文本有两个链接时增删/替换其中一个；无效范围不挂载；被替换对象的旧操作目标不能误触新对象 | 文本所有者、内部身份、交互释放 |
| 组合按钮 | 动态文字+装饰图标保持一个焦点/动作；独立 heading 的树位置、名称、禁用与展开状态符合约定；隐藏后代不漏出 | 控件名称与后代投影；真实宿主 AX |
| Field 与可替换外观 | 帮助/错误变化不重建编辑器；disabled、input、label 落到实际 control；条件样式撤销后不残留旧值 | 部件关系、身份、样式清除与宿主选择 |
| IME 与受控回写 | 输入中文/日文预编辑，应用同值回写和变换回写分别测试；组合中的 Enter/方向键、取消、最终 input 顺序有明确结果 | 编辑文档与宿主协议；禁止靠重挂载结束冲突 |
| 数字与 Save | 空串、`-`、`1e`、完整越界值、步进、AX 数值输入、Model 拒绝/变换逐项检查；blur 先于 Save 时仍使用明确接受规则 | kit 纯规则、业务校验、编辑草稿和接受值 |
| 失败发布 | 更新先修改部分文档，后续诊断/回调 panic；不得发布混合的新绘制/AX/native placement，后续普通动作拒绝，框架清理可推进 | 执行资格与输出发布；不要求撤销外部 Rust 副作用 |

优先修订的是保证边界与验收案例。Exact 的证据支持共享规则、早诊断、独立语义投影和失败后禁发，
不足以证明 Octara 需要公开类型状态体系、完整 CSS、库出口静态折叠器、VM 或新的表单/控件执行框架。
