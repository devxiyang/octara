# 用 Octara 写第一个窗口

在工作区中，`octara` 提供视图、状态和窗口，`octara-kit` 提供可选的标准控件，`octara-shell` 将视图放进桌面系统窗口。入门所用的接口在本版本中可运行。下面的 Rust 片段由 `cargo test -p octara-cli --doc` 检查，桌面启动示例只编译。

## 组合视图并观察一次点击

用 `#[ui]` 标记组装界面的函数，以便 Inspector 定位源代码。计数属于当前所有者，文字通过闭包读取它。给按钮一个稳定的操作名称和一个可读标签。

```rust
use octara::prelude::*;
use octara_kit::button;

#[ui]
fn counter() -> El {
    let count = state("count", || 0);
    col().size_full().p_4().gap_2()
        .child(text(move || format!("Count: {}", count.get())).id("count"))
        .child(button("Add one").id("inc").on_press(move || count.update(|n| *n += 1)))
}

fn main() {
    octara_shell::run("Counter", counter);
}
```

同一个返回 `El` 的构造函数可以直接交给桌面或无窗口宿主。`.child`、`show`、`keyed` 和弹层通过 `IntoView` 组合内容。内容工厂可以直接返回元素，例如 `show(move || open.get(), || text("详情"))`。这里断言文字变化可以同时发现按钮没有收到点击、状态没有改变、文字没有订阅状态三类错误。

```rust
let mut app = Window::headless(counter);
app.tap("inc").unwrap();
assert_eq!(app.kernel().text_of("count").as_deref(), Some("Count: 1"));
// Done means clean: no layout problems and no diagnostics.
assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
```

## 哪些读取会更新界面

在构造函数中直接读取会固定当时的值。下面的测试特意触发诊断。要让一个属性跟随数据，可以把 Signal 或读取它的闭包交给该属性。

```rust
use octara::prelude::*;

let app = Window::headless(|| {
    let n = state("n", || 1);
    text(format!("{}", n.get())) // read once: this text never updates
});
assert!(app.app().diagnostics().iter().any(|d| d.code == "STRICT_READ_UNTRACKED"));
// Fix: text(move || format!("{}", n.get())), or untrack(|| n.get()) for an intentional snapshot.
```

业务状态可以是普通结构体。`with` 借用字段，`update` 修改草稿，`memo` 根据草稿推导显示值。不要在同一值的借用回调中再次获取冲突的借用。

```rust
use octara::prelude::*;

struct Draft { title: String, tags: Vec<String> }
let mut app = Window::headless(|| {
    let draft = state("draft", || Draft { title: "Notes".into(), tags: Vec::new() });
    let title: ReadSignal<String> = memo(move || draft.with(|d| format!("{} ({})", d.title, d.tags.len())));
    col().size_full()
        .child(text(title).id("title"))
        .child(el().id("tag").role(Role::Button).label("Tag").size_8()
            .on_press(move || draft.update(|d| d.tags.push("Rust".into()))))
});
app.tap("tag").unwrap();
assert_eq!(app.kernel().text_of("title").as_deref(), Some("Notes (1)"));
assert!(app.app().diagnostics().is_empty());
```

## 集合的 key 和行的内容

快照来源适合小型或重算成本低的集合。`each` 和 `virtual_each` 的行工厂直接返回一个 `El`。通过数据源过滤应省略的行。下例修改文字后，行继续由相同的业务 key 标识。`.id(..)` 供测试和 Inspector 查找当前挂载的控件。

```rust
use octara::prelude::*;

let mut window = Window::headless(|| {
    let items = state("items", || vec![(1u32, "First".to_owned())]);
    col().size_full()
        .child(el().id("rename").size_8().child("Rename")
            .on_press(move || items.update(|items| items[0].1 = "Renamed".into())))
        .child(each(move || items.get(), |key, item| text(item).id(format!("item-{key}"))))
});
window.tap("rename").unwrap();
assert_eq!(window.kernel().text_of("item-1").as_deref(), Some("Renamed"));
assert!(window.app().diagnostics().is_empty());
```

