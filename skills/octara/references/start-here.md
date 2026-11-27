# 用 Octara 0.0.6 构建界面

Octara 用普通 Rust 函数组合视图。应用持有响应式状态和窗口树，宿主负责把树投影到原生窗口或浏览器 DOM。下面的 Rust 示例会随 `cargo test -p octara-cli --doc` 编译并运行。

## 声明窗口与交互

构造函数接收 `BuildCx` 来创建状态。动态属性接收 `ReadCx` 来跟踪读取。事件处理器接收 `WriteCx` 来更新状态或排队请求。纯展示组件可以直接返回 `El`。用 `El::select_none()` 关闭按钮标签等界面文字的文本选择。

```rust
use octara::prelude::*;

#[ui]
fn counter(cx: &mut BuildCx<'_>) -> El {
    let count = cx.signal(0);
    col()
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("Count {}", r.get(count))))
        .child(
            el()
                .role(Role::Button)
                .label("Increment")
                .test_id("increment")
                .focusable()
                .select_none()
                .on_press(move |w| w.update(count, |count| *count += 1))
                .child("Increment"),
        )
}

fn app() -> AppDefinition {
    let options = WindowOptions { title: "Counter".into(), ..Default::default() };
    AppDefinition::new().window(WindowDefinition::new(options, counter))
}

```

桌面可执行文件把定义交给 `octara_host_native::run_app(app())`。浏览器把定义和根元素交给 `octara_host_web::WebApp::launch`，并持有返回的 `WebApp`。一个 `WebApp` 拥有一处浏览器挂载。

文件放置目标通过 `El::on_file_drop` 声明。回调接收 `WriteCx` 和一组 `DroppedFile` 引用。原生宿主提供本地路径，浏览器提供文件显示名。

用 `BuildCx::menus` 注册随响应式状态更新的窗口菜单。用 `BuildCx::on_chord` 注册作用域快捷键。`mod` 在 macOS 上是 Command，在其他平台上是 Control。菜单和快捷键可以共用一个命令处理器。这两个注册方法也可用于 `WindowDefinition`。

```rust
use octara::prelude::*;

fn inspector(cx: &mut BuildCx<'_>) -> El {
    let open = cx.signal(false);
    cx.menus(move |r| vec![Menu {
        name: "View".into(),
        items: vec![MenuItem::Action {
            id: "inspector.toggle".into(),
            label: "Toggle Inspector".into(),
            enabled: true,
            checked: Some(r.get(open)),
            shortcut: Some("mod+i".into()),
        }],
    }]);
    cx.on_chord("mod+i".parse().unwrap(), |w| w.request_command("inspector.toggle"));

    el()
        .on_command("inspector.toggle", move |w| w.update(open, |open| *open = !*open))
        .child(show(move |r| r.get(open), |_| text("Inspector")))
}
```

## 配置原生窗口与几何

`WindowOptions` 可指定初始位置、内容尺寸范围、是否可调整大小和初始最大化状态。用 `BuildCx::window_geometry()` 订阅宿主报告的位置、内容尺寸、缩放比例、最大化和全屏状态。事件处理器可用 `WriteCx` 修改当前窗口的标题和几何、最小化窗口或切换最大化，也可按 `WindowId` 激活另一扇窗口。`WindowGeometryRequest` 只更新填写的字段，实际结果由宿主报告。

为每扇需要恢复几何的窗口指定稳定且唯一的 `persistence_key`。导出 `RestartState` 时会包含这些窗口的几何。把状态交给下次启动时的 `AppDefinition::launch` 即可恢复。

macOS 上的 `TitlebarStyle::HiddenInset` 让内容延伸到原生标题栏，`BuildCx::window_chrome()` 提供标题栏高度和交通灯按钮占用的区域。用 `window_drag_region(true)` 标记可拖拽区域，双击可切换最大化。其中的按钮、输入框与其他交互控件拥有自己的点击和焦点。`WindowMaterial::Vibrancy` 通过 AppKit 为透明区域提供系统材质。其他平台按各自支持的窗口能力呈现。

