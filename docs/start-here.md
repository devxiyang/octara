# 用 Octara 0.0.7 构建界面

Octara 用普通 Rust 函数组合视图。应用持有响应式状态和窗口树，宿主负责把树投影到原生窗口或浏览器 DOM。下面的 Rust 示例会随 `cargo test -p octara-cli --doc` 编译并运行。

## 声明窗口与交互

构造函数接收 `BuildCx` 来创建状态。动态属性接收 `ReadCx` 来跟踪读取。事件处理器接收 `WriteCx` 来更新状态或排队请求。纯展示组件可以直接返回 `El`。用 `El::select_none()` 关闭按钮标签等界面文字的文本选择。

用短小的 `#[ui]` 函数表达组件的用途，方便在检查结果中追踪组件来源。把角色、可访问名称和事件处理器放在实际操作的节点上。将静态样式写成 `const STYLE: TwStyle = tw!(...)`，再用 `.tw(STYLE)` 应用。`tw!` 会在编译时检查样式类。

一组固定的子节点用 `.child((a, b, c))`。元组成员可以是不同的 `IntoView` 类型。一组同类型的子节点用 `.children(iter)`，可直接传入数组或迭代器。

```rust
use octara::prelude::*;

const COUNTER_STYLE: TwStyle = tw!("p-4 gap-2");
const BUTTON_STYLE: TwStyle = tw!("px-3 py-2 rounded-md bg-surface");

#[ui]
fn increment_button(count: Signal<u32>) -> El {
    el()
        .tw(BUTTON_STYLE)
        .role(Role::Button)
        .label("Increment")
        .test_id("increment")
        .focusable()
        .select_none()
        .on_press(move |w| w.update(count, |count| *count += 1))
        .child("Increment")
}

#[ui]
fn counter(cx: &mut BuildCx<'_>) -> El {
    let count = cx.signal(0_u32);
    col()
        .tw(COUNTER_STYLE)
        .child((
            text("Counter").role(Role::Heading),
            move |r: &mut ReadCx<'_>| format!("Count {}", r.get(count)),
            increment_button(count),
        ))
}

pub fn startup() -> AppDefinition {
    let options = WindowOptions {
        presentation: WindowPresentation { title: "Counter".into(), ..Default::default() },
        ..Default::default()
    };
    AppDefinition::new().window(WindowDefinition::new(options, counter))
}

# let mut driver = octara_inspect::HeadlessDriver::default();
# let windows = driver.launch(startup()).unwrap();
# assert_eq!(windows.len(), 1);
```

```rust
use octara::prelude::*;

#[ui]
fn tags(labels: &[&str]) -> El {
    row().children(labels.iter().copied().map(text))
}
```

桌面可执行文件把定义交给 `octara_host_native::run_app(startup())`。浏览器把定义和根元素交给 `octara_host_web::WebApp::launch`，并持有返回的 `WebApp`。一个 `WebApp` 拥有一处浏览器挂载。

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
        presentation: WindowPresentation {
            title: "Workspace".into(),
            material: WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar),
            ..Default::default()
        },
        min_size: Some(Size::new(480.0, 320.0)),
        titlebar: TitlebarStyle::HiddenInset { traffic_light_offset: Some(point(14.0, 12.0)) },
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

## 使用平台模块

在应用定义上通过 `.with_module(octara_webview::module())` 显式注册 WebView。Apple 宿主使用 WKWebView，浏览器使用 iframe。macOS 默认使用 Underlay，Octara 弹层可以在 WebView 上方接收点击。iOS 默认使用 Overlay。无头测试可以注册 `octara_webview::mock()`。模块依赖关闭默认 feature 后，无头测试只编译 Rust 契约和 mock。

在同一窗口内用 `octara_webview::send` 和 `test_id` 向已挂载的 WebView 发送类型化命令，通过 `on_typed_native` 接收 `WebViewEvent`。Apple 后端允许受信任的文档通过 Promise 发起 RPC。用 `WebViewProps::html` 加载应用自己的文档，使用 `trust_origin` 明确指定允许外部文档调用桥接的来源。

```rust,ignore
use octara::prelude::*;
use octara_webview::{WebViewCommand, WebViewEvent, WebViewProps, send, view};

fn preview() -> El {
    col().child((
        el().role(Role::Button).focusable().on_press(|w| {
            send(w, "preview", WebViewCommand::Evaluate {
                script: "document.title".into(),
                request_id: Some("preview-title".into()),
            });
        }).child("Read title"),
        view(WebViewProps::html("<title>Preview</title><p>Hello</p>"))
            .test_id("preview")
            .h(260.0)
            .on_typed_native(|_, _, event: &WebViewEvent| {
                if let WebViewEvent::JavaScript { result, .. } = event {
                    println!("{result:?}");
                }
            }),
    ))
}

let app = AppDefinition::new()
    .with_module(octara_webview::module())
    .window(WindowDefinition::new(WindowOptions::default(), |_| preview()));
```