频繁增删时可让 `state_list` 维护增量变更。`read_only()` 提供实时只读视图。行工厂返回一个根元素，增删操作使用相应实体的 key。

```rust
use octara::prelude::*;

#[derive(Clone)]
struct Todo {
    id: u32,
    title: String,
}

fn todos() -> El {
    let items = state_list("todos", || vec![Todo { id: 1, title: "Write docs".into() }], |t: &Todo| t.id);
    let next = state("next", || 2u32);
    col()
        .size_full()
        .child(el().id("add").child("Add").on_press(move || {
            let id = next.get();
            next.set(id + 1);
            items.push(Todo { id, title: format!("Item {id}") });
        }))
        .child(col().id("list").flex_1().overflow_y_scroll().child(each(
            move || items.read_only(),
            move |id, todo| {
                let id = *id;
                row()
                    .id(format!("todo-{id}"))
                    .gap_2()
                    .child(text(move || todo.with(|t| t.title.clone())))
                    .child(el().id(format!("remove-{id}")).child("Remove").on_press(move || items.remove(&id)))
            },
        )))
}

let mut app = Window::headless(todos);
app.tap("add").unwrap();
assert!(app.kernel().exists("todo-2"));
app.tap("remove-1").unwrap();
assert!(!app.kernel().exists("todo-1"));
```

## 快捷键和文本输入

命令名可以被不同输入入口引用。下面在窗口中把保存绑定到平台的主修饰键与 S，再从可观察状态检查调用次数。

```rust
use octara::prelude::*;

let mut window = Window::headless(|| {
    let saved = state("saved", || 0);
    on_command("save", move || saved.update(|n| *n += 1));
    col().size_full().child(text(move || format!("saved {}", saved.get())).id("saved"))
});
window.app().bind_keys([("mod+s", "save", None)]).unwrap(); // `mod` is Cmd on Apple platforms, Ctrl elsewhere
window.press_key("mod+s").unwrap();
assert_eq!(window.kernel().text_of("saved").as_deref(), Some("saved 1"));
```

快捷键由 App 配置。`bind_keys` 整批解析后提交，解除绑定时将命令写为 `None`。`App::keymap()` 返回可编辑的应用配置，`App::set_keymap(...)` 接受同一类型。`App::effective_keymap()` 返回平台默认项和应用配置合成后的只读快照，窗口再按焦点选择生效的命令。

在按钮的 `.disabled(move || !window.command_enabled("edit.undo"))` 绑定中，可用性会随焦点和编辑历史更新。Inspector 快照的 `commands` 也来自当前窗口。检查命令执行后的文本与业务状态，可以区分已投递的编辑请求和应用接受的结果。

编辑回调收到当前提议。组件用 `behavior` 注册内部模型更新，应用追加回调仍然可以读到模型接受的值。测试直接注入框架编辑事件；要验收系统输入法，还需系统键盘与原生窗口。

`text_input()` 和 Kit 的 `text_field(model)`、`number_field(...)` 都返回 `TextInput`。直接链式配置 `.id(...)`、`.rows(...)` 或 `.on_submit(...)`。建议列表 `popup_list` 接受这个编辑器类型，`field` 也可为复选框等其他单节点控件提供标签。`TextInput::value` 绑定应用接受的正文，普通选择器通过 `El::a11y_value` 报告选中项名称。

```rust
use octara::prelude::*;

let mut window = Window::headless(|| {
    let value = state("value", String::new);
    text_input().id("name").value(value)
        .behavior(|b| b.on_input(move |text| value.set(text)))
        .on_input(move |proposed| assert_eq!(value.get(), proposed))
});
window.dispatch(Event::Input("name".into(), "Ada".into())).unwrap();
assert_eq!(window.kernel().value_of("name").as_deref(), Some("Ada"));
```

## 时间和所有者

动画有目标值和显示中的值。虚拟时间可以验证中间帧和最终静止，避免依赖机器速度。

