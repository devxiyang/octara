# 执行阶段边界审查

2026-11-17，Octara 工作树源码审查。对应
[Rune 执行架构研究](rune-execution-architecture.md)中“先查实际越界，再决定重构”的第三阶段。
最初为只读审查；以下控制流描述保留修复前基线，实施与实测结果见文末。
A 的三项失败、B 的测量写入和 C 的纯计算动作已在修复前运行复现。
这些结果不意味着所有宿主输入 API 或所有运动反馈路径都已经全面审计。

## 1. 保留的机制与边界

Octara 不需要为了获得测量/发布边界，改成每次更新都构造完整候选描述树。
现有 retained kernel、细粒度绑定和局部布局已经能承担主要职责：

| 边界 | 源码证据 | 已有回归 |
| --- | --- | --- |
| 有待处理响应式工作时，不把窗口布局标为完成 | `app/frame.rs::settle_layout` 在 flush 后检查 `has_pending` | `integration/windows.rs`、`integration/list_runtime.rs` |
| 输入 proposal 的接受回调尚未结束时，不发布嵌套帧 | 同函数检查 `kernel.has_active_edit()`；`kernel/editor.rs` 所有编辑提案 | `integration/edit.rs::reentered_input_is_refused_without_overwriting_the_outer_proposal` |
| 尺寸反馈超限保留 pending，等待下一次显式操作 | `settle_layout` 的四轮上限、`WindowUpdate::Incomplete`、`App::pending` | `integration/size_of.rs` 的循环与有限反馈用例 |
| 跨窗口失效重新准备，超限不忙重试 | `AppState::frame_except` 的 generation 记录与十六轮上限 | `integration/windows.rs::cross_window_layout_feedback_is_bounded_and_can_recover` |
| 反馈回调按注册 owner 执行，退出作用域后不再调用 | `app/feedback.rs::after_layout` 捕获 `ExecutionOwner`，调用前 `admits`；清理移除 hook | `app/feedback.rs` 单元测试 |
| 分离回调批次与注册表借用；较早 hook 可以撤销较晚 hook | `sync_after_layout` 克隆批次，调用前检查成员仍存在；每次调用后检查窗口存活 | 同模块的移除、关闭和析构重入测试 |
| 几何读与响应式发布分开借用 | `sync_after_layout` 先采样 sizes/tails/scrolls，再释放 kernel 和注册表借用，随后写 Signal | 同模块；`integration/size_of.rs` |
| 只有成功交给 host 后，才推进发布记录 | `app/presentation.rs::present` 在 `PlatformWindow::present` 返回后更新 text_input/cursor/presented/frames | `integration/app_lifetime.rs::failed_host_submission_does_not_publish_a_completed_frame` |
| watch 的依赖采集与业务回调分开 | `reactive/graph/signal.rs::watch` 的 source 在 Observe 下运行，callback 切换 Handler；`untrack` 不提升权限 | `reactive/graph/tests.rs` 与 `docs/design/reactive-core.md` |

这些是现有实现的证据，不表示它们覆盖所有扩展 provider 或所有 budget 退出路径。
尤其不能把已经正确的 scope admission、编辑接受和 host 失败处理删掉以减少类型数量。

## 2. 候选 A：焦点反馈耗尽准备预算后仍可能发布

**状态：修复前源码分析；已运行复现，见实施记录。**

`app/frame.rs::prepare_window` 的十六轮循环包含任务、布局、modal、焦点和 deferred command。
`settle_requested_focus(...) || reconcile_focus()` 成立时，代码把 `command_ran` 清为 false，
然后继续下一轮。假如 `on_focus` 不断提出另一个焦点请求，最后一轮仍能沿这条路径退出预算。
循环后只对 `command_ran == true` 补做布局，随后无条件进入 native 同步和 `present`。

`settle_requested_focus` 会消费一个请求，但它运行的 `on_focus` 可以再写入请求。
`WindowState::interaction_request_focus` 只设置 `focus_request`，不增加 kernel generation；
`App::pending` 目前以 `settled_generation != mutation_generation` 判断窗口更新未完成，
不单独检查焦点请求。因此预算结束后可能保留待办焦点，却记录为已完成窗口。

