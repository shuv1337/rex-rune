//! Downloads require a store that cannot follow links or replace files. Windows has no
//! such store yet, so admission fails before creating a file or requesting a data ticket.
#[cfg(unix)]
#[path = "../download/store.rs"]
pub mod store;
#[cfg(windows)]
#[path = "no_downloads.rs"]
pub mod store;

/// Check before starting a download or running its destructive probe.
pub fn available() -> anyhow::Result<()> {
    if cfg!(windows) {
        anyhow::bail!("Downloads are not available on Windows yet");
    }
    Ok(())
}