```rust
use octara::prelude::*;
use std::time::Duration;

let mut app = Window::headless(|| {
    let open = state("open", || false);
    col()
        .size_full()
        .motion(Spring::SNAP)
        .child(el().id("toggle").size_8().on_press(move || open.update(|o| *o = !*o)))
        .child(el().id("panel").size_8().bg(Token::Surface).opacity(move || if open.get() { 1.0 } else { 0.0 }))
});
app.tap("toggle").unwrap();
// The style is the target at once; the drawn value moves toward it frame by frame.
assert_eq!(app.kernel().visual_of("panel").unwrap().opacity, 1.0);
app.app().advance(Duration::from_millis(50));
assert!(app.app().next_deadline().is_some(), "still moving: another frame is due");
app.app().run_until_idle(Duration::from_secs(1)).unwrap();
assert!(app.app().next_deadline().is_none(), "at rest");
```

异步任务也可以用应用时钟验证。让任务归属于实际需要它的界面，以便窗口或局部作用域退休后停止交付。

```rust
use octara::prelude::*;
use std::time::Duration;

let mut app = Window::headless(|| {
    let status = state("status", || "loading".to_string());
    spawn(async move {
        sleep(Duration::from_millis(300)).await;
        status.set("ready".into());
    });
    text(move || status.get()).id("status")
});
assert_eq!(app.kernel().text_of("status").as_deref(), Some("loading"));
app.app().run_until_idle(Duration::from_secs(1)).unwrap();
assert_eq!(app.kernel().text_of("status").as_deref(), Some("ready"));
```

跨窗口的共享数据与工作可以在 `App::run` 内创建。宿主结束应用时调用 `shutdown`。

```rust
use octara::prelude::*;
use octara::platform::HeadlessPlatform;
use std::time::Duration;

let app = App::new(HeadlessPlatform::default());
let count = app.run(|| state("count", || 0));
let task = app.run(|| spawn(async move {
    sleep(Duration::from_millis(10)).await;
    count.set(1);
}));
app.poll();
app.advance(Duration::from_millis(10));
assert_eq!(app.run(|| state("count", || 99).get()), 1);
assert!(task.is_finished());
app.shutdown();
assert!(app.is_shutdown());
```

## 初始资源

要保证第一次呈现就使用正确的字体或主题，在创建窗口前配置 App。桌面宿主在 `octara_shell::Options::app` 中接受相应设置。

```rust
use octara::prelude::*;
use octara::platform::HeadlessPlatform;

let app = App::with_config(HeadlessPlatform::default(), AppConfig {
    theme: Theme::dark(),
    ..Default::default()
});
let window = app.open_window(WindowOptions::new("Configured"), || text("Ready")).unwrap();
assert_eq!(window.inspect().theme, "dark");
app.shutdown();
```

同一个根函数可以交给桌面宿主、`Window::headless` 和 `octara check`。根函数可直接返回 `El`、`View` 或其他 `IntoView` 内容。CLI 从应用的 `[package.metadata.octara]` 表读取 `view` 的函数路径。检查矩阵在每次执行根函数前设置平台、主题和尺寸。`octara inspect` 与 `octara run` 会在挂载前应用传入的 `--theme` 和 `--size`。

## 进入真实窗口

在仓库根目录打开一个桌面示例。第二个终端可以通过第一个终端指定的 Unix socket 读取这个进程的语义树。下面的连接方式适用于 macOS 和 Linux，socket 路径要尚未使用。

```sh
OCTARA_INSPECT=/tmp/octara-first.sock cargo run -p octara-examples --bin window -- counter
```

```sh
cargo run -q -p octara-cli -- inspect --connect /tmp/octara-first.sock --json
```

取得目标后，操作应带上这次观察返回的 `target`。若目标已失效，重新观察当前窗口；若反馈显示前序输入已经送达，先检查实际结果。真实窗口 Inspector 可以验证框架事件，系统输入法候选和系统返回手势还需要对应的操作系统入口。查 API 用 `octara vocab <name>`，查诊断用 `octara explain <code>`。
