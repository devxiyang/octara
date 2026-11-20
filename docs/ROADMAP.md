# 实现边界与路线图

| 能力 | 仓库目前可以检查什么 | 仍需取得的证据 |
| --- | --- | --- |
| 状态与结构 | Signals、带 key 的集合、挂载和退休。macOS 实窗已验收挂载中断时的首帧、后继绑定和释放 | 在其他宿主复核同样的窗口时序。[挂载中断实测](design/runtime-contract.md#macos-挂载中断实测) |
| 桌面窗口与绘制 | winit 宿主、CPU 参考绘制和 GPU 绘制。macOS 实窗已验收截图与图片切换。Windows 有交叉类型检查和历史实窗一致性作业 | Windows 实时检查、系统输入与实屏像素。[宿主支持范围](design/platform.md#桌面实时检查) · [图片实测](design/capture.md#图片尺寸像素与-gpu-图集) |
| 输入与语义 | 焦点、快捷键、语义操作和触摸位移分配。macOS 同窗验证了普通输入框的 AppKit AX 选区与范围坐标 | 跨进程读屏、换行和双向文字交界处的精细几何。[选区实测](design/input.md#辅助功能的文本选区与范围) |
| 组件组合 | 组件库使用公开视图接口实现按钮、表单、弹层、Tabs 和可搜索选择器 | 按桌面、触摸和辅助操作复核哪些行为需要下沉。[组件库](../crates/octara-kit/src/) |
| 移动端 | iPhone 17e 的 iOS 27.2 模拟器已核对系统触摸、中断恢复、侧滑返回、模态 VoiceOver 和普通单行字段的系统拼音 | iOS 真机签名、安装和实机验收。Android Activity 宿主与 APK 的系统输入和画面证据。[移动宿主支持范围](design/platform.md#移动宿主) |
| Agent 验证 | 构建收据核对进程，Inspector 读取语义和稳定目标；资料审阅实窗走通诊断、修正与结果回读 | 根据应用实作中反复出现的调用和观察成本决定接口收敛范围。[资料审阅实测](design/interaction-scenarios.md#资料审阅队列) |

## 推进顺序

文件重命名和嵌套拖动已经验证交互归属、取消和迟到处理。资料审阅队列用公开接口走通了 Agent 识别运行构建、观察界面、修复校验错误和读回业务结果的流程。[资料审阅实测](design/interaction-scenarios.md#资料审阅队列)

平台推进顺序为 **macOS → iOS → Linux → Android → Windows**。当前优先前三个平台，优先处理复合场景里已观察到的表达和反馈缺口。Android 和 Windows 现阶段以 CI 通过为验收条件。Android 交叉类型检查内核与 Kit，工作区测试覆盖 Android 的无窗口约定。Windows 沿用桌面构建、测试与窗口一致性作业。设备验收暂缓，设备环境可用后先接续 Android，再接续 Windows。[CI 配置](../.github/workflows/ci.yml) · [宿主支持范围](design/platform.md) · [工作清单](TODO.md)
