use octara::window::WindowOptions;

fn main() {
    octara_shell::run_with(
        octara_shell::Options { window: WindowOptions::new("Octara Album Back").size(390.0, 640.0), ..Default::default() },
        octara_examples::album_back::view,
    );
}