另一个需覆盖的出口是“最后一条 deferred command”：循环后的补做路径调用
`settle_requested_focus` 和 `reconcile_focus`，但后者的业务回调可能再次改变视图，
该路径没有重新证明更新已稳定就继续发布。

### 最小复现与验收

1. 两个可聚焦元素 A/B 的 `on_focus` 在 enabled 为 true 时互相 `focus`；通过输入 handler
   开启 enabled 并请求 A，避免在构建阶段提出请求。
2. 断言回调工作受限，窗口仍 pending，本次不增加平台发布帧，不产生立即重试 deadline。
   pending 查询本身不得继续跑反馈。
3. 后续显式操作关闭 enabled 后，窗口能够完成、清空 pending，并发布正确焦点。
4. 有限链超过十六次重定向时，第一次操作仍 pending；后续 poll 能继续完成，不能丢请求。
5. 第十六条 deferred command 引发隐藏/禁用焦点，再由 blur/focus 回调修改布局时，
   host 接收帧前必须完成这些变化；已有 command 丢弃策略保持原语义。

建议仅显式记录准备循环是否到达稳定出口；预算耗尽且仍有焦点/反馈工作时返回
`Incomplete` 并保留请求。不要引入公开事务对象或完整候选树。诊断如需新增，按
`octara::diagnostics` 的注册规则处理。

## 3. 候选 B：文本测量 provider 继承调用方写权限

**状态：修复前源码分析；已运行复现，见实施记录。**

`app/frame.rs::settle_layout` 直接调用 `kernel.layout`，没有为测量切换执行权限。
`kernel.rs::compute_layout` 借用共享 text engine，并通过 `text_size`/`input_size` 调用
用户可配置的 `TextEngine::layout`；`node_layout` 同时持有节点布局缓存借用。
在常见 Idle/Handler 入口下，这些 provider 方法继承允许写 Signal 的权限。
而 `settle_layout` 的 `has_pending` 检查在本次测量之前，provider 如在测量中写 Signal，
后面的 `sync_after_layout` 又没有产生变化，就可能在新的响应式工作处理前完成本次发布。
provider 若重入窗口或资源写入，还可能撞上已经持有的 kernel/text 借用。

已有可复用的权限边界是 `app/resources.rs::change_resources` 的字体注册分支：
它以 `Ctx::Observe` 和 `untrack` 调用 provider，拒绝业务写入，并以 detached engine
避免持有存储借用跨越字体注册回调。

### 最小复现与验收

1. 使用包装 `MonoEngine` 的自定义 `TextEngine`；其 `layout` 可运行一个仅触发一次的回调。
   在 App 根作用域创建 Signal，再在打开文本窗口前装入回调，尝试修改该 Signal。
2. 验收为：provider 确实执行，Signal 不变，产生现有纯阶段写入诊断，App 仍可正常使用，
   不出现为了修正 provider 写入而多发布的一帧。不要用无限 provider 自增循环作复现。
3. 分别尝试资源写入和 shutdown；必须在进入额外 kernel/engine 借用前拒绝，不能以
   RefCell panic 作为“成功阻止”的结果。重复覆盖普通文本与多行输入测量。
4. 测量 provider 主动 panic 时，App 必须失败且不能增加已完成发布计数；延续既有失败语义。

建议在运行时测量/provider 调用建立不可写、无依赖收集的内部边界，优先复用现有权限。
还需列出其它 shaping/LaidText 方法入口，避免只保护一个方法而遗漏同一权限路径。
`kernel/edit.rs::TextInputLayout::layout` 特意支持不进入 App 的原生查询，应单独保留并测试
其“engine 忙时返回 None”契约，不强制它通过业务帧或持有 Window。

## 4. 候选 C：部分 UI 动作未接入执行权限检查

**状态：修复前源码分析；已运行复现，见实施记录。**