注册 `octara_video::module()` 后，`video(VideoSource)` 和 Markdown 视频使用插件提供的播放器。没有播放器时使用作者指定的封面。文件和目录选择通过 `octara_picker`，在用户手势中用 `WriteCx::call_module` 发送 `PickFiles`、`PickFolder` 或 `PickSavePath`。浏览器可以返回文件内容和名称，原生宿主可以返回真实路径。`file_dropzone` 的点击回调由应用决定调用哪个选择器。注册 `octara_haptics::module()` 后，可以用 `WriteCx::notify_module` 请求触觉反馈。

`octara modules merge` 根据应用的 Cargo 依赖生成 Apple plist、entitlements、framework 列表和 Web headers。把 manifest 所需的变量写在应用包目录的 `octara.toml` 内。打包已有应用时，用 `--ios-info-plist`、`--ios-entitlements`、`--macos-info-plist` 或 `--macos-entitlements` 传入原始文件，保留 bundle 标识符等应用元数据。

```toml
[modules."octara.picker"]
photo_library_usage_description = "Select photos to attach."
```

```sh
cargo run -p octara-cli -- modules merge -p octara-chat-desktop --out-dir target/chat-module-metadata
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

任务绑定到注册它的作用域。虚拟时钟驱动休眠，宿主在 Cycle 中轮询就绪任务。`task.write` 在同步闭包内借出 `WriteCx`。

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

运行 `cargo run -p octara-examples-desktop --bin window -- showcase workspace` 体验数据表、树视图、命令面板、右键菜单和 Toast。工作台也展示了 Navigator 保活和元素拖放重排。

## 定制流式 Markdown

添加 `octara-markdown` 依赖后，用 `markdown(build, source)` 渲染字符串、响应式读取或 `TextBuffer`。用 `markdown_stream` 渲染模型输出，并在输入结束时更新完成信号。缓冲区随后排空，文档切换到完整解析。若上游已经合并增量，可用 `StreamingOptions::pace` 关闭额外的节奏缓冲，继续使用流式筛选和文字揭示效果。增量追加时，已完成的块会共享原有 AST 和挂载节点。

流式渐显默认随 `SystemPreferences.reduced_motion` 更新。用 `MarkdownConfig::streaming_effects_with` 根据响应式状态配置揭示效果，用 `allow_reduced_motion_effects(true)` 显式允许在减弱动态效果模式下使用揭示。`StreamingOptions::effects` 默认沿用作用域内的配置，也可用 `Some(StreamingEffects::None)` 关闭当前文档的揭示效果。

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

启用 `highlight` 可使用内置语法高亮，也可以注入 `Arc<dyn CodeHighlighter>`。高亮器接收以 LF 分行的源码。启用 `media` 可通过 `MediaResolver` 加载图片，并用宿主视频部件播放受支持的 `<video>`。用 `on_context_menu` 获取当前选区的可用复制格式，包括纯文本、Markdown、表格 Markdown 和图片 URL。

用 `TextRecognizer::file_paths` 将正文中的文件路径投影为链接。传入的解析函数会收到完整路径，包括 `src/lib.rs#L42` 和 `./Cargo.toml#L10-L20` 中的行号后缀。用 `LinkPillContent` 更新链接胶囊的标签、图标和颜色。自建富文本可将 `InlinePill::attachment` 交给 `Rich::attachment`，由宿主测量标签和图标并对齐文字基线。附件覆盖的逻辑文本仍可复制。

启用 `math` 后，用 `$…$` 写行内公式，用 `$$…$$` 写独立公式。公式沿用主题文字色，选区复制保留 TeX 源码，过宽的独立公式可横向滚动。通过 `El::theme_patch` 的响应式读取切换主题色时，公式会沿用现有矢量轮廓。用 `BuildCx::provide::<Arc<dyn MathRenderer>>` 注册自定义矢量排版器，或用 `MarkdownConfig::on_math_error` 接收公式解析错误。失败的公式显示等宽源码。

启用 `composer` 后，用 `MarkdownComposer` 构建带实时 Markdown 高亮的多行输入框。用 `TriggerConfig` 配置 `@`、`#` 或 `/` 等触发字符，读取 `active_trigger()` 或直接为 `editor` 提供候选项。浮层跟随编辑器光标。提及按单个实体删除和撤销，由 `markdown()` 输出为 `[@Alice](user://u_1)` 等链接。为受控编辑器提供作用域内的 `Arc<dyn EditorBackend>`，可以定制装饰、附件与原子范围。

补全项的 `Completion` 可通过 `.icon(...)`、`.description(...)` 和 `.category(...)` 添加信息。用 `editor_with_completion_row` 定制建议项的布局，继续使用编辑器的键盘导航和选中行为。

运行 `cargo run -p octara-examples-desktop --bin window -- showcase markdown` 查看流式文档和补全输入框。

## 用 Inspect 验证应用

将应用库和桌面启动包设为同一工作区的两个成员。`my-app/src/lib.rs` 公开 `startup() -> AppDefinition`，并在 `my-app/Cargo.toml` 中声明界面和测试所需的依赖。

```toml
[dependencies]
octara = "0.0.7"

[dev-dependencies]
octara-inspect = "0.0.7"
```