```rust
use octara::prelude::*;

fn workspace() -> WindowDefinition {
    let options = WindowOptions {
        title: "Workspace".into(),
        min_size: Some(Size::new(480.0, 320.0)),
        titlebar: TitlebarStyle::HiddenInset { traffic_light_offset: Some(point(14.0, 12.0)) },
        material: WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar),
        persistence_key: Some("workspace".into()),
        ..WindowOptions::default()
    };
    WindowDefinition::new(options, |build| {
        let chrome = build.window_chrome();
        let geometry = build.window_geometry();
        col()
            .child(row()
                .style_with(move |r, style| style.pl(r.get(chrome).leading_inset))
                .window_drag_region(true)
                .child(el().role(Role::Button).focusable().on_press(|w| {
                    w.set_window_title("Workspace settings");
                }).child("Settings")))
            .child(text(move |r: &mut ReadCx<'_>| {
                let size = r.get(geometry).size;
                format!("{:.0} × {:.0}", size.width, size.height)
            }))
    })
}
```

## 调用原生 WebView

用 `native_view("web")` 在 macOS 或 iOS 上挂载 `WKWebView`。原生属性接受 `url`，或接受配合可选 `base_url` 的 `html`。macOS 上的 `NativeSlot::Underlay` 让 Octara 的菜单、弹层及对话框在 WebView 上方接收点击。`Overlay` 把原生视图放在 Octara 表面上方，也是 iOS 的 WebView 槽位。

在同一个窗口内通过 `test_id` 向已挂载的原生节点发送命令。`evaluate_js` 接收 `script` 和可选的 `request_id`，通过 `js_result` 返回对应的 `value` 或 `error`。还有 `go_back`、`go_forward`、`reload`、`stop_loading`、`find` 和 `clear_find`。`find` 接收 `query`，以及可选的 `backwards` 和 `case_sensitive`。

页面脚本可以调用 `window.webkit.messageHandlers.octara.postMessage(payload)`。在 `on_native` 中接收 `post_message` 的 JSON 数据以及 `load`、`fail`、`title`。`navigation` 提供 `url`、`title`、`loading`、`can_go_back` 和 `can_go_forward`。

```rust
use octara::prelude::*;
use serde_json::json;

fn preview() -> El {
    col()
        .child(el().role(Role::Button).focusable().on_press(|w| {
            w.send_native_command("preview", "evaluate_js", json!({
                "script": "document.title", "request_id": "preview-title"
            }));
        }).child("Read title"))
        .child(native_view("web")
            .test_id("preview")
            .h(260.0)
            .native_props(json!({"html": "<title>Preview</title><p>Hello</p>"}))
            .on_native(|_, event| {
                if event.name == "js_result" {
                    println!("{}", event.data);
                }
            }))
}
```

## 读取、更新与清理

`cx.signal(value)` 把值交给当前作用域。句柄可以复制到事件处理器或派生闭包。用 `r.with(handle, f)` 借用值，用 `r.get(handle)` 取得独立的值。这两个方法接受实现了 `Readable<T>` 的句柄，包括信号、Store 字段、列表行和绑定。构造期间可用 `cx.sample(handle)` 或 `cx.read(|r| …)` 做一次非订阅读取。

`Binding<T>` 是可复制的双向绑定。用 `signal.into()` 直接绑定信号，或用 `cx.binding(read, write)` 定义读写逻辑。`octara-kit` 的受控控件接收 `Binding<T>`。在读写闭包中用 `r.get(binding)` 和 `w.set(binding, next)`。

```rust
use octara::prelude::*;

fn editor(cx: &mut BuildCx<'_>) -> El {
    let title = cx.persisted("title", || "Untitled".to_owned());
    let value: Binding<String> = title.into();
    let initial_title = cx.sample(value);
    let empty = cx.memo(move |r| r.with(value, String::is_empty));
    col()
        .child(input()
            .value(move |r: &mut ReadCx<'_>| r.get(value))
            .label("Title")
            .on_input(move |w, next| w.set(value, next.into())))
        .child(text(format!("Opened as {initial_title}")))
        .child(show(move |r| r.get(empty), |_| text("Enter a title")))
}
```

`#[derive(Store)]` 为结构体生成字段访问器。`cx.store(init)` 持有整份状态，`store.field()` 返回可复制的字段句柄。单独更新一个字段只通知该字段和包含它的 Store。读写整份状态时直接传入 Store 句柄。给嵌套字段加上 `#[store(nested)]` 即可继续访问其子字段。

