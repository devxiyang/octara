//! A join in an app: a rename rewrites the renamed person's rows only; a change the rows do not
//! show rewrites none; a person who goes leaves their rows in place.

use octara::prelude::*;
use octara_examples::apps::inbox;
use octara_examples::scenarios::external;

#[test]
fn a_rename_reaches_its_rows_only() {
    let mut h = App::new(|| inbox(200));
    // Person 4 sent messages 3, 23, 43, … (10 of 200).
    h.kernel_mut().reset_counters();
    external(&mut h, "rename", "4:Ada").unwrap();
    assert_eq!(h.kernel().text_of("from-3").as_deref(), Some("Ada"));
    assert_eq!(h.kernel().text_of("from-4").as_deref(), Some("Person 5"));
    assert_eq!(h.kernel().counters().prop_changes, 10, "the ten rows of person 4");

    h.kernel_mut().reset_counters();
    external(&mut h, "online", "4").unwrap();
    assert_eq!(h.kernel().counters().prop_changes, 0, "rows do not show who is online");

    external(&mut h, "forget", "4").unwrap();
    assert_eq!(h.kernel().text_of("from-3").as_deref(), Some("unknown"));
    assert!(h.kernel().find("message-3").is_some(), "the row stays");
    let before = h.kernel().tree_dump();
    h.hot_reload();
    assert_eq!(h.kernel().tree_dump(), before, "incremental and from-scratch trees differ");
    assert!(h.diagnostics().is_empty(), "{:?}", h.diagnostics());
}
