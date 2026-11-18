//! Desktop evidence for the panel fixture through framework touch injection.

use octara::app::{InputEvent, Phase, PointerEvent};
use octara::prelude::*;
use octara_examples::panel_scroll;

fn fixture() -> El {
    let id = current_window();
    let origin = state("sample-origin", || (0.0, 0.0));
    on_command("pan.bottom", || _ = scroll_to(panel_scroll::LIST, 0.0, 95.0));
    for (name, phase, up) in [
        ("pan.down", Phase::Down, 0.0),
        ("pan.forward", Phase::Move, 30.0),
        ("pan.reverse", Phase::Move, 5.0),
        ("pan.cancel", Phase::Cancel, 5.0),
        ("pan.up", Phase::Up, 5.0),
        ("pan.late-move", Phase::Move, 55.0),
    ] {
        on_command(name, move || {
            let Some(mut window) = App::current().window(id) else { return };
            if phase == Phase::Down {
                let at = {
                    let kernel = window.kernel();
                    let Some(list) = kernel.find(panel_scroll::LIST) else { return };
                    let (x, y, width, _) = kernel.rect(list);
                    (x + width / 2.0, y + 20.0)
                };
                origin.set(at);
            }
            let (x, y) = origin.get();
            window.event(InputEvent::Pointer(PointerEvent::touch(1, phase, x, y - up)));
        });
    }
    panel_scroll::view()
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara panel scroll".into(), size: (420.0, 460.0), ..Default::default() },
        fixture,
    );
}
