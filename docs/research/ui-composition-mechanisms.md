# UI 组合层深入调研：身份、内容、所有权与行为

日期：2026-11-17。目的：检验而非证明已有重设计草案。
本轮没有修改生产实现、运行参考框架测试或进行 OS 验收。
论文阅读范围为下文列出的正文部分，不声称复核全部形式证明。

## 结论先行

现有证据支持保留 Signals 与 retained tree，但**不支持现在就把 `scoped`、单一万能内容入口、
或新的动作装配函数定为最终公开 API**。先前方案应降为候选。

需要先定义五类事实：构建描述、挂载身份、响应依赖、资源所有权、呈现/语义关系。
它们可以共享内部表示，但不能因为都像“树”就认为父子关系完全相同。
例如弹层改变呈现位置而不改变逻辑所有者；来源标记不等于状态 owner；
Rust 捕获变量不等于动态读取 provider。

推荐保留的原则：普通 UI 可组合、原子编辑不可拆散、状态更新不靠组件全文重跑、
生命周期由框架收尾、动作多入口共用资格检查。暂缓决定具体 scope 语法和行为装配的公开形态。

进一步推演见第 10 节：A/B 都预设“所有局部状态必须按名字复用”，这个前提也需要审查。
其中 C 比较普通函数组合、新状态每次调用独立分配、运行时边界负责释放的组合方案。
这不是新的 Signals 引擎；它会改变状态构造、context 和热重载的公开契约，不能当作无成本删减。
特别是，C 不承诺丢弃已经执行过的构建函数的返回值可以撤销其副作用。
用户随后明确 AI 是框架的主要代码作者，并选择学习 Rune 的函数组合方式；第 11 节记录这一主线。
普通函数组合已经选定，C 中的匿名分配和恢复政策仍未定型，不能把两者捆绑批准。

## 证据方法与版本

- Octara：本地 `d5619e8` 及工作区；明确区分未提交的按钮草案。
- Rune：本地 `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`。
- Exact：本地 `08c889a9de21a30b7c3b308422fc588cf1a480d3`。
- React、Solid、SwiftUI、Base UI、Radix、Leptos：官方资料，访问日期为本文件日期。
- Solid 官方网站的不同页面与 GitHub 缓存存在版本跨度；不把未固定版本的文档作为 Solid 2
  最新实现证明。明确记录版本的 issue 可用于历史案例。没有确认其最新稳定版本号。
- 下列外部测试均是**阅读源码中的断言**，并非本轮实测结果。源码警告也不等于已经发生过线上事故。

## 1. 组件是否应该等于一个状态作用域？

### Rune：普通函数简洁，但更新和缓存有自己的成本

`crates/rune/src/tooling.rs::AppSpec` 用普通状态 S 和 `fn(&S, &mut Cx<S>) -> El` 表达视图。
`memo.rs` 明确说明 view 在 commit 时重新运行，memo 用 id 和 version 复用子树。
缓存会保留旧 handler；变化的捕获值必须纳入 version，列表事件应捕获稳定 id 而非位置。
环境变化、几何读取与热重载也需要各自的失效规则。

**可学**：普通函数与普通数据不需要组件类体系，按实际需求选择优化边界。
**不可推出**：Octara 也可以不管理局部 Signals 的生命周期，或复制 Rune 的 memo 签名就能安全缓存。
Signals 的依赖模型不同，不能把显式版本缓存与自动依赖追踪并列成两套默认更新系统。

### Exact：编译期展开并未消除状态实例

`contract/syntax/src/inline.rs` 把组件 view 展开，重命名声明避免捕获；`Owner` 仍区分根、实例和 region arm。
`contract/cli/tests/it/child_state_lifetime.rs` 断言：初始化可读取已结算输入；同一实例后续输入变化不重复初始化；
移除分支后状态消失，重新显示使用当时输入初始化。`instance_work.rs` 还断言行内状态变化不能被缓存遮住。

**可学**：作者组件、执行表示和状态 owner 可以不同；初始化与持续绑定要有不同语义。
**不可推出**：Rust 框架需要编译器 plan，或显式 key 必须出现在每个普通组件调用中。

### React / SwiftUI：身份有稳定规则，但不会免费消除复杂度

