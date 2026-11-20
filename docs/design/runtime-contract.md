# 执行资格与资源寿命

属于 App 的任务可以跨窗口运行，属于控件的任务随挂载作用域退休。用 `App::run` 明确建立应用级工作；只在需要独立寿命时为局部内容建 scope。普通组件函数可以使用调用者的所有者。[运行时](../../crates/octara/src/reactive/) · [App 生命周期用例](../../crates/octara/tests/integration/app_lifetime.rs)

## Failure boundary

逃出框架执行边界的 panic 可使 App 失去继续执行和发布的资格。`APP_FAILED` 出现后，保留诊断，执行 shutdown，需要重新运行时创建新 App。普通输入遭拒或一次请求失败可通过对应操作的返回值处理。框架资源可以继续释放；已中断的用户 cleanup 不得被重复调用。

## Lifetimes and external work

关闭请求立即撤销窗口和其作用域的执行资格。当前回调可以正常返回，后继工厂、绑定和帧提交按当前所有者重新检查。App 的更新循环负责收尾并释放宿主。首次挂载中断的窗口应提交零帧，已呈现的窗口在替换分支或移除列表行时关窗应停止提交后续帧。[窗口关闭回归](../../crates/octara/tests/integration/windows.rs) · [分支回归](../../crates/octara/tests/integration/scopes.rs) · [列表行回归](../../crates/octara/tests/integration/collection_rows.rs)

关闭窗口后，动画时钟停止。关窗路径在作用域 cleanup 完成后销毁尚在退出的节点，即使调用方仍持有窗口句柄。macOS 宿主创建原生窗口时保持隐藏，首次收到内核提交且应用了该帧的原生视图变更后才显示窗口。[关窗释放](../../crates/octara/src/app.rs) · [退出动画销毁](../../crates/octara/src/kernel/anim.rs) · [macOS 宿主](../../crates/octara-shell/src/platform.rs)

Rune 在回调中登记关窗请求，由宿主边界处理。Exact 在原生 `windowWillClose` 中撤销路由，随后销毁会话，延后的首帧回调也复核会话代次。Octara 沿用同一条原则，在执行资格撤销后由统一生命周期入口收尾。[Rune 窗口请求](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/window/lifecycle.rs#L290-L355) · [Exact 窗口关闭](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactMac/main.swift#L492-L514) · [Exact 首帧回调](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/SessionActivation.swift#L8-L25)

### macOS 挂载中断实测

在 macOS 27.2 上保持观察窗口开启，依次打开单独的验收窗口。用 AppKit 窗口编号核对 WindowServer 登记，用相同的原生窗口身份关联 `octara.frame_trace/1`。已经呈现的窗口另取 `display` 像素作为关闭前的实屏证据。[显示捕获契约](capture.md)

| 中断位置 | 最终读回 |
| --- | --- |
| 子作用域第一次 `watch` | AppKit 窗口保持隐藏。WindowServer 只登记了屏外窗口，关窗后该记录消失。绘制尝试与提交均为零。cleanup 一次，跳过工厂结果转换、绑定与后继工厂。 |
| 替换普通和带动画的分支 | 两个窗口各有两帧宿主提交，关窗后计数仍为二。cleanup 各一次，跳过替换工厂和绑定。保留窗口句柄时，退出分支的原节点也已销毁。 |
| 移除普通和虚拟列表的行 | 两个窗口各有两帧宿主提交，关窗后计数仍为二。cleanup 各一次，跳过新行工厂和绑定。 |

四个已呈现窗口均取得 2× 的 WindowServer `display` 像素。关窗后，其 AppKit 窗口与 WindowServer 登记均消失，旧 Inspector 目标返回 `STALE_TARGET`，观察窗口继续应答。修复前，首次 watch 关窗时 AppKit 已报告该窗口可见，WindowServer 仍报告屏外，宿主提交为零。此次观察支持区分 AppKit 可见状态、帧提交与实屏像素。

## Timers and channels

周期定时器需要大于零的周期，`INVALID_INTERVAL` 表示注册未被接受。检查任务归属和触发条件后再调整时间。确定性测试可以推进应用时钟，断言任务何时就绪、何时取消。Future 被再次轮询时要以当前 Waker 为准。[任务用例](../../crates/octara/tests/integration/task.rs)

## Retired completion failures

外部阻塞工作可能晚于 App 关闭才返回。退休后的结果不应重新激活队列或向窗口发布。如果迟到的通知或析构本身 panic，`LATE_COMPLETION_PANIC` 记录在原 App 上。排查时分别观察业务结果有没有交付、外部工作是否已释放、故障计入哪个 App。
