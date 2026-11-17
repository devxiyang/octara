# UI 架构与基础契约调研：SwiftUI、Rune、Exact

日期：2026-11-17。用途：支撑[从内核到 ui-kit 的审查](../design/ui-foundation-review.md)。
本文记录证据与取舍，不是框架排名，也不把参考项目的公开问题当作已在 Octara 复现的问题。
全局重设计见[分层架构提案](../design/layered-architecture-redesign.md)；第 5 节补充架构级对照。
React/Base UI/Tailwind 的后续想法与 Rune/Exact 的支持证据、反例和修订见[前端语义交叉审查](frontend-native-cross-review.md)。

## 1. 来源与版本

| 来源 | 本轮查阅范围 | 限制 |
| --- | --- | --- |
| SwiftUI | Apple 官方组件/环境文档，以及身份、依赖、性能的 WWDC 架构说明 | 在线公开契约；未运行 SwiftUI 对照程序，不推测私有实现 |
| Rune | 本地源码 `3686e94ae09545fd5f2a61e22aabfbeba1cd896f` | 本地快照，未更新、未运行，不称其为最新上游 |
| Exact | 本地源码 `08c889a9de21a30b7c3b308422fc588cf1a480d3` | 本地快照，未更新、未运行；issue 描述的旧版本与当前快照须区分 |
| Octara | `b117aeea644d2480ff62be0c4a004ac7c7ed0fbc` | 当前源码、既有测试及之前已留存的 Chat 验证结果 |

Rune 本地目录：`/Users/devxiyang/code/archive/reference/rune`；
Exact 本地目录：`/Users/devxiyang/code/archive/reference/exact`。
下文 GitHub 链接固定到实际阅读的提交，便于定位；本轮源码分析使用本地副本。

## 2. 按底层问题比较机制

### 控件行为、视觉标签与可访问名称

