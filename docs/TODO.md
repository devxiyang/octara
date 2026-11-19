# 当前工作清单

这里记录仍需决策或取得证据的工作。通过的实现细节随代码和回归测试维护。

## 第一轮架构验收

- [x] 资料审阅队列用公开接口完成从启动、观察、校验到结果回读的实窗闭环。[实测记录](design/interaction-scenarios.md#资料审阅队列)

- [ ] 在 macOS 真实窗口验收文件重命名。用拼音候选确认、同名文件重排、外部改名冲突和删除重建验证选区、光标、提交次数及过期目标。记录系统候选窗内容与位置。[桌面重命名契约](design/interaction-scenarios.md#桌面文件重命名)
- [x] 在 iOS 模拟器核对联动面板的原生触摸、系统中断、迟到位移和恢复呈现。[实测记录](design/interaction-scenarios.md#ios-模拟器实测)
- [x] 在 macOS 实窗核对触控板的直接阶段、惯性、边缘及原视口卸载后的路由。[实测记录](design/input.md#macos-实窗采样)
- [ ] 用验收样本复核组件可组合的公开 API。每个拟公开的入口至少要对应一个用现有公开接口无法正确表达的步骤。[架构的原语标准](ARCHITECTURE.md#怎样判定一个公开原语)

## 紧随其后的对照

- [x] 在两个同标题的 macOS 实窗核对系统菜单、快捷键、工具栏和 Inspector 的撤销归属与可用性。[实测记录](design/interaction-scenarios.md#两个同标题文档的撤销)
- [ ] 移动端系统返回分别取消和完成，核验页面身份、阅读位置和释放次数；记录使用的宿主和设备。

## 从上轮接续的工程问题

- [ ] 核对可编辑文本框的辅助技术选区与文字范围坐标。macOS 实窗已获得 AX 字段边界，`AXSelectedTextRange` 和 `AXBoundsForRange` 返回 `kAXErrorNoValue`。[受控文本框实测](design/input.md#受控文本框实测)
- [ ] 复核工作区已有的挂载中断改动。三个已知触发点是子作用域首次 watch 关窗、替换分支时 cleanup 关窗、移除列表行时 cleanup 关窗。核对后继工厂、绑定写入、释放计数和首帧提交；另行记录真实屏幕结果。[窗口](../crates/octara/tests/integration/windows.rs) · [分支](../crates/octara/tests/integration/scopes.rs) · [列表行](../crates/octara/tests/integration/collection_rows.rs)
- [ ] 横图换竖图、空图变非空及旧绘制列表重画时，核对布局尺寸和像素版本。单张图片过大与多张图耗尽 GPU 容量分别复现。[图片测试](../crates/octara/tests/integration/image.rs)
- [ ] 记录 Windows 实时检查通道和移动宿主的支持范围。没有平台输入或屏幕证据的项目继续保留待验收状态。

过去的决策过程可从 Git 历史查询，例如 `git log --all -- docs/design`。当前设计、实现边界和待办各有一个入口。[架构](ARCHITECTURE.md) · [实现边界](ROADMAP.md)
