//! The several-windows example: a note opens in a window of its own and closes itself.

use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara_examples::apps;

#[test]
fn notes_open_and_close_their_own_windows() {
    let app = App::new(HeadlessPlatform::default());
    let mut main = app.open_window(WindowOptions::new("Windows"), apps::windows).unwrap();
    main.tap("new").unwrap();
    main.tap("new").unwrap();
    let ids = app.windows();
    assert_eq!(ids.len(), 3);
    let mut note = app.window(ids[2]).unwrap();
    assert_eq!(note.options().title, "Note 2");
    note.tap("close").unwrap();
    assert_eq!(app.windows(), vec![ids[0], ids[1]]);
    assert_eq!(main.kernel().text(main.kernel().find("opened").unwrap()), "2 notes opened");
}