SwiftUI 的 `Button(action:label:)` 将动作与标签视图分开。标签可以是组合内容，
不需要把一个文本节点同时当作整个按钮布局容器。
来源：[Apple Button 文档](https://developer.apple.com/documentation/swiftui/button/init(action:label:))。

Rune 的 Button 保存可访问标签，并提供 `icon` 和 `label_with`。自定义布局得到按钮字体与颜色，
不会因为视觉标签换成复杂结构而丢掉按钮名称。其测试明确覆盖自定义标签的颜色、宽度和名称。
来源：[actions.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/actions.rs)、
[button_label.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/tests/button_label.rs)。

Exact 的按钮实现会检查内容子树及继承样式等输入；其历史问题记录还涉及按钮后代的标题语义在
macOS 与 web 上不一致。该记录说明“按钮外观正确”和“按钮内部语义正确”是不同验收项。
来源：[button.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/arena/button.rs)、
[按钮内部标题问题记录](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261010-macos-a-heading-inside-a-button.md)。

**适合借鉴：**一个交互根，标签布局可组合，名称来源明确，内容更新不改变控件身份。
**不照搬：**SwiftUI 的整套视图更新与泛型体系、Rune 的整套 builder API、Exact 的平台标题提取系统。
Octara 先解决文本公开子节点与内部链接节点的所有权冲突，再选择按钮表层写法。

### 编辑草稿、可选数字与提交

SwiftUI 的格式化 TextField 支持绑定可选值。官方说明解析失败时可选绑定变为 nil。
这对一些应用方便，但如果直接将它当作 Octara 的全部数字模型，会混淆“用户清空”和“非法草稿”。
来源：[Apple 可选格式化输入](https://developer.apple.com/documentation/swiftui/textfield/init(_:value:format:prompt:)-4g6if)。

Exact 的 number-field recipe 保留原始字符串、派生解析结果，把规范化放到提交/失焦；
它特别说明输入过程中改写文本会扰动接下来的输入位置。
来源：[number-field recipe](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/recipes/number-field.md)。

同一快照中的问题记录指出，某些 painted host 曾未兑现数字字段的步进和语义行为。
这是参考项目记录的历史/宿主问题，本轮未运行验证其现状。
来源：[数字字段跨宿主问题记录](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/20261110-number-field-is-plain-text-on-painted-hosts.md)。

**适合借鉴：**输入草稿与业务值分离，提交时机明确，键盘、语义输入和宿主行为共同验收。
**不照搬：**解析失败统一变成空值；新增一个全局表单执行器；只设置 SpinButton 角色就宣称完成数字输入。
Octara 现有 `number_field`、`field`、`FieldInfo` 和事件层已覆盖主要机制，应先正确组合它们。

### 文本对齐和几何

SwiftUI 明确区分文本内部的多行对齐与容器对整个 Text 的对齐。
来源：[Apple multilineTextAlignment](https://developer.apple.com/documentation/swiftui/environmentvalues/multilinetextalignment)。

Rune 的文本样式包含对齐，文本布局调用布局引擎执行对齐。
来源：[el/text.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/text.rs)、
[text.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/text.rs)。

**适合借鉴：**把对齐作为文本布局事实，与视觉行、字形位置和相关缓存共同设计。
**不照搬：**仅平移画出的文字，或用 `items_center` 代替段落对齐。
本轮未对参考项目的 RTL、选择、链接或辅助技术做完整运行验收；Octara 必须自行验证这些契约。

### 输入提示与名称

SwiftUI TextField 明确区分编辑 text、prompt 和 label。
来源：[Apple TextField](https://developer.apple.com/documentation/swiftui/textfield/init(text:prompt:label:))。

Rune 的 InputSpec 将编辑器状态、placeholder 和 placeholder_color 分开存储。
来源：[el/input.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/el/input.rs)。

**适合借鉴：**提示不进入编辑值，稳定名称不依赖提示当前是否可见。
**不照搬：**为达到视觉效果把提示写进 value，或新增一个独立输入组件规避现有编辑器。

### 环境、主题与初始化

SwiftUI 的 environment 修改作用于当前视图及其后代，不影响范围外的视图；专门的修饰符可以表达常用环境值。
来源：[Apple environment](https://developer.apple.com/documentation/swiftui/view/environment(_:_:))。

Rune 在 Runtime 构造时接收 Env 和可选的 TextSystem；Exact 用主题资源与 token 表达品牌，
支持运行期间的 scheme 变更。它们提供了“环境/资源是显式输入”的设计参考。
来源：[Rune runtime.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime.rs)、
[Rune Env](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/cx.rs)、
[Exact theme recipe](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/recipes/theme.md)。

**适合借鉴：**在首个视图构建前明确初始资源，把运行期间修改纳入已有执行政策。
**不照搬：**把 App 全局主题一夜之间改成完整的子树环境系统；把跨平台动态环境、主题解析语言或 VM
作为修复启动体验的前置条件。Rune 的 Env 也不能直接等同于 Octara Theme 或 SwiftUI EnvironmentValues。

## 3. 机制取舍汇总

| 机制 | 本轮建议 | 在 Octara 的归属 |
| --- | --- | --- |
| 组合标签与交互根分离 | 采用原则，签名待设计 | 内容/语义契约 → View → kit |
| 显式可访问名称，视觉标签可自定义 | 采用 | 共享语义投影 + kit |
| 编辑草稿与提交值分离 | 保留现有设计 | 编辑器 + NumberField + 业务校验 |
| 段落对齐进入共享布局 | 采用原则，需完整几何验收 | 文本内核与样式原语 |
| prompt 与 value、label 分离 | 采用 | 输入原语与语义投影 |
| 首屏环境显式输入 | 采用，限定资源与宿主边界 | App / Shell |
| 完整 SwiftUI 环境/更新模型 | 不采用 | 当前没有足够需求证据 |
| Exact VM、主题语言、宿主控件体系整体迁移 | 不采用 | 与本轮问题不匹配 |
| 另一套表单状态机或 kit 调度器 | 不采用 | 现有 Signals、事件层和作用域可承载 |

## 4. 调研尚未证明的事情

- 参考框架在所有宿主上的行为完全一致。
- Octara 所有共享资源入口都存在同样的错误后果。
- 增加一个按钮函数或修改返回类型就能解决内容所有权与名称问题。
- 不修改下层契约，仅重写 Chat 或加一组组件样式，就能完成这轮收敛。

下一轮应以主审查文档的 F1/F4 为起点，先用针对性证据确认底层契约，再向上设计可组合原语和 kit。

## 5. 全局架构对照

补充于 2026-11-17。本节比较架构机制，不以三者的总模块数或公共类型数量评价复杂度。
SwiftUI 依据公开说明；Rune/Exact 仍使用第 1 节固定的本地快照，没有更新上游或运行对照程序。

### SwiftUI：身份、依赖和环境是不同维度

Apple 将视图值与视图身份区分：值可以重复生成，持久状态关联身份的生命周期。
它区分显式身份和结构身份；依赖决定哪些视图需要更新。
因此，结构变化导致状态重建，和某属性变化使显示失效，是不同事件。
来源：[Demystify SwiftUI，WWDC21](https://developer.apple.com/videos/play/wwdc2021/10022/)。

Apple 的性能说明强调缩小依赖范围，也提醒不能机械地把每项数据都拆成独立视图。
来源：[Demystify SwiftUI performance，WWDC23](https://developer.apple.com/videos/play/wwdc2023/10160/)。

2025 年的工具说明进一步展示：环境变更可能通知多个读取环境的视图，即使检查后某个 body 无需运行，
检查仍有成本；不建议将高频几何或时钟值放入环境。
来源：[Optimize SwiftUI performance with Instruments，WWDC25](https://developer.apple.com/videos/play/wwdc2025/306/)。

**Octara 的选择：**明确结构 key、运行中目标身份、依赖有效性，保留现有 Signals 精细绑定。
借鉴依赖局部化和身份连续性，不复制 SwiftUI 的 body 重求值模型；不引入承载全部数据的大环境对象。
这些是从公开机制得到的设计推论，不是对 SwiftUI 私有 AttributeGraph 实现的审计。

### Rune：测量和提交的能力边界，复用的完整条件

Rune core README 描述普通 Rust 状态驱动 view，事件处理拿到可变状态，随后重新构建并提交视图；
hover/press 可以仅改变展示轨迹。它的 `States` 与 `StatesRef` 则区分可变与只读状态访问。
来源：[core README](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/README.md)、
[states.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/states.rs)。

`runtime/commit.rs` 的 `Measurement` 持有测量所需资源，不持有 Scene 或可变应用状态；
测量完成得到 `Measured`，再交给最终 apply。重试有单独 Pass 和边界，
例如 reveal 与行驻留策略不把每次重试当作首次提交。
来源：[commit.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit.rs)。

子树复用检查 memo 输入、根修饰和继承状态；继承状态包括方向、inert、scale、前景色等。
“相同节点身份”本身不意味着可以复用所有结果。
来源：[memo_reuse.rs](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/memo_reuse.rs)。

**Octara 的选择：**用私有类型/函数限制测量和提交可做的事；缓存声明真实依赖；
几何重试不能重复业务动作或动画启动。不照搬完整视图重建、memo 子树搬运或 `Tx` API，
也不将 Rune 的强制全量测量退路直接用于 Octara 大型虚拟列表。

### Exact：生产者、文档内核、宿主各自有契约

Exact 的 Contract 文档描述编译 UI/交互声明，并将网络、存储和较重计算放入数据模块。
VM 的 `Env` 列出求值可读取的输入；它与普通 Rust 闭包的可执行能力有本质区别。
来源：[Contract guide](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/contract-for-humans.md)、
[vm.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/vm.rs)。

`kernel/document.rs` 有两项应分清的保证：普通 apply 的 batch 在写入前验证；
多阶段 document commit 则逐阶段验证，后续阶段失败会使提交无法完成，先前阶段不保证回滚。
提交完成前不得呈现这份中间文档。
来源：[document.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/kernel/document.rs)。

按钮内容复用还检查环境、子树、样式、属性、继承状态及 intrinsic 等因素；
结合第 2 节宿主历史问题，可见声明、缓存与宿主兑现需要分别验证。
来源：[button.rs](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/arena/button.rs)。

**Octara 的选择：**明确写入前的验证、失败后的发布禁止、宿主的兑现证据。
不引入 Exact VM、字节码、整批写日志或独立 UI 语言，也不承诺任意 Rust 操作具有事务原子性。

### 逐层取舍

| Octara 层 | SwiftUI 的参考 | Rune 的参考 | Exact 的参考 | 本提案决定 |
| --- | --- | --- | --- | --- |
| 状态/执行 | 身份与依赖分离 | 只读/可写访问边界 | 求值输入与执行结果分离 | Signals 不变；统一已有权限政策 |
| App/资源 | 环境作用范围及更新成本 | 初始 Env/TextSystem | 显式主题和宿主环境 | App 单一修改所有者，启动前配置 |
| 文档/结构 | 身份决定状态连续性 | keyed 复用与继承条件 | 文档验证和提交边界 | 容器/原子内容，内部链接归文本 |
| 测量/输出 | 数据依赖决定更新 | Measurement → Measured → apply | 未完成提交不能呈现 | 统一几何，有界重试，区分就绪和呈现 |
| 原语/kit | 动作与 Label 组合 | 可组合标签且保留名称 | 语义控件与宿主契约 | 一个交互根，组合配置目标明确 |
| 工具/验收 | 更新原因可观察 | 本节未完整审计工具层 | 历史宿主问题提供反例 | 直接使用 Octara 权威状态与真实窗口证据 |

这不是将三套架构拼接起来。它们帮助我们明确边界，具体机制仍由 Rust + Signals、保留文档、
既有平台和 Agent 接口承担。未读到或未运行的部分不推定为参考框架已经解决。
