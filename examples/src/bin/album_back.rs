use octara_examples::{album_back, window_options};

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples::native::run(window_options("Octara Album Back", 390.0, 640.0), album_back::view)
}