```rust
use octara::prelude::*;

#[derive(Store)]
struct Document {
    title: String,
    views: u32,
}

fn document(cx: &mut BuildCx<'_>) -> El {
    let document = cx.store(Document { title: "Untitled".into(), views: 0 });
    let title = document.title();
    let views = document.views();
    col()
        .child(input()
            .value(title)
            .label("Title")
            .on_input(move |w, next| {
                w.set_if_changed(title, next.to_owned());
            }))
        .child(text(move |r: &mut ReadCx<'_>| format!("Views: {}", r.get(views))))
        .child(el().role(Role::Button).focusable().on_press(move |w| {
            w.mutate(views, |views| {
                if *views == 10 {
                    return false;
                }
                *views += 1;
                true
            });
        }).child("Record view"))
}
```

`cx.memo` 计算派生值并比较新旧结果。`cx.memo_with(init, |r, value| …)` 复用已有存储。与 `w.mutate(handle, |value| …)` 一样，回调在值发生变化时返回 `true`。`cx.trigger()` 创建无载荷通知，计算中用 `r.track(trigger)` 订阅，事件处理中用 `w.notify(trigger)` 使订阅者重新计算。

`cx.persisted` 使用作用域路径和业务键来识别可恢复状态。`AppDefinition::state_schema` 标识数据结构。退出、切换发布版本或更换进程时可以导出 `RestartState`，再从同一 schema 恢复持久值。用 `scope` 或列表行建立独立寿命，用 `cx.on_cleanup` 释放外部资源。清理回调中的 `CleanupCx` 提供采样能力。

任务绑定到注册它的作用域。虚拟时钟驱动休眠，宿主在 Turn 中轮询就绪任务。`task.write` 在同步闭包内借出 `WriteCx`。

```rust
use octara::prelude::*;
use std::time::Duration;

fn delayed_label(cx: &mut BuildCx<'_>) -> El {
    let ready = cx.signal(false);
    cx.spawn("mark-ready", move |task| async move {
        task.sleep(Duration::from_millis(250)).await;
        task.write(|w| w.set(ready, true));
    });
    text(move |r: &mut ReadCx<'_>| if r.get(ready) { "Ready" } else { "Waiting" })
}
```

## 用业务键更新列表

`each` 按业务键复用已挂载的行，并给行构造闭包传入业务键引用和可复制的 `RowSignal<T>`。`virtual_each` 根据宿主测得的视口选择驻留行，保留拥有焦点或指针触点的行。

用 `r.get(row)` 或 `r.with(row, f)` 读取确定存在的行。行可能暂时缺失时用 `r.try_get(row)` 或 `r.try_with(row, f)`，它们返回 `Option`。在列表之外可用 `cx.row_signal(list, key)` 创建同样的句柄。读取只订阅指定业务键。`WriteCx` 也可读取行，更新行则用 `w.update_row(list, &key, f)`。

已有列表可以用 `filter_with`、`map_list`、`group_by` 等增量算子组合。`filter_with` 接收显式参数信号，筛选闭包只借用行数据和参数值。列表来自普通信号或多个响应式读取时，用 `cx.derive_list` 返回带业务键的行。

```rust
use octara::prelude::*;

fn tasks(cx: &mut BuildCx<'_>) -> El {
    let rows = cx.list::<u64, String>();
    col()
        .child(
            el().role(Role::Button).focusable().test_id("add").on_press(move |w| {
                let id = w.len(rows) as u64;
                w.push_row(rows, id, format!("Task {id}"));
            }).child("Add"),
        )
        .child(each(rows, |_, _, row| {
            text(move |r: &mut ReadCx<'_>| r.try_get(row).unwrap_or_default())
        }))
}

fn selected_tasks(cx: &mut BuildCx<'_>, selection: Signal<Vec<u64>>) -> El {
    let rows = cx.derive_list(move |r| {
        r.with(selection, |ids| ids.iter().map(|&id| (id, format!("Task {id}"))).collect())
    });
    col().child(each(rows, |_, _, row| {
        text(move |r: &mut ReadCx<'_>| r.try_get(row).unwrap_or_default())
    }))
}
```

