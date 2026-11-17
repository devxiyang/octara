# 收敛后的分层架构复核

日期：2026-11-17。Octara 基线：`8170b32`，已 push 到 `origin/init`。
原评审只做分析；后续获批实施记录见文末。

Rune 依据为本地固定版本 `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`，以及前轮已保存的
`631e8a67cb3c40225efa3f6c4f57b80e7b18f190` 执行链研究；本轮没有声称验证上游最新 HEAD。
前序方案见 [组合设计](ui-composition-redesign.md) 和 [Rune 执行链研究](../research/rune-execution-architecture.md)。

## 判断标准

减少重复事实、重复算法和没有实际消费者的抽象；不以包数、导出数或 trait 数衡量简单程度。
保留 Signals + retained tree，不引入第二套失效机制。普通函数负责组合，显式 owner 负责确实需要的生命周期。

## 从底层向上

| 层次 | 保留 | 合并或删除候选 | 优先级 |
| --- | --- | --- | --- |
| primitives / render | 单向依赖、独立绘制协议、CPU/GPU 后端 | 合并 Picture / Bitmap 为一种规范像素结果，删除字段搬运与重复 PNG 编码 | P2 |
| runtime / signals | 统一执行权限、owner 生命周期、Provider 边界、Signals | 统一结构身份与路径政策，禁止用显示文本代替真实 key 身份 | P1 |
| 输入内核 | 编辑器、手势识别、公开事件组合 Behavior | 私有 app::behavior::Behavior 只有 Field 一个实现，可改为直接编辑器分派 | P2 |
| view 组合 | El / View 的构造与挂载时机、IntoElement、现有 Rows 复用 | 删除只供 overlay 单一 View 使用的 Children trait、12 组 tuple 实现及 boxed 转换 | P2 |
| 选项组件 | keyed 快照、真实选择与焦点政策 | 删除 RadioGroup 的 RadioRow、StateList 镜像、order、排序和同步 watch | P2 |
| 选择交互 | Combobox 与输入建议不同的焦点/关闭/提交规则 | 共享纯选项运算和适用的行构造；不新增万能 Controller | P2 |
| Markdown | 流式解析缓存、文本映射、链接/选择 | 审查全局实例计数与命名状态恢复，收敛派生缓存所有权 | P2，先验证 |
| kit / app | 可选 kit、普通函数、显式业务参数 | 审查无命名状态组件的 scope；共享真正重复的 divider 交互 | P3，先证明等价 |

### 1. 身份问题：已复现，应先处理

`view/list.rs:135` 用 `Display` 将列表 key 拼入 scope 路径；行集合却用 `Eq + Hash` 判等。
`reactive/graph/scope.rs:230–264` 按 `(path, state_key)` 恢复命名状态，并可把已有状态归属移到新 scope。
这两套身份规则不等价。

临时 headless 回归：两个分别为 Key(1)、Key(2) 的键，Eq/Hash 不同，但 Display 都输出 `same`；
每行声明 `state("local", || key.0)`。期望行文本 `1 / 2`，实际 `1 / 1`。
运行 `cargo test -p octara --test integration review_identity_probe -- --nocapture`，
第二行断言失败（actual `Some("1")`，expected `Some("2")`）。临时测试已撤回，原文件已恢复。
这不是重复业务 key 的误用，而是当前泛型契约允许的不同 key 被路径字符串合并。

显式 scope 已转义 `%` 和 `/`，但 branch/list 仍各自拼接路径。不能只给 list 补转义：
两个不同 key 的 Display 仍可能相同。下一步须统一明确：实际挂载身份、跨 reload 恢复身份、诊断显示名各承担什么职责。
选择稳定 key 契约或内部结构身份时，须同时考虑 reload；不能用随机 ID、裸 hash 或全局计数掩盖问题。
验收覆盖同显示名不同 key、分隔符、重排/移除、虚拟化离屏、分支切换及 reload，确认状态独立且旧目标失效。
Rune 的新版 commit/flatten 集中构造 node_path 值得参考，但仍拼接原始字符串 key；不能照抄其编码并假定安全。

### 2. 可以确定删除的机械结构

