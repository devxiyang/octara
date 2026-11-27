use std::{env, fs, path::PathBuf};

fn main() {
    // Cargo copies the external package readme into the root of the published archive.
    let packaged = PathBuf::from("start-here.md");
    let guide = if packaged.is_file() { packaged } else { PathBuf::from("../../docs/start-here.md") };
    println!("cargo:rerun-if-changed={}", guide.display());
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR")).join("start-here.md");
    fs::copy(&guide, output).unwrap_or_else(|error| panic!("{}: {error}", guide.display()));
}
