use octara_examples::{panel_scroll, window_options};

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples_desktop::native::run(window_options("Octara panel scroll", 420.0, 460.0), panel_scroll::view)
}