- `octara-kit/src/forms.rs:123–176`：RadioGroup 仍维护旧选项镜像。Segmented 已使用相同 RadioOption 的 keyed 快照，
  可沿已有路径迁移，无需再造 collection 原语。保留禁用导航、选中值政策、重复 key 诊断与目标身份。
- `octara/src/view/mod.rs:141–187`：私有 Children 与 tuple 实现只有 boxed 一条消费路径；
  overlay 实际只传一个 View。直接构造内部集合即可，公开 `.child/.children` 不受影响。
- `octara/src/app/behavior.rs:11–32`：只有 TextInput/Field 的私有动态接口；可直接调用编辑器行为。
  这与公开 `view::Behavior` 的多层事件组合不是同一个机制，后者应保留。

### 3. 可合并，但要明确语义

`octara-primitives/src/capture.rs:7` 的 Picture 和 `octara-render/src/lib.rs:28` 的 Bitmap
都持有 width/height/straight-alpha sRGB RGBA。规范结果放底层，renderer 私有保留 premultiplied 转换。
目前 PNG 写入并不完全相同：Picture 写 sRGB metadata，Bitmap 没有；错误类型也不同。
合并应统一编码/错误策略，验证透明像素与 PNG 元数据，而不是只重命名或加别名。

Combobox/PopupList 可共享 enabled keys、循环导航等纯计算与合适的行构造。
Rune 的 [Combobox](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L315-L332)
复用 PopupList 行是直接参考。模态选择、非模态输入建议、Menu、Tabs 和 Radio 不应合成一个状态机。

### 4. 暂不能机械删除

Tabs/Segmented 没有命名局部状态，并不意味着 scope 无作用：它还决定延迟构造、命名空间和资源归属。
删除前验证未挂载工作、context、handler/task cleanup 和 reload。不要把隐式 scope 再藏进所有普通函数。

Markdown (`octara-kit/src/markdown/mod.rs:82`) 用全局 AtomicU64 生成 scope key。
每次 reload 的新 key 不能认领旧命名派生状态，存在旧分配保留风险，尚未做重复 reload 的数量验证。
直接换稳定 key 也不够：恢复的 blocks 可能与新 Doc 不一致，初次 watch 又跳过同步。
先建立缓存生命周期和一致恢复规则。Rune 同样保留独立的
[Markdown 缓存](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/markdown/cache.rs)，
不能把解析缓存当普通重复状态一并删除。

Model getter/setter 支持拒绝和归一化写入；直接改为 Signal::set 会改变契约。
TextEngine/LaidText 包装保护开放 provider 的外部回调；Rune 使用具体 TextSystem，不能据此推断这些保护冗余。
Signal / ReadSignal、ScopeId / NodeId、TextBuffer / String 和 El / View 各有不同职责，当前没有合并依据。

额外的小型依赖清理候选：`octara-fonts/src/lib.rs:28` 的 CJK runtime installer 是资产包依赖 core 的原因；
调用方可用已有 AppConfig.text_engine 注册公开字体字节。保留资产包，评估删除 installer 与该依赖，验证首帧 CJK/Latin 和字重。

## 建议推进顺序与停止条件

1. 先统一结构身份设计并闭合已复现问题；不新增状态/执行原语。
2. 删除 RadioGroup 镜像与无消费者的 Children 转换族，分阶段验证并提交。
3. 合并规范像素结果；评估私有单实现输入接口的直接分派。
4. 单独验证 Markdown 重载缓存，再决定 scope 和缓存组织；按具体重复点共享选择组件内部实现。

完成标准是每条事实只有一个权威来源、同类结构身份遵循同一规则、删除路径无遗留消费者，
原有输入/目标/所有权语义通过验证。headless 身份复现不代表已完成真实 OS 输入或 IME 验证。
不为追求统一再引入通用组件管理器，不以删除 kit 包本身作为本轮目标。


## 获批实施：结构身份

第一阶段以结构化 ScopeSegment 替代字符串路径：App、Window、显式 scope、branch、普通/虚拟列表各有明确的段类型。
动态 key 按类型、Hash 和实际 Eq 认领运行时内的规范身份；碰撞仍比较真实 key，不使用裸 hash 作为身份。
命名状态恢复和内核选择路径使用同一规范身份。Display 不参与身份判定；Copied.rows 为 Markdown 回调保留显示标签载荷，
已注明标签可能重复，不用于框架内部选择定位。删除虚拟列表显示名反查表和旧 child_name 拼接政策。

