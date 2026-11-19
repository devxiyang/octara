fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara Album Back".into(), size: (390.0, 640.0), ..Default::default() },
        octara_examples::album_back::view,
    );
}