## 组合 Kit 外观与 Headless 行为

`octara-kit` 为原生和 Web 提供主题控件。用 `button(label, size, variant)` 指定尺寸和视觉强调。组合自定义控件时，从 `octara_kit::parts` 获取 `button_part`、`editor_style`、`popup_surface` 等带主题的部件，从 `octara_kit::headless` 获取行为。复选框接受 `Binding<bool>` 或 `Binding<Checked>`。

```rust
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button, headless, toggle_button};

fn toolbar(build: &mut BuildCx<'_>) -> El {
    let pinned = build.signal(false);
    let included = build.signal(Checked::Mixed);
    row()
        .gap(sp(2.0))
        .child(button("Save", KitSize::Md, ButtonVariant::Primary))
        .child(toggle_button("Pin", pinned.into(), KitSize::Sm, ButtonVariant::Ghost))
        .child(headless::checkbox::checkbox(
            text("Include archived").px(sp(2.0)).rounded(Radius::Full),
            included.into(),
        ))
}
```

用 `headless::Selection` 配置单选或多选。选项用 `headless::Choice` 表示，列表用独立的业务键区分重名项。`PickerOptions::Virtual` 按视口挂载选项。传入自己的行工厂可以在实际的选项节点上布置图标、标签和说明。通过 `PopupOptions` 配置浮层的位置、宽度、最大高度与表面样式，用 `ComboboxOptions` 同时配置组合框的选项列表、浮层与输入框。

```rust
use octara::prelude::*;
use octara_kit::{ButtonVariant, PickerOptions, PopupOptions, Size as KitSize, button, choice_row, headless, select};

fn model_picker(build: &mut BuildCx<'_>) -> View {
    let choices = build.list_from([
        ("fast", headless::Choice::new("Fast")),
        ("deep", headless::Choice::new("Deep")),
    ]);
    let model = build.signal(Some("fast"));
    select(
        "model",
        "Model",
        headless::Selection::Single(model.into()),
        choices.read_only(),
        PickerOptions::Standard,
        PopupOptions::default(),
        |caption| button(caption, KitSize::Md, ButtonVariant::Secondary),
        choice_row,
    )
}
```

在构造控件前调用 `build.provide(octara_kit::KitLocale::zh_cn())`，让当前作用域内的 Kit 控件使用简体中文的系统标签。可通过 `KitLocale` 的字段覆盖个别标签。

运行 `cargo run -p octara-examples --bin window -- showcase workspace` 体验数据表、树视图、命令面板、右键菜单和 Toast。工作台也展示了 Navigator 保活和元素拖放重排。

## 定制流式 Markdown

添加 `octara-markdown` 依赖后，用 `markdown(build, source)` 渲染字符串、响应式读取或 `TextBuffer`。用 `markdown_stream` 渲染模型输出，并在输入结束时更新完成信号。缓冲区随后排空，文档切换到完整解析。若上游已经合并增量，可用 `StreamingOptions::pace` 关闭额外的节奏缓冲，继续使用流式筛选和文字揭示效果。增量追加时，已完成的块会共享原有 AST 和挂载节点。

用 `markdown_with(build, source, config)` 为单个文档传入渲染配置。通过 `ParseOptions` 控制表格、任务列表、GFM 提示等语法。主题可选 `MarkdownTheme::document()`、`chat()` 或 `compact()`，也可以用 `theme_with` 跟随应用状态。`on_link` 提供主键按压、次键按压和悬停事件及视口坐标。

```rust
use octara::prelude::*;
use octara_markdown::{MarkdownConfig, markdown_with};

fn notes(build: &mut BuildCx<'_>, source: TextBuffer) -> El {
    let config = MarkdownConfig::chat().on_link(|w, event| {
        if event.kind != LinkEventKind::Press {
            return false;
        }
        if let Some(path) = event.href.strip_prefix("workspace:") {
            w.set_window_title(path);
            return true;
        }
        false
    });
    markdown_with(build, source, config)
}
```