内核 token 不持有用户值；用户 key 的 Hash/Eq/Display 和临时释放在不追踪、无环境 owner 的只读边界执行。
最后一个 token 释放仅标记待清理；最外层 App 操作返回时在内部借用结束后回收失效 key。无退休标记时不扫描。
shutdown 清空 key 值，即使外部仍持有选择 token。热重载保留的命名状态路径保有 token，按真实 key 恢复。

公开 scope/keyed 明确要求 Eq + Hash + Display + 'static，不增加作者需要管理的 ID 或新状态原语。
TextBuffer 的 Eq/Hash 延续其原有缓冲区身份语义，Chat key 类型同步迁移；不兼容旧字符串状态路径。
身份回调审查同时闭合 named_state 注册和 hot_reload 写权限入口，保留合法主机/handler 重载。

验证：核心 594 通过、0 失败、1 既有忽略项；教程 doctests 12 通过。
新增用例覆盖 snapshot/增量 each 与 virtual_each 的同显示名、同 hash、重排、移除重建、reload、过期目标，
以及虚拟离屏复制、类型隔离、比较/释放不订阅、索引借用释放与 shutdown 回收。独立代码复核无剩余阻塞项。

选择用 RowKey 还包含稳定的 owner 路径和列表区域：同一容器中的相邻 each，或两个无包装节点的
scope 各自声明的 each，可以重复使用同一个业务 key 而互不混淆。新增测试验证单独选择右侧行、
跨列表选择与保存路径在 reload 后仍定位正确。最终核心验证更新为 **594 通过、0 失败、1 忽略**。

范围边界：现有虚拟列表排序注册仍以滚动容器为单位，本轮没有扩展同一容器内多个 virtual_each 的布局/排序协议；
需要多个独立虚拟列表时使用各自的容器。显示标签载荷不承担选择定位。

## 获批实施：RadioGroup

删除 RadioRow、可写 StateList 选项镜像、order 字段、排序和同步 watch，直接向 each 提供 keyed 快照。
保留现有受控值、禁用导航、焦点和缺失选中值政策。重复 ID 沿用快照拒绝规则，恢复后旧目标仍被拒绝。
基于新核心复验：8 项 kit 单元测试和 107 项集成测试全部通过。

## 获批实施：删除 Children 转换

删除私有 Children trait、12 组 tuple 实现及 boxed 适配器。唯一调用方 overlay 直接构造
Content::Box(vec![content()])，保留原来的来源位置、容器节点、延迟工厂和 owner。
公开 child/children 接口不变。10 项 overlay 相关回归全部通过；本阶段不为等价转换新增镜像测试。

最终工作区验证：**966 通过、0 失败、3 既有忽略项**，包含全部 11 项本地 HTTP 验证。
初次沙箱运行拒绝回环端口绑定；取得所需执行权限后完整重跑通过。严格 all-targets Clippy、格式、
分层与 diff 检查均通过。对身份 token 的退休标记导致的 mutable_key_type 保守诊断，
只在四个具体集合使用处注明其不参与 Eq/Hash 的理由，没有放宽全局 lint。
这些结果不代表真实 OS 输入、窗口呈现或 IME 候选窗验收。


## 后续阶段：统一绘制像素结果（init）

截至 d5756aa 的收敛改动已推送 init，并快进推送 main；后续改动继续留在 init。
本阶段合并 Bitmap 与 Picture，规范类型位于 primitives::capture，renderer 不再声明平行类型或旧名别名。
CPU 的浮点预乘转换和 GPU 的读回格式转换仍由各自后端负责；标准结果为自顶向下的 straight-alpha sRGB RGBA8。
像素访问、内存 PNG 编码和文件保存共用同一种结果；编码与 IO 失败返回 io::Result，
App 截图边界继续附带文件路径并负责 native snapshot 收尾。

参考本地 Rune 固定版本 3686e94 的 render.rs::write_png：明确 RGBA8/sRGB 与 IO 错误；
image/pixels.rs 的 Pixels 则另有可变帧、generation 和唤醒状态。借鉴的是结果数据与可变资源的分工，
不迁移它的图集/缓存，也不照搬 PNG 保存时自动创建父目录的行为。
Octara 的 Image（资源）和 Capture（窗口/原生覆盖报告）继续保留独立语义。
原 Painter 适配只是移动字段，本次删除不宣称减少像素复制或提升渲染性能。

