//! The download store's interface on Windows. No destination or partial file can be made.
//! There are no local partials to clean up because admission always refuses the download.
use anyhow::{Result, bail};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Destination {
    _private: (),
}

impl Destination {
    pub fn open(_path: &Path) -> Result<Self> {
        bail!("Downloads are not available on Windows yet")
    }

    pub fn free_space(&self) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Downloads are not available on Windows yet",
        ))
    }

    pub fn create_partial(&self, _name: &str, _records: &Records) -> Result<Partial> {
        bail!("Downloads are not available on Windows yet")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Auto,
    Link,
}

pub struct Partial {
    _private: (),
}

impl Partial {
    pub async fn write(&mut self, _bytes: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Downloads are not available on Windows yet",
        ))
    }

    pub async fn finish_with(self, _name: &str, _how: Placement) -> Result<PathBuf> {
        bail!("Downloads are not available on Windows yet")
    }
}

#[derive(Clone, Debug, Default)]
pub struct Records {
    _private: (),
}

/// Records are never created here. The type preserves the inspection interface for probes.
#[derive(Clone, Debug)]
pub struct Record {
    pub dir: PathBuf,
    pub name: String,
    pub pid: u32,
    pub started: u64,
    pub created: u64,
}

impl Records {
    pub fn at(_path: PathBuf) -> Self {
        Self::default()
    }

    pub fn list(&self) -> Vec<Record> {
        Vec::new()
    }

    /// There is nothing to clean up on a platform that has never saved downloads.
    pub fn clean_up_leftovers(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Windows uses the home folder's Downloads, not an XDG user-dirs file.
pub fn default_directory() -> PathBuf {
    crate::platform::dirs::home().join("Downloads")
}

/// A folder as a person writes it in a setting: absolute, or starting with `~`.
pub fn expand_home(text: &str) -> PathBuf {
    match text.strip_prefix('~') {
        Some("") => crate::platform::dirs::home(),
        Some(rest) if rest.starts_with(['/', '\\']) => {
            crate::platform::dirs::home().join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_downloads_do_not_touch_the_destination_or_records() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let error = Destination::open(&missing).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Downloads are not available on Windows yet"
        );
        assert!(Records::default().clean_up_leftovers().is_empty());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        assert_eq!(
            default_directory(),
            crate::platform::dirs::home().join("Downloads")
        );
    }
}
