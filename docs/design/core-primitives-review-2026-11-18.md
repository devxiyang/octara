# 内核与核心原语整体复核

日期：2026-11-18；Octara 基线 `4d0f021`，分支 init。本轮只评审、最小复现和文档，不修改运行实现。
范围是选取关键入口及其所有权链核查，不等同于逐行审计整个内核、所有平台或线程调度的形式证明。

## 结论

保留 Signals + retained tree 和已有统一执行协议。没有证据要求另建 VM、执行引擎或更多作者原语。
接下来应收敛两个底层契约：**重载保留状态的归属与退休**、**像素数据的尺寸不变量**。
继续精简 kit 不能解决这两类问题。快照列表的相等性传播是独立的设计候选，暂不追求极端优化。

## 已证实的问题

### P2：重载保留的旧 owner 状态越过窗口关闭

位置：[scope.rs](../../crates/octara/src/reactive/graph/scope.rs) 的 dispose_scope / named_state / shutdown，
以及 [app.rs](../../crates/octara/src/app.rs) 的 Window::reload。

重载期间 dispose_scope 不销毁 named node、不删除 store；mount_root 让再次声明的状态迁移到新
scope。但不再出现的 owner 路径没有对应退休步骤。旧 scope 已销毁，后续正常 close_window 只能
遍历新树，无法清掉这些旧路径。shutdown 最终扫描全部 node/store，才释放残留。

最小复现：App 中开一个窗口，root 用外部 Cell 指定 scope key；scope 声明一个 1 KiB Vec 状态
和文本绑定。依次把 key 改成 1..6，每次 hot_reload，然后关闭窗口、最后 shutdown。

| 时刻 | reactive_nodes |
| --- | ---: |
| 新 App，未开窗口 | 0 |
| 初次挂载 | 16 |
| 六次更换 key 后重载 | 17、18、19、20、21、22 |
| close_window 后 | 6 |
| shutdown 后 | 0 |

这是实际节点保留，不是每次重建的累计分配计数。影响需要热重载且 owner 路径变化；不能扩大成
普通卸载都泄漏，也不是已证实的安全漏洞。临时 core integration 用例已执行并撤回，日志在
`/private/tmp/octara-review-reload.log`。

**应统一设计：** 把重载保留记录明确归属于窗口/重载过程。重建成功后，对比旧记录的 owner 路径
与新树实际存活的 owner 路径，决定保留或退休；窗口关闭须能释放该窗口保留的全部记录，App root
状态不受影响。失败重载仍可通过既有 shutdown 收尾，不能重放已执行的 cleanup。

**不可采用立即按 state 调用认领数清扫。** 上一阶段 `b5c95f3` 已验证：Model setter 可在重载后
第一次操作时才调用 state。新 owner 已存在，但该状态尚未重新声明，仍应恢复；否则会破坏合法的
延迟注册。必须区分 owner 路径存活与某个 state 是否已经重新读取。对于同一存活 owner 中已经
从源码删除的 state key，运行时不能仅凭未被读取就判断其不可再用；这一保留政策需明确写入契约。

验收：同路径恢复、不同路径退休、handler 延迟恢复、组件重排、多个窗口隔离、正常关窗、重载
构建失败与 cleanup panic、最后 shutdown；用 Drop 计数及节点数同时证明资源释放。

### P2：RGBA 尺寸校验依赖编译模式

位置：[image.rs](../../crates/octara-primitives/src/image.rs) 的 Image::rgba（74 行）与 set（117 行）。
`width * height * 4` 先在 u32 中计算，再转 usize。开启溢出检查时会 panic；关闭时回绕可能让
错误缓冲区长度通过 assert。最小例子只需空 Vec，不需要分配巨大图片。

将仓库原始 image.rs 用 path module 编入独立小程序，复用本地 png 库；分别使用 rustc 的
`-C overflow-checks=on/off`（不是重新实现一份公式）：

