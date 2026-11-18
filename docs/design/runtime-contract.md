# 执行资格与资源寿命

属于 App 的任务可以跨窗口运行，属于控件的任务随挂载作用域退休。用 `App::run` 明确建立应用级工作；只在需要独立寿命时为局部内容建 scope。普通组件函数可以使用调用者的所有者。[运行时](../../crates/octara/src/reactive/) · [App 生命周期用例](../../crates/octara/tests/integration/app_lifetime.rs)

## Failure boundary

逃出框架执行边界的 panic 可使 App 失去继续执行和发布的资格。`APP_FAILED` 出现后，保留诊断，执行 shutdown，需要重新运行时创建新 App。普通输入遭拒或一次请求失败可通过对应操作的返回值处理。框架资源可以继续释放；已中断的用户 cleanup 不得被重复调用。

## Lifetimes and external work

框架在调用用户函数、执行 cleanup 或挂载行时可能被重入，连当前窗口都可能被关闭。挂载中断的复核条件是：原调用返回后重新判断父作用域是否仍有效，跳过后继绑定或替换工厂，确保部分构造的窗口不能提交首帧。工作区相关改动的验收状态列在[工作清单](../TODO.md)。[窗口关闭用例](../../crates/octara/tests/integration/windows.rs)

## Timers and channels

周期定时器需要大于零的周期，`INVALID_INTERVAL` 表示注册未被接受。检查任务归属和触发条件后再调整时间。确定性测试可以推进应用时钟，断言任务何时就绪、何时取消。Future 被再次轮询时要以当前 Waker 为准。[任务用例](../../crates/octara/tests/integration/task.rs)

## Retired completion failures

外部阻塞工作可能晚于 App 关闭才返回。退休后的结果不应重新激活队列或向窗口发布。如果迟到的通知或析构本身 panic，`LATE_COMPLETION_PANIC` 记录在原 App 上。排查时分别观察业务结果有没有交付、外部工作是否已释放、故障计入哪个 App。