用 `BuildCx::provide::<Arc<dyn BlockSlotRenderer>>` 为一个作用域注册代码块、独立链接、图片、视频、公式和指令渲染器。`render_slot` 返回 `None` 时使用默认部件。代码、公式和指令的正文与完成状态通过信号更新，围栏闭合时保留组件状态。`DocumentStore::snapshot` 提供按文档顺序排列的图片、链接和槽位索引。

启用 `highlight` 可使用内置语法高亮，也可以注入 `Arc<dyn CodeHighlighter>`。启用 `media` 可通过 `MediaResolver` 加载图片，并用宿主视频部件播放受支持的 `<video>`。用 `on_context_menu` 获取当前选区的可用复制格式，包括纯文本、Markdown、表格 Markdown 和图片 URL。

启用 `math` 后，用 `$…$` 写行内公式，用 `$$…$$` 写独立公式。公式沿用主题文字色，选区复制保留 TeX 源码，过宽的独立公式可横向滚动。通过 `El::theme_patch` 的响应式读取切换主题色时，公式会沿用现有矢量轮廓。用 `BuildCx::provide::<Arc<dyn MathRenderer>>` 注册自定义矢量排版器，或用 `MarkdownConfig::on_math_error` 接收公式解析错误。失败的公式显示等宽源码。

启用 `composer` 后，用 `MarkdownComposer` 构建带实时 Markdown 高亮的多行输入框。用 `TriggerConfig` 配置 `@`、`#` 或 `/` 等触发字符，读取 `active_trigger()` 或直接为 `editor` 提供候选项。浮层跟随编辑器光标。提及按单个实体删除和撤销，由 `markdown()` 输出为 `[@Alice](user://u_1)` 等链接。为受控编辑器提供作用域内的 `Arc<dyn EditorBackend>`，可以定制装饰、附件与原子范围。

运行 `cargo run -p octara-examples --bin window -- showcase markdown` 查看流式文档和补全输入框。

## 用 Inspect 验证应用

在 headless 驱动和真实宿主中都可以通过 `octara.inspect/2` 发送观察与动作。先从观察结果取完整目标；节点被销毁并重建或换窗后重新观察。回执提供送达状态、当前树版本、结算状态和已提交帧。有实际呈现回执的宿主还提供已呈现帧。

```rust
use octara::prelude::*;
use octara_inspect::{HeadlessDriver, protocol::{AgentCommand, AgentRequest}};

let mut driver = HeadlessDriver::new(Default::default());
let id = driver.app_mut().open_window(WindowOptions::default(), |_| {
    text("Ready").test_id("status")
}).unwrap();
let receipt = driver.execute(AgentRequest::new(Some(id), AgentCommand::Observe));
assert!(receipt.ok && receipt.progress.settled);
let snapshot = receipt.snapshot.unwrap();
assert!(snapshot.nodes.iter().any(|node| node.test_id.as_deref() == Some("status")));
```

查询当前 Rust 源码里的签名，或者让 CLI 用应用的 Cargo metadata 入口启动 headless 会话。

```sh
cargo run -q -p octara-cli -- vocab BuildCx
cargo run -q -p octara-cli -- inspect --manifest-path examples/Cargo.toml --json
cargo run -q -p octara-cli -- audit --manifest-path examples/Cargo.toml --json
```

应用在 `[package.metadata.octara]` 的 `entry` 字段声明返回 `AppDefinition` 的公开 Rust 函数。连接已启动的应用时，把同一协议的 JSONL 请求交给 `octara run`。下例请求当前窗口列表。

```json
{"protocol":"octara.inspect/2","op":"list_windows"}
```

从 `octara::diagnostics` 查询运行时诊断或使用 `octara explain CODE`。未结算的投影、失效目标和已经销毁的响应式句柄有各自的诊断。

`octara audit` 检查已结算窗口的文本裁剪、交互控件的无障碍名称和失效的焦点目标。文本报告用 `file:line` 指向节点的定义位置。JSON 报告使用 `octara.audit/1`，按窗口列出问题及其级别、目标和源码位置。错误级问题退出码为 1。窗口无法启动、连接失败或无法完成审计时退出码为 2。只检查一扇窗口时，从 `list_windows` 回执取得 ID 后传给 `--window-id`。使用 `--connect SOCKET` 可以检查现有的宿主窗口。