| 输入 | 检查开启 | 检查关闭 |
| --- | --- | --- |
| Image::rgba(65536, 65536, 空 Vec) | 拒绝 | 接受 |
| 合法 1×1 Image.set(65536, 65536, 空 Vec) | 拒绝，原帧保持 1×1/4 字节 | 发布为 65536×65536/0 字节 |

源码及输出保留在 `/private/tmp/octara-review-image.rs`、`/private/tmp/octara-review-image.log`。
这证实原语能发布内部不一致的帧；本轮没有实际执行异常图像的 GPU 上传，也没有证明内存安全漏洞。

**应统一设计：** 明确尺寸、字节长度和坐标的校验边界，用 checked 运算在正确整数宽度计算
RGBA 布局，验证成功后才替换旧帧。继续审查 Picture、CPU target 分配和 GPU readback 的同类
算术，不能只修一个构造函数。公开 Picture/Frame 可被构造不代表所有入口都能信任其内容；需要
区分数据载体、受控资源发布和 renderer 的验证责任，不急于新增通用资源管理器。

验收：普通尺寸、零尺寸的明确政策、乘法溢出、缓冲区不匹配、set 失败保留旧帧，以及开启/关闭
溢出检查的一致结果。Image 身份/版本耗尽是另一个极低频问题，不与这次可构造的不变量缺口混为一谈。

## 设计候选，不当成已证实功能缺陷

### 快照集合的传播边界

[view/list.rs](../../crates/octara/src/view/list.rs) 的 Rows::build 给行 item 使用 comparer 恒 false
的内部 memo，允许 T 不实现 PartialEq。快照发生变化时，每个旧行都可能向下游传播；调用方需要
再加 memo 或引用相等边界阻止不变文本更新。Markdown 已加入这些边界；上轮实测 9/513 行追加
分别为 29/1037 次闭包运行，均仅写 5 字节、dirty_text=1。这是此前阶段的数据，本轮未重测性能。

根因是 snapshot 输入的变化语义没有携带相等性，而不是 Signals 出现错算。应先盘点包含闭包、
资源句柄和普通值的真实调用方，再决定是否为快照输入表达相等性；不能直接给所有 each 增加
PartialEq 限制，或为此新增第二套列表状态。增量 ReadList、稳定 row identity 和立即读取当前源
的语义应保留。当前不把这项列为必须优化，也不声称总更新应为 O(1)。

### 外部可变资产与唤醒

Image::set 只替换资源内容/版本，注释承诺的是下一帧看到更新；它不接收 App 或窗口订阅者。
若未来支持后台视频源自动驱动空闲窗口，需要明确谁唤醒、谁订阅、卸载何时取消、一次帧读取的
版本一致性。不能把 primitives 反向依赖 App，也不能因为 Rune 有 live source 就另造 Signals。
本轮未做空闲真实窗口复现，不能将这一候选标为现有 redraw bug。

## 各层保留与审查边界

| 层 | 本轮检查 | 判断 |
| --- | --- | --- |
| primitives | Image/Frame、Picture、尺寸计算与 renderer 消费处 | 优先闭合 RGBA 不变量；保留资源与输出数据的区别 |
| 执行政策 | execution.rs 的 Operation/Ctx 与 owner 恢复 | 保留统一准入；未找到新的权限绕过复现 |
| Signals | Signal/ReadSignal、memo/watch、with/update、context | 保留小型公开接口；结构体等 T 数据已支持，with/update 承担借用边界 |
| 生命周期 | scope 销毁、命名 store、reload/close/shutdown | 已证实重载保留记录的归属缺口，按协议修复 |
| 结构身份 | typed key interner、region、显式 scope、世代 | 保留实际 Eq/Hash 与显示标签分离；不改 UUID/随机 key |
| 集合/视图 | each 的 snapshot/ReadList 共用 Rows，行输入更新 | 保留共用挂载机制；相等性传播单独评估 |
| 异步 | ReadyQueue/TaskWaker、Sleep、Background 的锁与投递边界 | 保留入队合并、取消和释放分离；本轮没有新竞态证据 |
| 输入/呈现 | owner-bound handler、窗口 reload 与图像消费边界 | 保留直接编辑器分派和真实目标校验；未重新审计全部 IME/平台桥 |