`view/mod.rs` 中的 `focus`、`scroll_into_view`、`invoke_after_update` 和 `settle_motion`
直接调用窗口可变状态路径，没有像同文件 `on_command` 那样先检查 `Operation`。
所以单独把测量切到 Observe，仍不能阻止 provider 用这些 API 排入焦点/command/reveal，
或直接落定 motion。这也是普通 memo/watch source 可达到的权限缺口。

验收应逐项从 memo、watch source、属性绑定调用，并重复套入 `untrack`；
断言焦点、命令执行次数、滚动和 motion 均未改变。正常 handler 和 task 入口仍需可用。
只在动作发生前接入已有 admission policy，不为这些 API 再建另一套权限表。

## 5. 需先取证的 motion 问题

**状态：差异已看到，可观察错误尚未证明；复现未运行。**

`kernel.rs::layout` 每次执行后会 `clamp_scrolls` 和 `track_frames`，而窗口的尺寸反馈
可能要求多次布局。这比 Rune 的 Measurement/Measured/apply 边界更宽：准备中的几何会
更新 motion 轨迹。不能仅凭该差异就断言当前行为错误，也不应直接搬动全部布局代码。

先在同一逻辑时间比较“直接到最终尺寸”与“经多轮尺寸反馈到同一尺寸”的运动位置、速度、
后续采样和 deadline；同时覆盖未完成布局后才停止反馈的情形。只有发现试算次数影响
已发布运动时，才考虑把轨迹目标更新推迟到稳定后的内部发布步骤。

## 6. 第三阶段完成条件

- 候选 A/B/C 先有能说明失效的最小行为回归，再提交最小内部修复；未复现的推导明确保留状态。
- 回归检查平台收到的帧、pending/重试行为、回调次数和最终交互行为，不能只比最终树。
- 保留已有 retained tree、Signals、owner/代次、编辑接受、失败与析构契约；不增加公开 Tx、
  手工 version/memo 协议或第二套候选树运行时。
- 既有尺寸反馈、跨窗口反馈、任务 budget、deferred command、编辑重入、provider 和 host
  失败测试继续通过；测试使用共享已有构建缓存，不额外创建无界产物。

本文只记录审查与验收要求，不声称第三阶段已实施或测试通过。


## 6. 第三阶段实施记录

- A：prepare_window 必须证明到达稳定出口；焦点回调耗尽轮次时保持 Incomplete，
  最后一条命令引发焦点回调也不能直接发布。复用 LAYOUT_LOOP，原命令队列超限丢弃政策不变。
  基线三项失败（无限重定向、超过十六步的有限链、最后命令触发 blur 更新），修复后四项通过。
- B：App 创建资源时给 TextEngine 和返回的 LaidText 安装一个私有 provider 执行边界，
  统一只读、无依赖收集、无 ambient scope/window；保留缓存 downcast。底层 text crate 不依赖
  reactive。Runtime 使用 Weak，不引入框架引用环；原生快照通过私有可用性判断在 owner 失败或
  消失时拒绝查询；shutdown 在 cleanup 前撤销原生查询可用性，但不提前释放引擎。引擎暂忙而文本为空仍可返回几何，这与 owner 已失效是不同情况。
  布局只写一次 Signal 的隔离基线实测从 0 变成 1；修复拒绝写入。
- C：UI 动作复用 Operation::Write；键盘注册使用 Register，保留构建时注册。
  instant 被拒绝时仍以原权限执行闭包。命令调用的权威入口是 Window::invoke，
  convenience 函数与同/跨 App 捕获的 Window 都不能绕过。动作专项基线两项失败，修复后四项通过。

这轮不新增公开原语或测量候选树，也没有改变全局 a11y_hidden 输入含义。
宿主 event/dispatch/press_key 等输入入口没有据此获得完整安全认证；运动在布局重试中的
可观察等价性仍需先取得失败证据，再考虑改动。完整工作区验证记录在 TODO。

交叉复核补充：无窗口 App-root 纯计算调用 convenience invoke 先检查权限，再解析窗口，
避免把正常拒绝变成缺少窗口的 panic。Window::invoke 仍保留实际句柄的权威检查。
成功 shutdown 的原生快照测试与 owner drop 分开；重复 shutdown 在 cleanup 中仍是无诊断的幂等操作。