React 将 state 关联到渲染树中的位置，类型和 key 影响保持/重置；同一描述被渲染在两处可形成不同实例。
这与 Octara 的一次性 View 不等价。[React 状态身份](https://react.dev/learn/preserving-and-resetting-state)

SwiftUI 区分短暂 View 值和持续身份，状态生存期依附身份。结构分支和不稳定 ID 会改变该生存期。
借鉴的是规则明确，不是让 Rust 描述值的 Drop 等同于卸载。[Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/)

### 对 Octara 的新发现

`reactive/graph/scope.rs::named_state` 在同一 scope 重复同名 state 时报告诊断；若类型相同且已有值有效，
仍返回已有 signal。因此，把 `state("count", ...)` 放进普通函数再调用两次，不能假定它自动得到两个实例。
这是源码路径推演，不是本轮新运行复现。

显式边界方案降低宏成本，却把“何时必须创建边界”的判断交给作者；统一组件入口可减少这种误用，
但会增加构建延迟、返回类型和身份生成的设计成本。两者都应进入实验，不能凭 Rune 的简单调用形式裁决。

## 2. children 到底是什么：值、描述、工厂还是已创建对象？

React 的元素是描述；Solid 的 children 处理涉及解析嵌套数组、fragment 和访问器，并缓存解析结果以免重复创建。
`children` 不是语法糖即可解释的无成本操作。[Solid children](https://docs.solidjs.com/reference/component-apis/children)

Exact 的 slot 在调用者词法/provider 环境展开，但每个 children 出现位置创建自己的内容实例，
状态属于其外围 region。`compose.rs` 与 `action_related.rs` 检查 provider 穿过包装及转发的规则。
它在编译期实现，不能直接用一个运行时“捕获 owner”替代。

**对 Octara 的结论**：

- 一次性描述、可重复工厂应在契约中区分；不能通过 Clone 为有状态描述赋予模糊复用语义。
- “所有内容都接受 IntoElement”只能解决类型转换，不能自动解决求值时机。
- 空内容、fragment、延迟分支应共用挂载表示，但不能要求所有输入先被迫解析成节点再检查是否为空。
- 显示条件为 false 时是否执行内容、一个工厂执行几次、未挂载描述的构建副作用如何结束，都必须定义。

## 3. context、owner、依赖收集必须分别推理

React context 读取逻辑组件树中最近 provider；provider 改变会更新消费者。
这并非调用者源代码处的 lexical provider，也不是 Octara 当前“替换 provider 只影响后续查找”的语义。
[React useContext](https://react.dev/reference/react/useContext)

Solid 的 runWithOwner 资料说明恢复 owner 不等于恢复依赖 listener，且异步 await 后不会继续保留同步上下文。
该页面正文请求曾返回 404，本轮取得的是官方页面搜索快照，只用来交叉印证概念，不据此锁定 2.x 实现。
[Solid runWithOwner](https://docs.solidjs.com/reference/reactive-utilities/run-with-owner)

`createRoot` 官方文档强调 disposal，但搜索还发现 2.x 的 root 归属变更线索；相关 PR 原文未成功取得，
因此本轮不把“所有 Solid 版本都是 detached root”或其相反说法作为结论。
[Solid createRoot](https://docs.solidjs.com/reference/reactive-utilities/create-root)

**对 Octara 的结论**：已有 owner 恢复、运行资格与依赖 collector 分离值得保留。
不能让“恢复上下文”暗中重新授权退休实例，也不能为了孩子获得 theme 而恢复旧 collector。
context 方案至少要比较：实际构建时 lookup，以及显式 provider 内容工厂；目前没有充分证据证明只保留
任意位置调用 provide 的方式最易用。构建时 lookup 可作为基线，尚不作为最终 UX 裁决。

### Portal 的反例

React portal 改变 DOM 位置，但 context 与事件传播仍沿 React 树；换目标 DOM 节点会重建内容。
[React createPortal](https://react.dev/reference/react-dom/createPortal)

因此，Octara overlay 不应从视觉父节点推导状态 owner。不过“逻辑 owner 保留”也不能直接推出
事件应该冒泡到锚点：锚定关系、逻辑归属、事件路由各自需要契约，尤其要防止点击面板重复激活触发器。

## 4. 行为组合的行业实践与代价

Radix asChild 通过把 props 和行为交给子元素实现复用，自定义叶组件必须转发 props 和 ref，
更换底层元素时还要自行保证焦点与可访问性。[Radix Composition](https://www.radix-ui.com/primitives/docs/guides/composition)

Base UI render 合并到同一元素，但属性、样式、ref 和事件有不同规则；事件有明确顺序与阻断方式，
组件自身读取的 disabled 也不能简单由内层属性覆盖。[Base UI Composition](https://base-ui.com/react/handbook/composition)

React 官方也提示 cloneElement 会使数据流难以追踪，并列出显式 render prop/context 等替代。
[React cloneElement](https://react.dev/reference/react/cloneElement)

**对 Octara 的结论**：优先显式拥有一个实际交互节点，不照搬任意子树注入属性。
一个“装配按钮语义”的函数仍须回答：是否覆盖既有 role/name、多个 handler 的顺序、谁能取消默认动作、
是否允许在装配后覆盖资格。若这些问题只能靠不断加特殊规则解决，应比较直接的无外观语义构造入口。
无外观 Button 构造不等于新增 kernel Button 类型，不应仅为了表面上“只有 div”而排除它。

### 原子动作内容的约束

HTML button 的内容模型禁止交互后代和带 tabindex 的后代；这是结构约束。
[WHATWG button](https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element)
WAI 的 Button Pattern 分别规定名称、Enter/Space 激活、禁用语义以及动作后的焦点政策。
[WAI Button Pattern](https://www.w3.org/WAI/ARIA/apg/patterns/button/)

映射到 native UI 时应保留单动作语义与真实焦点保证，不照搬 HTML 的全部内容分类。
AX 隐藏不是禁止交互；静态构建检查也不覆盖后续插入。需要比较统一动态验证与限制内容入口的成本，
并明确检测后是拒绝挂载还是拒绝激活；不能仅发警告后留下语义与操作不一致的树。

## 5. 论文提供的理论基线

### Incremental Computation with Names（OOPSLA 2015，扩展版 2021）

阅读正文 §1、§2.4、§2.5 及 §4.3 的结论。论文以 names 控制跨次计算的复用，同时需要 double-use
检查保证正确性，并证明其 calculus 的 from-scratch consistency。
[论文正文](https://arxiv.org/html/1503.07792v6)

启发：身份应控制复用，重复命名不是仅影响性能的细节。我们可比较增量与参考求值的结果，
但不能把论文证明套在 Octara，也不能用全新 UI 重建后的编辑状态去比较一个应保留选择的交互过程。
静态投影一致性与交互身份连续性需两组 oracle。

### On the Expressive Power of Hook-Indexed State（2026 预印本）

阅读扩展版摘要、§1–2 的动机与贡献。论文给出 hook-indexed state 的 calculus 和类型系统，
将其翻译为引用式状态并证明类型与语义对应；同时承认翻译存在代码重复与抽象损失。
[论文 PDF](https://arxiv.org/pdf/2610.32925)

启发：不能把“能用普通引用实现”误读为“不需要状态抽象”；也无需为了熟悉感引入按调用序号的 hooks。
本轮没有核验全部 117 页证明，没有据此宣称某种 Rust API 最优。

### Willow：Temporal Dependency Analysis（2026 预印本）

阅读正文 §2、§7 及 scheduler 限制，关注 debounce/网络反馈案例。论文将注册、触发、取消与卸载
纳入时序 effect，并给出原型检查器；其模型以 render 为核心，不证明外部调度器按时完成。
[论文正文](https://arxiv.org/html/2607.27074v1)

启发：按时间线验证建图、事件、结算、取消和退休后的回复，比仅比较最终截图更有价值。
不建议为 Octara 引入时序类型系统；先将这些边界变成可执行的场景轨迹。

## 6. 已记录的问题与反例，而非笼统的“成熟框架经验”

| 来源 | 已观察的证据 | Octara 应避免的设计错误 |
| --- | --- | --- |
| Solid 2.0 rc.8 issue #3500 | signal/store 的 root 写权限不同；维护者确认并按更严格规则修复 | 同一权限散落在不同 setter；debug/release 差异未经明确设计 |
| Rune memo 文档 | 缓存 handler 持有旧捕获，版本必须包含变化输入 | 按视觉相等复用，却让回调读旧业务值 |
| Exact lifecycle 测试 | 初始化不随参数更新重跑；离开分支再进入重新初始化 | 把初始化和绑定混成一种 props 更新机制 |
| Exact instance_work 测试 | 缓存不可遮住行私有状态、时钟和全局依赖变化 | 只比较列表 item 就跳过全部后代工作 |
| Radix / Base UI 文档 | 转发、覆盖与取消规则必不可少 | 看似自由的行为组合，实际靠未记录的特殊顺序 |
| React 官方身份文档 | key/type/位置改变可重置状态 | 用每次生成的新 ID 表示持久实例 |

Solid 案例有明确版本和维护者响应，关闭日期 2026-09-17；不能描述为当前仍未修复：
[issue #3500](https://github.com/solidjs/solid/issues/3500)。其余文档警告和测试断言是契约证据，
不冒充历史线上事故统计。

## 7. 下一步应做的区分性实验

先在文档中给候选接口写相同输入与预期轨迹，再决定是否制作隔离原型。不能先修改核心再倒推结论。

| 场景 | 必须可观察的结果 | 区分什么方案 |
| --- | --- | --- |
| 同函数两个私有 state，各置于可独立移除的分支 | 值独立，移除一个只取消其工作 | 命名复用与新分配；组件边界与动态分支边界 |
| 包装函数增加/删除 | 仅外观包装不意外改变状态身份 | 函数调用是否等于实例；key 归属 |
| 预建内容 vs 延迟内容置于 provider | 明确读值、创建时点与释放所有者 | 内容值/工厂，以及 lookup/捕获规则 |
| 条件 false，内容含任务 | 未呈现时是否创建任务必须有唯一答案 | eager 内容的副作用与描述纯度 |
| 同内容呈现两次 | 明确共享描述还是两个实例；禁止复用已挂载对象 | 一次性值 vs 可重复工厂 |
| 行重排、移除、同 key 再插入 | 重排保留；退休旧回调不可作用于新实例 | 逻辑 key vs 代次资格 |
| portal 面板内按键/点击 | context 正确、焦点正确、不重复触发锚点 | 呈现树与事件/owner 关系 |
| Button 内动态加入输入 | 明确拒绝结果，AX 与实际操作一致 | 内容约束执行时机 |
| 两层行为取消/禁用 | 顺序、默认动作、应用观察值确定 | 行为装配是否真的比语义构造简单 |
| 组件描述构建但未挂载 | 无悬挂工作，释放次数明确 | 构建副作用与 owner 建立时机 |
| 同一输入多次更新后迟到回复 | 已退休工作不可发布，编辑器不重建 | 生命周期与事件时序 |
| 普通列表 vs 增量列表 | 投影一致，身份分别按契约验证 | 优化是否增加可见语义差异 |

Rune `runtime/memo_reuse_tests.rs` 对缓存/非缓存路径比较节点、命中、焦点和输出，提供了差分验证实践。
我们借鉴验证方式，不移植缓存系统。实际测量项目应限于构建次数、挂载/释放次数、更新节点数；
没有必要先做极端吞吐优化。

## 8. 对重设计草案的修订建议

1. 保留五类事实的边界，暂不为其新增公开对象。
2. 显式 scope 与统一实例入口是保留全面命名 state 前提下的对照；第 10 节进一步比较解除这个前提的 C。
   `scoped` 名称不定稿，不能先把每个组件都变成实例，再研究是否真的需要实例。
3. 内容转换统一不等于求值时机统一。先定预建值、延迟工厂和未挂载描述的契约。
4. 不预选“按钮行为装配”胜过“无外观语义构造”；两者都落在既有容器与事件系统上。
5. 暂缓批量删除原语；先删除已确认的重复路径，保留编辑、身份资格与释放协议。
6. 以本节实验完成情况决定迁移，不再用又一篇架构文档代替接口验证。

本轮产出是有来源、反例与适用边界的设计依据；未解决的选项已明确保留，不声称架构定型。

## 9. Rune 式组合 + Signals：调用方式推演

继续审查发现，当前 start-here 的 `#[component] fn counter()` 直接使用命名 state，
窗口根提供了 scope，因此单实例教学可以工作；但这个宏不建立独立实例，
不能把同一示例放两次就假定状态自动隔离。这里是源码和示例的对照结论，尚未新增运行复现。
当前 skill 又要求每个构建函数都标 component，使“来源标记”和“有状态实例”的区别更难理解。

下面全部是**候选接口伪代码**，不是当前可用 API，没有编译验证。
`scoped`、新的 component 宏语义与裸按钮入口均未实现；示例用于比较调用成本。

### 对照 A：组件内部显式声明边界

```text
fn counter(initial: i32) -> View {
    scoped("Counter", move || {
        let count = state("count", || initial);
        counter_content(count)
    })
}

row().children([counter(0), counter(10)])
```

边界放在组件定义内部，调用者不必传 scope；这比每次调用 `Counter("left")` 更自然。
不过，两个同名实例如何分配默认身份仍然要由挂载规则解决，不能仅改名为 scoped。
作者遗漏内部 scoped 时仍会回到共享命名状态的问题。

### 对照 B：组件声明入口建立实例

```text
#[component]  // 候选新语义：返回延迟描述，挂载时执行正文并创建实例
fn counter(initial: i32) -> View {
    let count = state("count", || initial);
    counter_content(count)
}

row().children([counter(0), counter(10)])
```

调用方相同。区别在于 B 使组件声明本身承诺实例边界，A 要求正文手工建立。
两者都应降低到同一个既有 scope + View 挂载机制，不建立两种运行时。
若选 B，当前仅标记来源的宏必须被彻底替换，不能兼容两种行为。

B 的代价必须显式接受：正文延迟执行，参数需要拥有或满足挂载生命周期；
不能无条件捕获短借用，不能把任意 Rust 函数返回类型都偷偷改成 View。
宏须限制为清晰的组件签名并给出错误，不能继续依靠简单文本拼接处理任意参数模式。
这些实现成本尚未验证，不能因为示例简短就宣布 B 获胜。

### 两种方案都保留普通函数组合

```text
fn card(content) -> El {
    col().padding(...).background(...).child(content)
}

fn counter_content(count: Signal<i32>) -> View {
    // 只组合由调用者拥有的状态，不申请私有实例
    row().children([text(读取 count 的绑定), button(递增 count 的动作)])
}
```

card 与 counter_content 不需要生命周期宏，也不自动增加 scope。
因此推荐验证的 B 不是“所有函数都变成运行时组件”，而是：
**普通函数组合内容；组件声明表达独立实例；动态区域负责挂载与退休。**
这是作者模型，不是增加三个新的公开原语。

### 身份规则先于 key 的语法

- 静态结构中的两个实例应自动独立，不要求手工写 left/right 仅为了避免冲突。
- 动态集合必须有局部稳定 key；key 属于集合中的逻辑行，不要求再传一遍内部组件 ID。
- 同 key 重排保留实例；移除后再创建产生新代次，旧任务与 Agent target 不能复活。
- 不用源码行列作为普通运行身份；来源位置只用于诊断。热重载是否能映射旧状态是另一项契约。
- 显式重置继续通过动态结构边界表达，不把测试 id 混成重置 key。

默认静态身份可按父 region 中的声明实例位置/类型确定，但 Octara 的命名状态与热重载目前
依赖路径。正式实现前必须验证插入兄弟、重载及动态行包裹组件的规则，不凭 React 类比直接宣称等价。

### children 与 context 的区分性样例

```text
ready = read_theme_and_build_text()       // 此时已经读取外层 theme
delayed = counter(0)                     // B 下仅创建描述，未执行正文
provider(theme = dark) {
    column[ready, delayed]
}
```

候选“构建时读取”规则下，ready 保留先前读值，delayed 的正文在 provider 下面挂载时读 dark。
如果这区别难以从接口解释，需考虑显式 provider 工厂或让内容构建 API 更一致，
而不是暗中改变已创建内容。这里的 provider 也是概念表达，不是新增 API 决定。

条件内容含任务应在条件成立并挂载后才创建任务；丢弃一个尚未挂载的组件描述不能执行正文，
也不能运行正文中未注册过的 cleanup。函数参数表达式仍按 Rust 规则立即求值，
宏不能承诺使调用者传入的所有表达式都变懒。

### 按钮：两种公开形态的实际差异

```text
形态一：button_behavior(container(children), name, on_press)
形态二：button(name, on_press).child(children)  // 无默认皮肤的语义构造
```

两者都可实现为现有 Box + 同一交互协议，差别在公开承诺。
形态一需处理传入容器已有 role/焦点/handler 的冲突；形态二一开始就确定动作根，
更容易表达禁止独立交互后代的内容契约，kit 再提供样式。
目前更值得验证形态二；不能把“无 kit 可写 UI”解释成“每次必须手工拼 role 和焦点”。
也不必复制 HTML 的全部元素种类或把 button 加进 kernel Kind。

### A/B 对照的适用前提

保留“所有局部 state 都必须有名字”的前提时，B 比 A 少一次作者手工声明边界。
但这不足以决定整个架构。第 10 节撤掉该前提后，C 成为更值得验证的简化方向；
B 保留为要求未挂载描述完全不执行正文时的对照。无外观语义按钮仍独立于这个选择。

## 10. 再向下一层：组件边界是否由命名状态倒推出来？

本节是源码推演与契约选择，**不是新实现或测试通过记录**。没有修改 Signals、宏或挂载代码。
以下 `state(|| ...)`、provider 等写法均为候选伪代码，当前 API 仍是 `state(key, init)`。

### 10.1 新证据：分配、复用和释放是三个不同问题

当前 `reactive/graph/signal.rs::Signal::scoped` 已经可以在同一个 scope 下分配多个不同节点。
`graph.rs::create_node` 为每次新分配建立运行时/槽位/代次身份，并登记到所属 scope。
同 owner 并不意味着同 Signal，也不意味着必须按 Hooks 的调用序号找回状态。

`scope.rs::named_state` 则先查询 `(scope_path, key)`，存在同类型有效值就返回旧节点。
非 App scope 的重复声明会诊断，但仍可能共享同一 Signal。这服务于复用和热重载，
并不是依赖追踪或统一释放所要求的算法。

因此，先前推理“普通函数调用两次会共享 state，所以必须给函数创建组件实例”少检查了一个前提：
如果局部状态构造表示新分配，两次调用本来就可以独立，不需要为了消除名字碰撞额外建立 owner。

旧的 `Signal::new` 删除还承担了收敛 App 所有权的目的，不能简单恢复原入口。
内部 `Signal::scoped` 不具备完整的公开构造契约；新的公开分配仍必须要求有效 owner、
明确允许的执行阶段、隔离初始化的依赖收集和权限，保留失败/退休检查。
**可取消的是强制名字，不是状态的所有权。**

### 10.2 与参考框架交叉验证

Leptos 的官方生命周期章节明确区分组件树与 decision tree：组件主要是函数组织，
响应式 owner 承担嵌套工作和 arena Signal 的释放。它还直接指出两个代价：
把子 owner 的 Signal 向上保存可能产生已释放访问；在长寿命 owner 中不断分配则可能长期占用资源。
[Leptos 生命周期](https://book.leptos.dev/appendix_life_cycle.html)

这支持“函数不必等于 owner”，不等于应该照搬 Leptos 的所有 effect 重跑规则。
Octara 仍保留纯计算与结构构建的权限区别，memo 不因此获得创建任意子 UI/任务的资格。

Rune 的普通函数/普通状态证明作者模型可以不依赖组件类体系；但其 App 状态和 view 重跑机制
不能证明局部 Signal 自动安全。Exact 的 `Owner::{Root, Instance, Arm}` 及 `Fill` 则给出更直接的参照：
展开来源、词法 provider 和 region 所有权分别记录，slot 在哪里实例化会影响状态生存期。
这不是“Exact 没有实例”，而是不能把源码函数调用、呈现位置和释放边界压成同一个事实。

源码依据：[Rune memo](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/memo.rs)、
[Exact inline/Owner/Fill](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/syntax/src/inline.rs)、
[Exact lifecycle assertions](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/contract/cli/tests/it/child_state_lifetime.rs)。
这些固定版本的源码本轮再次核对，未同步远端，也未运行它们的测试。

### 10.3 候选 C：按运行时需要建立边界，普通函数直接组合

```text
fn counter(initial: i32) -> View {
    let count = state(|| initial);        // 每次实际执行都是新分配
    counter_content(count)               // 普通 Rust 函数
}

row().children([counter(0), counter(10)])  // 两个状态，不需要两个命名 scope
show(open, || counter(0))                 // 分支成立时才调用 counter
each(items, item_key, |item| counter(item.initial))
```

普通组合函数立即执行，不重复执行来传播 state 更新。root、条件分支、keyed 行、显式重置和弹层
建立具有实际存亡语义的边界；context 隔离也可以复用现有 scope，不新增另一套 owner 运行时。
仅增加一个 card/helper 函数不会新增状态身份或释放边界。两个静态 counter 可同属窗口 owner，
因为静态结构本来就不单独移除；要独立显隐，则分别放进动态分支。

这不是把 scope 删除，而是不要求作者先把“有私有 state 的函数”划成另一种组件语言。
当前 `show` 的 `switch_at` 和 `each` 行已经创建、退休 scope，可作为候选的实现基础。
是否删除组件专用边界还取决于热重载及构建副作用的选择，不能仅凭这段伪代码决定。

| 时间线 | C 的预期语义 | 身份来自哪里 |
| --- | --- | --- |
| 两次调用 counter | 分配两份值；修改左边不影响右边 | 两次新分配，不是 state 字符串 |
| 同一分支内只改计数 | 更新读取它的绑定，不重跑 counter | 已挂载的节点与 Signal |
| 条件仍为 true，再次结算 | 保持分支；不重新初始化 | 分支连续存在，不是每次求值一个新 owner |
| true → false → true | 退休旧分支及工作，再分配新状态 | 分支的两次不同生存期 |
| 列表 A/B 重排 | 移动已有行，保留行内状态与编辑连续性 | 同一集合中仍存活的业务 key |
| 删除 A，之后重新插入 A | 新行、新状态；旧任务/Agent target 不能复活 | 业务 key 相同，生存期和代次不同 |
| 把状态放到 show 外再传进去 | 状态随外层存活，隐藏只退休内层 UI/工作 | 状态的创建 owner，不随句柄转移 |
| 虚拟行离开视口 | 按行挂载契约释放局部状态；需保留的草稿存到数据模型 | 可见 UI 与业务数据寿命分开 |

最后一项也说明：不能承诺“只要列表 key 相同，任何未挂载行的全部局部状态就自动永久保存”。
若未来需要 offscreen 保活，必须单独说明存储与释放成本，不能通过无限缓存偷偷实现。

函数抽取的性质也应精确：保持执行位置和次序的普通 helper 抽取，不改变运行时所有权。
把构建从分支内移到外面会改变生存期；热重载后的源码映射也不包含在这个等价性承诺中。

### 10.4 C 的反例：普通函数不是纯描述语言

```text
let unused = widget_that_registers_a_watch();
drop(unused);                             // 未把返回内容放到 UI 中
```

普通函数已经执行，其 watch 可能已运行首次回调。返回值的 Drop 既不能撤销任意 Rust 副作用，
也不能从 Copy Signal 句柄推断哪些状态只属于这份内容。在 C 中，已经注册的工作归创建时 owner，
直到该 owner 退休或显式停止；丢弃未挂载 El 只释放描述及其捕获，不隐式退休同 owner 的其他工作。
在长寿命 owner 中反复丢弃有副作用的构建结果可能积累工作，不能称为“自动没有泄漏”。

因此不能同时承诺下面三个条件，而不增加额外机制：

1. 任意普通函数立即执行，允许直接注册有状态工作；
2. 返回的 View 未挂载时，函数正文仿佛完全没有执行；
3. 不使用显式延迟工厂、宏改写或另一套资源转移协议。

建议 C 选择诚实的 Rust 求值规则：**立即内容是已执行的组合；条件/列表/弹层内容必须经工厂延后执行**。
条件 false 时不调用其工厂，因此工厂内部不分配、不注册、不清理从未存在的工作。
预先在外面创建的值和工作不因之后被闭包捕获而改变创建时间或 owner。
不把现有 `watch` 偷偷改成挂载后才首次执行；否则同名 API 在不同调用位置会出现第二套时序。

如果产品要求所有自定义组件描述都能安全预建、任意丢弃且绝不注册工作，C 的这个形式就不合格，
应选择 B 或显式延迟组件工厂。这里是可检验的取舍，不是“宏更优雅”或“普通函数天然更简单”。

### 10.5 context 需要有内容范围，不能靠函数名称隔离

当前 `provide` 修改 active scope 的 provider 表。假如两个普通 helper 共享该 scope，
第一个 helper 的 provide 会影响第二个 helper 随后的读取；函数返回并不恢复旧 provider。
因此 C 不能只修改 state 构造而保留“组件里随便 provide 就只影响这个组件”的解释。

更明确的候选是内容范围式 provider：传入值与内容工厂，在独立 scope 下执行并挂载该内容。
scope 持续到对应内容退休，供延迟子内容、handler 和 task 恢复，而不是函数返回时就弹出临时栈。
动态 context 值仍使用 Signal；普通组件作者不需要在 handler 中反复替换 provider 身份。
App 级环境注册的用途另行保留，不把它与局部 UI 范围混为一个重载入口。

此规则按实际执行位置查找 provider：工厂内构建的消费者看到该范围；此前构建并读取的值保留旧结果。
不会把所有闭包都解释成 Exact 的编译期词法 slot，也不会移动已有 Signal 的 owner。
provider 返回的是有上下文边界的内容，不应靠事后修改任意 El 让已经执行过的后代重新读取 context。

### 10.6 热重载是 C 的真实成本，不能把它藏进调试信息

原架构已有命名状态跨重载复用的行为，不能把匿名状态默认重置当成兼容实现。
本轮尚未批准降低该保证。需要明确选择：

| 策略 | 收益 | 代价 |
| --- | --- | --- |
| 普通局部状态新分配，App 模型跨窗口重建保留；少数状态显式声明恢复身份 | 默认组件不需要名字和实例路径；业务状态归属直观 | 匿名局部状态在重载重建时重置；需要保留的局部状态要有明确映射 |
| 每个局部状态默认跨重载恢复 | 延续现有开发体验目标 | 仍需区分两次同函数调用的恢复地址；命名实例、编译器元数据或同等机制无法凭空省掉 |

源码行号、类型名、每次新生成的 UUID 都不能独立解决第二行的问题：同行多实例仍需区分，
编辑代码会移动行号，新 UUID 也不是旧状态的恢复地址。一个 `"count"` 在两个静态 counter 中同样不足。
若要求插入/删除包装之后仍自动精确保留，应另行证明映射规则，而不是静默按序号碰运气。

运行时分配身份、集合业务 key、重载恢复地址、Agent 观察到的节点代次、调试来源可以互相引用，
但不能替代对方。Agent 继续从实际挂载节点操作并检查代次；状态恢复不能让旧事件目标再次有效。
函数来源可保留为开发工具的元数据，不需要为了获取组件调用链强迫每次调用分配运行时 scope。

### 10.7 对 ui-kit 以下抽象的影响

无需因为这些关系不同就建立五套公开树或一个新的 VM。建议保持：

| 事实 | 负责者 | 不应该承担的职责 |
| --- | --- | --- |
| 依赖与值变化 | 现有 Signals 图 | 猜测 UI 的生命周期或重载恢复地址 |
| 工作的存亡、执行资格 | 现有 owner/执行协议 | 把每个普通函数变成独立实例 |
| 已挂载节点与布局、编辑、输入 | 现有 retained tree/kernel | 认识 card、Field、Chat 等组件类型 |
| 条件、列表、弹层和内容挂载 | UI 组合层 | 增加另一套状态容器或调度器 |
| 外观与复合交互政策 | kit 或普通组件函数 | 重写编辑、焦点和动作资格协议 |

作者日常仍是在组装内容树。基础内容包括容器、文本、图像、原子编辑和定制绘制；
属性是值或绑定；动态结构显式使用工厂。`row/col` 是容器便利写法，不是新的执行原语。
一个无外观 button 入口可以把名称、禁用、焦点与动作接到同一个容器上，kit 再组合样式。
不需要每个应用手工装配 ARIA，也不需要为每种控件新增 kernel Kind。

弹层的锚定、显示位置与 owner 仍分开；事件路由沿明确协议执行，不能简单沿 scope 链冒泡。
Agent/AX/绘制读取已结算事实；开发工具展示来源和 owner 关系即可，不另建能修改事实的“Agent 组件树”。

### 10.8 能删什么，以及停止继续抽象的标准

如果 C 被采用，可以删除“每个局部 state 必须命名”的要求；组件专用 scope/宏强制实例化不再是
一般有状态函数的先决条件。若仍需显式恢复或延迟实例，只保留表达该真实需求的入口。
局部 provider 改为有内容边界后，可以删除组件作者路径上依赖共享 scope 顺序替换的习惯。
这都是待迁移事项，本轮没有删除旧 API。

必须保留：运行时身份检查、scope 退休与资源释放、结构绑定优先级、任务取消、编辑协议、
有效动作目标，以及一份明确的构建时机契约。它们解决时间与所有权问题，不是可用改名消除的负担。

下一轮不再平行发明第四套组件模型。普通函数沿 Rust 求值规则执行；
需要延迟的内容显式使用工厂（作者模型已在第 11 节明确）。匿名局部状态在热重载中如何处理仍待裁定。
随后用下面的轨迹检验选定模型，再迁移生产代码：

- 同函数双实例、普通 helper 抽取、两个可独立显隐的实例：值独立，外观包装不改变 owner。
- 分支不变/切换、行重排/删除/再插入：初始化和 cleanup 次数明确，迟到事件不能作用于新实例。
- 未调用工厂与已经执行但未挂载的内容分别验证，不能把后一种也写成“零副作用”。
- 两个 sibling provider、延迟 children、弹层 handler/task：查找范围一致，退出后不污染兄弟。
- 按选定的恢复契约检查重载；即使值保留，旧挂载目标仍退休。
- 无 kit 的自定义动作、IME 输入与建议面板都沿同一节点/编辑/执行协议实现，不增加兜底的第二路径。

通过标准是这些场景只有一套可解释的构建与退休规则、旧路径可删除，而不是公开名称降到某个数字。
本轮完成的是前提审查和预期轨迹推演，尚未做隔离原型、性能测试或真实窗口验收。

## 11. 用户明确定位后的收敛：面向 AI 的函数组合

用户明确框架首先给 AI 使用，并要求学习 Rune 的函数组合方式。这改变了评审的优先级：
不再以人工记忆多少原语、宏是否少打几行代码来判定优劣；优先保证生成代码、局部修改、
来源定位、真实操作和结果验证有统一契约。这也符合已有 `docs/architecture.md` 的 Agent-first 主线。

本轮再次检查 Rune `tooling.rs::AppSpec<S>`：`init: fn() -> S` 与
`view: fn(&S, &mut Cx<S>) -> El` 分开；`examples/lab/src/ui.rs` 的 view 直接调用 sidebar、
stage_view、panel 等普通函数组成 El。headless tooling 使用实际运行时与渲染器，虚拟时钟服务于可重复场景。
[Rune AppSpec](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/tooling.rs)、
[Rune 函数组合实例](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/examples/lab/src/ui.rs)

采纳其作者模型：函数接收数据、响应式句柄、内容和回调，返回可组合内容；应用与 kit 使用同样机制。
保留 Octara 的 Signals 精细更新，不复制 Rune 的 commit 重跑、Cx/Tx 全套接口或版本缓存要求。
也不新增一层让 AI 生成 JSON UI/自定义 DSL 再转回 Rust 的执行表示。

这项选择**不要求先改变 state 签名**：两个状态在恰当 owner 中创建后传给同一个 counter 函数，
已经是普通函数组合。有用的局部状态可保留；是否默认匿名分配是后续便利性与恢复契约的比较。
不再为每个有状态函数强制增加 scope；也不把所有状态搬到全局作为简化捷径。

来源标记仍然有用，但不承担生命周期。AI 的闭环沿现有 Inspector、vocab、diagnostics、
目标代次和真实窗口反馈推进；缺少的属性来源能力应标为缺口，不以一个新 Agent tree 掩盖。
对照 B 的强制组件实例宏不再作为默认架构候选；确有延迟需求时使用普通函数的内容工厂。

当前可推进的方案已收敛到[重设计文档](../design/ui-composition-redesign.md)。剩余工作是契约和场景验证，
不是再展开新一轮“什么是组件”的命名讨论。本轮仍只改调研/设计文档。


## 12. 普通函数组合的隔离验证

日期：2026-11-17；使用当前 `d5619e8` 加工作区的公开 core API，Rust 1.100.0。
没有修改生产源码或新增工作区测试目标；探针在临时目录运行，复用已有 core 构建缓存。
`cargo run -q --offline -p octara-cli -- vocab` 核对了 IntoElement、El::label、El::behavior、each 和 kit Model。

结果：**4 passed，0 failed**，并检查各场景的布局与运行时诊断为空。

| 探针 | 实际断言 | 不能推出的结论 |
| --- | --- | --- |
| 参数传入状态 | 两份值互不影响，同句柄明确共享；更新保留动作 target；来源仍指向 counter | 所有局部状态都应外置，或跨重载恢复已解决 |
| 条件工厂 | false 时不执行；重复 true 不重建；隐藏清理一次；重开产生新 target 和初值 | 任意预建后丢弃的 View 都没有构建副作用 |
| 输入函数 | 行为 setter 先接受/归一化，应用回调随后同时看到提案与接受值；编辑 target 保留 | OS IME、selection/undo 的全部行为已验证 |
| keyed 快照 | 重排不重建；删除后同 key 新插入产生新 target；无需可写 options 镜像 | 已验证完整 Combobox/PopupList 迁移或虚拟列表性能 |

这里的 action/editor 是验证普通函数签名的局部 helper，并非新公开 API；action 没有实现待设计的
动态嵌套交互校验。没有 component(...) 或 component_keyed(...) 包装，来源宏没有分配 scope。
本轮没有新增控件状态类型家族，也没有修改 Signals 的创建或恢复规则。

以下保留实际运行的探针源码。复现时将代码块保存为临时 probe.rs，在仓库根执行
`cargo build --locked --offline -p octara --message-format=json`，从 compiler-artifact 的 filenames 收集
依赖搜索目录，再用 `rustc --edition=2024 --test`、`--extern octara=target/debug/liboctara.rlib`
和这些 `-L dependency=...` 编译，运行所得二进制的 `--test-threads=1`。
本机 Cargo 的依赖产物分布在各包构建目录，不应假定全部位于 target/debug/deps。

<details>
<summary>4 个隔离检查的完整源码</summary>

```rust,ignore
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// Review probe using only current public core APIs. Not a proposed new button implementation.
#[component]
fn action(name: impl IntoProp<String>, content: impl IntoElement) -> El {
    row().role(Role::Button).label(name).focusable().h(44).min_w(80).child(content)
}

#[component]
fn card(content: impl IntoElement) -> El {
    col().gap_2().child(content)
}

#[component]
fn counter(id: &'static str, value: Signal<usize>) -> El {
    col().id(id).gap_2()
        .child(text(move || value.get().to_string()).id(format!("{id}-value")))
        .child(action("Increment", "+1").id(format!("{id}-inc"))
            .on_press(move || value.update(|n| *n += 1)))
}

#[component]
fn editor(value: ReadSignal<String>, propose: impl Fn(String) + 'static) -> El {
    text_input().value(value).h(44).w(240).behavior(|b| b.on_input(propose))
}

fn assert_clean(window: &Window) {
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}

#[test]
fn parameters_determine_shared_or_independent_state_and_keep_provenance() {
    let mut window = Window::headless(|| {
        let left = state("left", || 0usize);
        let right = state("right", || 10usize);
        col().size_full().gap_2()
            .child(card(counter("left", left)))
            .child(counter("right", right))
            .child(counter("shared", left))
            .into()
    });
    let before = window.inspect();
    let target = before.find("left-inc").unwrap().target.clone();
    assert_eq!(before.find("left").unwrap().component.as_ref().unwrap().name, "counter");
    window.tap("left-inc").unwrap();
    assert_eq!(window.kernel().text_of("left-value").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("right-value").as_deref(), Some("10"));
    assert_eq!(window.kernel().text_of("shared-value").as_deref(), Some("1"));
    assert_eq!(window.inspect().find("left-inc").unwrap().target, target);
    assert_clean(&window);
}

#[test]
fn branch_factory_owns_creation_cleanup_and_recreation_without_component_scope() {
    let builds = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let (built, dropped) = (builds.clone(), drops.clone());
    let mut window = Window::headless(move || {
        let open = state("open", || false);
        on_command("open", move || open.set(true));
        on_command("hide", move || open.set(false));
        let (built, dropped) = (built.clone(), dropped.clone());
        col().size_full().child(show(move || open.get(), move || {
            built.set(built.get() + 1);
            let dropped = dropped.clone();
            on_cleanup(move || dropped.set(dropped.get() + 1));
            counter("child", state("count", || 0usize)).into()
        })).into()
    });
    assert_eq!((builds.get(), drops.get()), (0, 0));
    window.invoke("open").unwrap();
    let first = window.inspect().find("child-inc").unwrap().target.clone();
    window.tap("child-inc").unwrap();
    window.invoke("open").unwrap();
    assert_eq!((builds.get(), drops.get()), (1, 0));
    window.invoke("hide").unwrap();
    assert_eq!((builds.get(), drops.get()), (1, 1));
    window.invoke("open").unwrap();
    assert_eq!(window.kernel().text_of("child-value").as_deref(), Some("0"));
    assert_ne!(window.inspect().find("child-inc").unwrap().target, first);
    assert_eq!((builds.get(), drops.get()), (2, 1));
    assert_clean(&window);
    window.app().shutdown();
    assert_eq!(drops.get(), 2);
}

#[test]
fn callback_parameters_keep_the_editor_and_model_acceptance_order() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let observed = seen.clone();
    let mut window = Window::headless(move || {
        let value = state("draft", String::new);
        let observed = observed.clone();
        col().size_full().child(card(
            editor(value.read_only(), move |next| value.set(next.trim().to_owned()))
                .id("editor").label("Draft")
                .on_input(move |proposed| observed.borrow_mut().push((proposed, value.get())))
        )).into()
    });
    let target = window.inspect().find("editor").unwrap().target.clone();
    window.dispatch(Event::Input("editor".into(), "  中文  ".into())).unwrap();
    assert_eq!(window.kernel().value_of("editor").as_deref(), Some("中文"));
    assert_eq!(&*seen.borrow(), &[("  中文  ".to_owned(), "中文".to_owned())]);
    assert_eq!(window.inspect().find("editor").unwrap().target, target);
    assert_clean(&window);
}

#[test]
fn keyed_snapshot_composition_retains_rows_without_a_writable_mirror() {
    let builds = Rc::new(Cell::new(0));
    let built = builds.clone();
    let mut window = Window::headless(move || {
        let items = state("items", || vec![(1u32, "A".to_owned()), (2u32, "B".to_owned())]);
        on_command("reverse", move || items.update(|rows| rows.reverse()));
        on_command("remove", move || items.update(|rows| rows.retain(|(id, _)| *id != 1)));
        on_command("restore", move || items.update(|rows| rows.push((1, "Again".to_owned()))));
        let built = built.clone();
        col().size_full().child(each(move || items.get(), move |key, item| {
            built.set(built.get() + 1);
            text(item).id(format!("row-{key}")).into()
        })).into()
    });
    let first = window.inspect().find("row-1").unwrap().target.clone();
    assert_eq!(builds.get(), 2);
    window.invoke("reverse").unwrap();
    assert_eq!(window.inspect().find("row-1").unwrap().target, first);
    assert_eq!(builds.get(), 2);
    window.invoke("remove").unwrap();
    assert!(window.inspect().find("row-1").is_none());
    window.invoke("restore").unwrap();
    assert_ne!(window.inspect().find("row-1").unwrap().target, first);
    assert_eq!(window.kernel().text_of("row-1").as_deref(), Some("Again"));
    assert_eq!(builds.get(), 3);
    assert_clean(&window);
}
```

</details>