Signal::set 不要求 T: PartialEq，memo 的值相等截止属于另一职责；不为减少类型名合并可写与只读
句柄。untrack 仍只关闭依赖收集，不提升写权限。scope 与 El/容器不能混成一个概念：上轮删除
实验已经证明组件命名身份有独立作用。StateList 的 read_only 与便利组合方法不因表面重复就删除。

## Rune / Exact 的借鉴边界

只核对本地固定版本：Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`，Exact
`08c889a9de21a30b7c3b308422fc588cf1a480d3`，没有宣称它们是最新上游。

- Rune `runtime/motion/live_latch.rs` 按真实 scene 成员刷新 live subscriptions；读取前消费 pending
  wake，避免发布与清标记竞态。可学习的是存活集合决定订阅寿命、清标记的时间顺序；不移植完整
  live 系统。`image/pixels.rs` 的资源携带 live source，与 Octara 目前纯资产层的边界不同。
- Exact 的历史问题记录 `issues/20261110-runtime-destroyed-in-a-served-callback-is-never-freed.md`
  描述先置 destroyed、再因回调忙碌拒绝真正 destroy，导致后续无法补做释放。本地 Bridge.swift
  仍可见这一调用形态；本轮没有运行其 XCTest、也没有验证最新上游修复状态。可学习的是不能将
  “不再执行用户工作”误当成“所有资源已释放”，不是照搬它的线程/桥接执行器。
- Octara 已有 Retiring 与 shutdown 继续收尾、Background 锁外通知/析构、ready 入队合并等机制。
  这些应保留并用故障测试约束；本次要把重载保留 store 纳入同一归属协议，而非继续在每个组件
  手动规避缓存残留。

## 验证与推进顺序

本轮执行：核心 lib **158 项**、primitives lib **13 项**测试全部通过。另执行上述临时 reload
integration 复现和原始 Image 源文件双编译模式探针；临时仓库改动全部撤回。通过既有测试不表示
上述未覆盖场景已经正确。没有完整 workspace 重跑、真实窗口、GPU 异常输入或新延迟基准。

1. 先明确重载保留记录的窗口/owner 归属协议和失败收尾，再实现、补齐验证。
2. 统一审查并修复像素布局不变量，不扩大成图像/renderer 全面重写。
3. 快照相等性和外部资产唤醒保留为有实际需求/数据时再推进的候选。

完成标准是状态不会归错 owner、窗口关闭释放其保留记录、失败仍能收尾、非法像素不会被发布；
不是消灭所有 scope、减少所有类型名或追求每次更新常数时间。新增公开原语目前没有必要。

## 实施阶段一：重载命名状态归属

统一入口 settle_named_state 在窗口重建完成、窗口关闭时结算该窗口的命名状态。依照当前存活
owner 的规范路径认领保留值，并把尚未由 state() 查找的值加入新 scope 的节点清单；因此延迟
handler 恢复与之后的正常卸载都有效。App 状态与其他窗口不参与本窗口结算。

无存活路径的名字先全部撤销，再调用值的析构函数。这样用户析构重入 hot_reload 时只能重新
创建已退休路径，不会复活一半退休的条目。未开始释放的值继续由图节点持有；析构 panic 后
shutdown 可扫描并消费剩余节点，已开始的析构不重放。不增加公开原语或第二套恢复缓存。
同一存活 owner 中尚未读取的旧 key 继续保留，无法仅凭未读取推断它已从程序中删除。

阶段一验证：新增 5 个 scope 回归全部通过，覆盖重复路径变化、多窗口/App 隔离、handler
延迟恢复、重建/析构失败、多条遗留值在 panic 后收尾，以及析构重入重载。全 workspace
测试通过，包含已有 cleanup 中断不重放验证；严格 Clippy、格式与分层检查通过。
本阶段没有真实 OS 窗口输入验证。