像素阶段已完成：无 Bitmap 兼容别名，无 renderer 的平行 PNG 实现或直接 png 依赖。
验证通过：primitives 13 项单元测试与 1 项 doctest、render 2 项单元测试与 10 项集成测试、
core 截图相关 5 项、CLI harness 2 项。沙箱最初无法取得 GPU 适配器；取得执行权限后，
4 项 GPU 用例在真实适配器上全部通过，包含 CPU/GPU 像素比对、透明度、路径及 PNG 往返。
内存与文件 PNG 保留相同 RGBA8/sRGB 元数据；flush 错误保留 IO kind，截图保存失败仍清理原生快照并附路径。
工作区 all-targets 严格 Clippy、格式、分层和 diff 检查通过；没有声称完成真实窗口呈现验证。


## 后续阶段：直接编辑器分派（init）

私有 app::behavior::Behavior 只有 Field 一个实现；本阶段移除该 trait、Field 单例及动态分派元组。
指针识别器先检查捕获节点仍存活、未退出且为 TextInput，再直接调用编辑器的按下/拖动方法。
鼠标和笔优先拖动字段选择；触摸仍可由滚动识别处理。移动、释放和取消复用同一目标检查。
原来放在 behavior.rs 的 owner-bound handler 注册原样移至 input.rs，随后删除该文件。
公开 view::Behavior 的事件组合和注册清理语义不变。

本地 Rune 固定版本 3686e94 的 runtime/input/pointer.rs 在输入捕获时直接调用 input_drag 并返回，
提供了直接编辑器分派的参考；不迁移其输入状态机，也不改变 Octara 原有的手势优先级。

直接分派阶段已完成。验证：核心 **597 通过、0 失败、1 既有忽略项**，虚拟交互 **18 通过**。
新增三项回归验证鼠标/笔优先选择、释放/取消与迟到事件、字段上的触摸滚动，以及已销毁/退出动画中的
旧字段不会把捕获传给替换字段。原有 Shift、多击、事件组合与编辑生命周期测试通过。
工作区 all-targets 严格 Clippy、格式、分层和 diff 检查通过；独立生产代码复核无剩余问题。
本阶段没有真实 OS 输入验证，也不宣称删除动态分派带来可测性能收益。

## 后续阶段：Markdown 派生缓存归属（init）

已复现同一 Markdown 视图反复热重载后，响应式节点从 143 增长为 194、245，随后每次增加 51。
原因是全局自增 scope key 让每次挂载产生新的命名状态路径；这些用于派生块、代码行和表格的
StateList 被当作应跨 reload 恢复的状态保留。仅把计数器替换成稳定 key 还会遇到新 Doc 与恢复的
旧列表不同步，因此不采用改 key 或补首次同步的局部修补。

本阶段把解析文档和渲染缓存放回挂载 owner 的 memo 生命周期：源文本是输入，Doc 保留尾部解析
缓存，派生块通过现有 each 快照组合。删除 Markdown 的全局实例计数、独立命名 scope、命名列表
镜像和同步 watch，不新增公开原语，也不改变用户命名状态的恢复规则。

本地 Rune 固定版本 3686e94 的 markdown/cache.rs 区分输入文本与解析缓存，并在流增长时替换
旧前缀、复用已完成块。这说明解析缓存本身应保留；Octara 采用自己的 owner 释放边界，不移植
Rune 的线程级文档缓存和跨 commit 预算。保留 Doc.kept 前缀复用与代码尾行增量分割；为未改变
的块和行设置 memo 相等性边界，避免快照更新穿透到所有文本绑定。快照仍需遍历集合身份，
因此节点稳定和尾部增量不等于整体 O(1) 更新，验收必须同时检查生命周期与流式工作量。

复制回调只捕获与所选文本同版本的 Doc 缓存，不读取活跃 memo。虚拟列表会先提取离屏 Piece，
再释放临时 row owner，最后调用复制回调；强制求值 memo 会读取已销毁节点，或把旧 Piece 与新源偏移
混用。回调持有的 Rc 数据可短暂越过 owner 释放，而响应式计算仍正常退休。
有序列表起始编号和表格单元格对齐改为动态绑定；块种类、标题级别、有序/无序结构纳入私有块 key，
显示标签继续为 b{i}，保留 Markdown 复制的索引载荷。

