# 平台验证路线

当前分层和工程门禁见 [Octara v2 架构总纲](design/architecture-v2.md)。实现与回归分布在各 crate 的测试中。

| 范围 | 验证入口 |
| --- | --- |
| 编辑器、重排、模态和触摸滚动 | [无窗口回归](../crates/octara-inspect/tests/headless.rs) |
| 多窗口、虚拟时钟和回放 | [确定性模拟](../crates/octara-inspect/src/dst.rs) |
| GPU 队列、颜色与回读 | [渲染测试](../crates/octara-render/tests/integration/) · [设备执行器](../crates/octara-gpu/src/executor.rs) |
| macOS 原生窗口首帧 | [原生宿主验证](../crates/octara-host-native/examples/smoke.rs) |
| Chromium 的真实 DOM、选择和回滚 | [浏览器验证](../crates/octara-host-web/examples/) |

后续平台验证沿用 macOS、iOS、Linux、Android、Windows 的推进顺序。实窗或真机验收记录在对应测试和构建收据中。[工作清单](TODO.md)
