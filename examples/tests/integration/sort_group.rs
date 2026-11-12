use octara::prelude::*;
use octara_examples::apps::contacts;
use octara_examples::scenarios::{external, press, toggle};

fn names(h: &Window, team: &str) -> Vec<String> {
    let k = h.kernel();
    k.child_ids(&format!("members-{team}"))
        .iter()
        .map(|c| k.text_of(&format!("name-{}", c.trim_start_matches("contact-"))).unwrap())
        .collect()
}

/// After every operation the incremental tree must equal a from-scratch rebuild.
fn consistent(h: &mut Window) {
    let incremental = h.kernel().tree_dump();
    h.app().hot_reload();
    assert_eq!(incremental, h.kernel().tree_dump(), "incremental and from-scratch trees differ");
}

#[test]
fn sorted_groups_follow_renames_moves_and_adds() {
    let mut h = Window::headless(|| contacts(30));
    assert_eq!(h.kernel().text_of("teams").as_deref(), Some("3 teams"));
    assert_eq!(h.kernel().child_ids("groups"), ["group-blue", "group-green", "group-red"]);
    assert_eq!(h.kernel().text_of("team-blue").as_deref(), Some("blue (10)"));
    for t in ["blue", "green", "red"] {
        let n = names(&h, t);
        let mut sorted = n.clone();
        sorted.sort();
        assert_eq!(n, sorted, "{t} is sorted by name");
    }
    consistent(&mut h);

    // Open a row, then rename it to the front of its group: the row moves, it is not rebuilt.
    let id = h.kernel().child_ids("members-red")[5].trim_start_matches("contact-").to_owned();
    press(&mut h, &format!("open-{id}")).unwrap();
    h.kernel_mut().reset_counters();
    external(&mut h, "rename", &format!("{id}:a-first")).unwrap();
    assert_eq!(h.kernel().child_ids("members-red")[0], format!("contact-{id}"));
    assert!(h.kernel().exists(&format!("detail-{id}")), "row-local state survives a move");
    assert_eq!(h.kernel().counters().nodes_created, 0, "a move creates no nodes");
    consistent(&mut h);

    // Change team: the contact leaves red and joins blue at its sorted position.
    external(&mut h, "team", &format!("{id}:blue")).unwrap();
    assert_eq!(h.kernel().text_of("team-red").as_deref(), Some("red (9)"));
    assert_eq!(h.kernel().text_of("team-blue").as_deref(), Some("blue (11)"));
    assert_eq!(h.kernel().child_ids("members-blue")[0], format!("contact-{id}"));
    consistent(&mut h);

    // Starring changes neither order nor groups.
    toggle(&mut h, &format!("star-{id}")).unwrap();
    assert_eq!(h.kernel().checked_of(&format!("star-{id}")), Some(true));
    consistent(&mut h);

    // Emptying a group removes it; adding brings it back.
    let green = h.kernel().child_ids("members-green");
    for c in green {
        external(&mut h, "team", &format!("{}:red", c.trim_start_matches("contact-"))).unwrap();
    }
    assert_eq!(h.kernel().child_ids("groups"), ["group-blue", "group-red"]);
    consistent(&mut h);
    external(&mut h, "add", "zz:green").unwrap();
    assert_eq!(h.kernel().child_ids("groups"), ["group-blue", "group-green", "group-red"]);
    assert_eq!(h.kernel().text_of("team-green").as_deref(), Some("green (1)"));
    consistent(&mut h);
    assert!(h.app().diagnostics().is_empty(), "{:?}", h.app().diagnostics());
}

mod virtualized {
    use super::consistent;
    use octara::prelude::*;
    use octara_examples::apps::{CONTACT_HEIGHT, HEADER_HEIGHT, contacts_virtual};
    use octara_examples::scenarios::{external, press};

    fn window(h: &Window) -> Vec<String> {
        h.kernel().child_ids("contacts").into_iter().filter(|c| !c.ends_with("-top") && !c.ends_with("-bottom")).collect()
    }

    fn height(h: &Window, id: &str) -> f32 {
        let k = h.kernel();
        let n = k.find(id).unwrap();
        k.rect(n).3
    }

    #[test]
    fn grouped_list_virtualizes_with_two_row_heights() {
        let mut h = Window::headless(|| contacts_virtual(90_000));
        let w = window(&h);
        assert_eq!(w[0], "team-blue");
        // 600 px: one 24 px header, then 16 px rows: (600 - 24) / 16 = 36 rows, + overscan.
        assert!(w.len() < 50, "only the window exists: {}", w.len());
        assert_eq!(height(&h, "team-blue"), HEADER_HEIGHT);
        let total = 3.0 * HEADER_HEIGHT + 90_000.0 * CONTACT_HEIGHT;
        let shown: f32 = w.iter().map(|id| height(&h, id)).sum();
        assert_eq!(height(&h, "contacts-top") + shown + height(&h, "contacts-bottom"), total);
        consistent_flat(&mut h);

        // Scroll to the green header: blue's 30,000 rows and its header come first.
        let green_at = HEADER_HEIGHT + 30_000.0 * CONTACT_HEIGHT;
        h.dispatch(Event::ScrollTo("contacts".into(), 0.0, green_at)).unwrap();
        let w = window(&h);
        let pos = w.iter().position(|id| id == "team-green").expect("green header in window");
        assert_eq!(pos, 5, "overscan rows precede the header");
        assert_eq!(h.kernel().text_of("team-green").as_deref(), Some("green (30000)"));

        // A visible contact changes team: it leaves the window, counts update, nothing else moves.
        let id = w[pos + 1].trim_start_matches("contact-").to_owned();
        press(&mut h, &format!("open-{id}")).unwrap();
        h.kernel_mut().reset_counters();
        external(&mut h, "team", &format!("{id}:red")).unwrap();
        assert_eq!(h.kernel().text_of("team-green").as_deref(), Some("green (29999)"));
        assert!(!h.kernel().exists(&format!("contact-{id}")), "moved out of the window");
        // One row enters the window to fill the gap: a contact row is 5 nodes (row, name, the star's
        // press area and box, open).
        let c = h.kernel().counters();
        assert_eq!((c.nodes_created, c.nodes_destroyed), (5, 6), "one row in; the moved row (with its open detail) out");
        consistent_flat(&mut h);
        assert!(h.app().diagnostics().is_empty(), "{:?}", h.app().diagnostics());
    }

    fn consistent_flat(h: &mut Window) {
        consistent(h);
    }
}