生命周期回归已通过：同形重载 8 次计数不变，三类 Source 交替形状重载 18 次每种形状计数稳定，
挂载/卸载 16 次均回到空分支基线。代码尾部验证中，9 / 513 行追加一字符分别运行 29 / 1,037 个
闭包，但均只更新 5 字节尾行、标记 1 个 dirty text、创建/销毁 0 个节点并保留全部行身份。
因此本阶段保留尾部解析、内容分配与文本布局边界，但接受现有快照 O(rows) 身份遍历和 memo 检查；
不为消除这项成本新增集合原语，也不声称取得整体性能提升。真实窗口/帧延迟未在本阶段测量。

最终验证：kit **120** 项、Chat **73** 项通过，0 失败、1 个既有忽略 doctest；包括全部 11 项本地
HTTP 用例（沙箱拒绝回环端口后，授权重跑通过）。工作区 all-targets 严格 Clippy、格式、diff 与
分层检查通过。未修改内核、公开 API 或依赖；实现保持在 Markdown 两个文件内。

## 后续阶段：字体资产包去除运行时依赖（init）

截至 120658b 的改动已推送 init，并快进推送 main；后续继续在 init。
本阶段删除 octara-fonts::cjk::install，不保留别名或新注册包装函数。该包只导出已有 REGULAR、
SEMIBOLD、FAMILY、SCRIPTS，生产依赖为空。调用方通过已有 AppConfig.text_engine 注册字体，
然后创建 App/窗口；截图工具此前已经采用此路径。App 的运行期字体变更能力仍保留，二者职责不同。

参考本地 Rune 固定版本 3686e94 的 text.rs::TextSystem::new：字体字节作为输入交给文本系统完成
注册，资产不需要认识运行中的 UI。Octara 保留现有文本服务接口和显式脚本 fallback 政策，不照搬
Rune 的默认字体替换规则或系统字体发现机制。依赖检查同步禁止字体资产包向 core 建立生产边。

字体阶段验证完成：2 项 CJK 集成测试和 1 项 doctest 通过；禁用默认 feature 的构建/测试通过
（CJK 用例按 feature 关闭）。首帧混合文本和输入框无缺字，400/600 字重按字体字节核验，独立
CPU 绘制像素一致。普通依赖树只剩字体包自身；工作区 all-targets 严格 Clippy、格式、diff 与
分层检查通过。未做真实 OS 窗口验证，也不声称性能收益。

## 后续阶段：选择组件的纯运算复用（2026-11-18，init）

对照本地 Rune 固定版本 3686e94 的 combobox.rs::build_list 调用：它共享 PopupList 行构造，
但把 active 与 choose 回调留给 Combobox。这一边界值得学习；Octara 当前更适合先共享纯计算。
新增私有 choices 模块，仅包含 unique_ids 与 adjacent_id；合并两处重复 ID 检查和循环定位。
函数不持有状态、不读 Signals、不处理事件，不引入公开类型、控制器或新的生命周期。

唯一性仍检查整个原始选项集合；Combobox 在输入无效时仍把全部选项交给 each 诊断并退休行，
不能先按搜索词过滤而隐藏重复 ID。循环定位只消费已经筛出的可用 ID：缺失 active 从对应首/尾
进入、首尾回绕、空集合无目标。空集合时是否写 active 继续由调用组件决定，保持原有行为。

本阶段不共享整行构造：Combobox 的 selected 是提交值、highlight 单独控制背景，行是可截断
的 button；PopupList 的 selected 是待选项，支持描述文字并保持编辑器焦点和选择。把这些塞进
一个多开关行工厂会增加概念负担。模态、查询、提交、关闭、IME 及 Model 拒绝写入语义保持原位。
Tabs/Menu/Radio 没有被并入同一个状态机；后续 scope 精简仍须先证明其资源与命名空间等价。