在同级的 `my-app-desktop/Cargo.toml` 中依赖应用库和原生宿主。桌面包的 `src/main.rs` 调用 `octara_host_native::run_app(my_app::startup())`。

```toml
[dependencies]
my-app = { path = "../my-app" }
octara-host-native = "0.0.7"
```

用 `cargo test -p my-app` 单独运行应用库的无头测试。`HeadlessDriver` 可以用 `launch(startup())` 启动同一个应用定义，也可以单独打开待测界面。它用 `octara-pipeline` 计算布局和语义，以 `octara-render` 的 CPU 渲染器生成绘制截图。用 `cargo tree -p my-app` 检查测试依赖里是否有 `octara-host-native`、`octara-gpu`、`wgpu` 或 `objc2`。单独构建应用库可以避免桌面包的依赖 feature 合并到同一次构建中。[Cargo tree 参考](https://doc.rust-lang.org/cargo/commands/cargo-tree.html) · [Cargo feature 解析](https://doc.rust-lang.org/cargo/reference/resolver.html#features)

在无头驱动和真实宿主中都可以通过 `octara.inspect/2` 发送观察与动作。先从观察结果取完整目标。节点被销毁并重建或换窗后重新观察。回执提供送达状态、当前树版本、结算状态和已提交帧。有实际呈现回执的宿主还提供已呈现帧。

```rust
use octara::prelude::*;
use octara_inspect::{HeadlessDriver, protocol::{AgentCommand, AgentRequest}};

let mut driver = HeadlessDriver::default();
let id = driver.app_mut().open_window(WindowOptions::default(), |_| {
    text("Ready").test_id("status")
}).unwrap();
let receipt = driver.execute(AgentRequest::new(Some(id), AgentCommand::Observe));
assert!(receipt.ok && receipt.progress.settled);
let snapshot = receipt.snapshot.unwrap();
assert!(snapshot.nodes.iter().any(|node| node.test_id.as_deref() == Some("status")));
```

CLI 通过应用库公开的 `startup` 函数启动无头会话。若包名为 `my-app`，在该包的 `Cargo.toml` 中登记入口。

```toml
[package.metadata.octara]
entry = "my_app::startup"
```

用 `octara vocab` 查询当前 Rust 源码里的签名，用 `octara inspect` 观察窗口，用 `octara audit` 检查界面问题。

```sh
cargo run -q -p octara-cli -- vocab BuildCx
cargo run -q -p octara-cli -- inspect --manifest-path examples/Cargo.toml --json
cargo run -q -p octara-cli -- audit --manifest-path examples/Cargo.toml --json
```

用 `octara run` 发送同一协议的 JSONL 请求。连接已启动的应用时传入 `--connect SOCKET`。下例请求当前窗口列表。

```json
{"protocol":"octara.inspect/2","op":"list_windows"}
```

从 `octara::diagnostics` 查询运行时诊断或使用 `octara explain CODE`。未结算的投影、失效目标和已经销毁的响应式句柄有各自的诊断。

`octara audit` 检查已结算窗口的文本裁剪、交互控件的无障碍名称和失效的焦点目标。文本报告用 `file:line` 指向节点的定义位置。JSON 报告使用 `octara.audit/1`，按窗口列出问题及其级别、目标和源码位置。错误级问题退出码为 1。窗口无法启动、连接失败或无法完成审计时退出码为 2。只检查一扇窗口时，从 `list_windows` 回执取得 ID 后传给 `--window-id`。使用 `--connect SOCKET` 可以检查现有的宿主窗口。

## 配置开发与测试构建

在工作区根 `Cargo.toml` 设置 Cargo profile。下面的通配覆盖为开发和测试构建中的非工作区依赖设置 `opt-level = 2`。对一般的下游应用，这包括用到的 `octara` 和 `octara-*` crate，以及依赖图中的 `wgpu`、`parley` 和 `taffy`。工作区成员使用默认的 `opt-level = 0`，便于快速迭代应用代码。[Cargo profile 参考](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)

```toml
[profile.dev.package."*"]
opt-level = 2

[profile.test.package."*"]
opt-level = 2
```

依赖优化可以降低 debug 构建中文本排版和渲染的 CPU 开销，具体收益随工作负载而变。在 macOS arm64 上对 `6ea4f49` 的相同代码进行比较时，外部依赖从 `opt-level = 1` 调到 `2`，千项待办启动并完成首次投影的中位耗时从 514.30 ms 降到 463.64 ms。27 个 Chat 和 examples 测试可执行文件的 `__eh_frame` 与 `__unwind_info` 合计从 43,613,036 降到 40,669,392 字节，减少约 6.7%。可用 `xcrun size -m` 查看这两个段在自己项目中的体积。

Octara CLI 将根清单的 `[profile.*]` 表原样复制到生成程序的独立工作区，并使用应用的 Cargo target 目录。Cargo 在生成的工作区中把应用库及其依赖视为工作区外依赖，因此通配覆盖也会命中参与构建的原工作区成员。若希望生成程序以 `opt-level = 0` 编译应用库，可为相应 profile 设置具名包覆盖，例如 `[profile.dev.package.my-app]`。具名包覆盖优先于通配覆盖。[Cargo profile 参考](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)
