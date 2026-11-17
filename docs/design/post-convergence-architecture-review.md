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