纯运算阶段验证：kit 120 项、Chat 模型身份/选择 9 项通过，0 失败、1 个既有忽略 doctest。
沿用组件级回归覆盖禁用/空选项、回绕、重复 ID 退休与恢复、稳定目标、Model 拒绝写入和 IME
输入协议。工作区 all-targets 严格 Clippy、格式、diff、分层检查通过；未做真实 OS 输入验证。

## scope 删除候选的验收结论（2026-11-18，init）

**保留 Tabs、Segmented、RadioGroup 的显式 scope。** 组件内部没有命名状态，并不代表整个
组件子树没有命名状态。Model setter 在行 owner 中执行，允许注册 state、cleanup 和任务；
外层按组件 id 建立的 scope 是这些资源的稳定命名空间。普通函数和 each 不能自动替代它。

新增回归：同一父 owner 下的 left/right 两个组件均使用选项 key `shared`；每个 setter 在首次
调用时注册自己的 handler-count，初值分别 10/20，并注册 cleanup。首次选择后为 11/21，
交换组件顺序并热重载，再次选择应为 12/22。分别临时移除三种组件的 scope 后，三个实验均
失败，得到 left=22/right=12：each 区域按父 owner 的声明顺序编号，命名状态跟随位置而非组件。
所有实验改动已撤回；保留原 scope 后回归通过，reload 与 shutdown 各自释放对应旧行 cleanup。

延迟构造的审查也需要精确：each 的输入 memo 在挂载时创建，并非调用 each 时立即运行；因此
不能以“删 scope 必然提前求值 options”为理由保留它。本次保留的实证理由是跨重排 reload 的
命名身份，而不是假定所有包装都不可删。Select、Combobox、PopupList 另有明确的命名交互状态，
继续保留各自 scope。没有新增自动 owner、容器身份规则或公开原语。

本地 Rune 固定版本 3686e94 的 SegmentedControl/RadioItem 接收外部选择与回调，并使用元素 key
组合子节点。这支持把状态留给调用方，但不能证明 Octara 的 owner 边界可删：两个框架的状态
存储和执行协议不同。此次不搬用 Rune 的索引 key 来替换 Octara 的组件 id / 业务选项 key。

该候选已审查关闭：若未来要删除此 scope，必须先给出替代的稳定命名身份方案和实际收益；
不为了少一层包装引入另一个隐式命名空间机制。

最终验证：121 项 kit 测试通过、0 失败、1 个既有忽略 doctest；工作区 all-targets 严格 Clippy、
格式、diff 与分层检查通过。删除实验均已恢复；本次是 headless 身份/生命周期验证，未做真实 OS 输入。

## 后续阶段：复用分隔条组件（2026-11-18，init）

Chat::desktop_navigation 与 kit::split 重复同一套横向宽度拖动、8/40 像素键盘步长、Splitter
语义和可视样式。差异在模型边界：split 最小 120，Chat 限制在 180–420。新增 kit::splitter(id,
model) 作为独立布局的窄组件，让 split 和 Chat 都通过它组合；不新增内核原语、控制器或 scope。
它只提出宽度变化，显示值读取 Model；clamp、拒绝和归一化仍由调用方处理，下一次拖动重新采样
已接受宽度。用已有 behavior 注册组件事件，调用方添加 on_key/on_drag 不会覆盖内置调整行为。

Chat 保留自己的响应式外壳和桌面导航分支，避免把整套布局替换成 split 后跨断点重建主面板。
本地 Rune 固定版本 3686e94 的 shell/split.rs 同样把 grip 的外观和应用状态回调拆开组合；其
PaneState 还负责折叠、悬停和松手回退，本轮不迁移这些并不存在于 Octara split 中的产品状态。

这个变化增加一个可单独组合的 kit 叶组件，同时删除 Chat 中重复的分隔条实现；不以导出数量
衡量简洁度。split 继续负责两栏布局和原有最小宽度，splitter 不强制应用布局或宽度范围。

分隔条阶段验证：kit 122 项、Chat 长会话 3 项、教程 doctests 12 项、skill 5 项通过，0 失败、
1 个既有忽略 doctest。验证 Model 拒绝/限制写入、外部修改后的键盘读值、新拖动基于已接受值，
以及 Chat 宽度边界和断点往返保持编辑器身份。工作区严格 Clippy、格式、diff 与分层检查通过；
教程及导出 skill 同步，vocab 可查新组件。没有真实 OS 窗口或帧时间验证。
