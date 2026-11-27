//! Publisher commands. The installed app decides whether to accept a signed package.

use octara_release::signing;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub(crate) enum Action {
    /// Generate a private key file and print the public key to embed in a build.
    Keygen {
        #[arg(long)]
        private_key: PathBuf,
    },
    /// Sign the exact bytes of a native resource document.
    Sign {
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        document: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Publish a signed package to a static origin directory using only its public key.
    Publish {
        #[arg(long)]
        site: PathBuf,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        package: PathBuf,
    },
}

impl Action {
    pub(crate) fn run(self, out: &mut String) -> io::Result<()> {
        match self {
            Self::Keygen { private_key } => {
                let (private, public) = signing::generate_key()?;
                write_new(&private_key, &private)?;
                writeln!(out, "{public}").expect("a String can be written");
            }
            Self::Sign { private_key, document, output } => {
                let packet = signing::sign(&fs::read(private_key)?, File::open(document)?)?;
                write_new(&output, &packet)?;
            }
            Self::Publish { site, public_key, package } => {
                let key = octara_release::public_key(&public_key)?;
                let published = signing::publish(&site, key, File::open(package)?)?;
                writeln!(out, "{}", serde_json::to_string(&published)?).expect("a String can be written");
            }
        }
        Ok(())
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)?;
    Ok(())
}
