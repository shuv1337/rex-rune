//! Files uploaded to a terminal's machine through Rex (`upload_begin`, API revision 11). The
//! server stores each file, then types the paths of a whole batch into the terminal as a paste.
use crate::{transport, wire};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use ring::rand::{SecureRandom, SystemRandom};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::tungstenite::Message;

pub const PROTOCOL: &str = "rex.data.bytes.v1";
pub const MIN_API_REVISION: u32 = 11;
const CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub enum Source {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

/// One file to upload: the name the server stores it under and where its bytes come from.
#[derive(Clone, Debug)]
pub struct File {
    pub name: String,
    pub source: Source,
}

impl File {
    pub fn path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        Self {
            name,
            source: Source::Path(path),
        }
    }
    pub fn bytes(name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            name: name.into(),
            source: Source::Bytes(bytes.into()),
        }
    }
    /// The exact size to declare. Only regular files can be uploaded.
    pub fn size(&self) -> Result<u64> {
        match &self.source {
            Source::Bytes(bytes) => Ok(bytes.len() as u64),
            Source::Path(path) => {
                let meta = std::fs::metadata(path)
                    .with_context(|| format!("{} can't be read", self.name))?;
                ensure!(
                    meta.is_file(),
                    "{} is a folder or special file; only files can be uploaded",
                    self.name
                );
                Ok(meta.len())
            }
        }
    }
}

/// `upload:` and a random version 4 UUID, lower-case, as `upload_begin` requires; also used
/// for batch IDs.
pub fn new_uuid() -> Result<String> {
    let mut b = [0u8; 16];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| anyhow::anyhow!("No randomness for an upload ID"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

/// Streams one reserved file on its ticket: exactly `size` bytes, then an empty message, which
/// ends the upload. Rex closes with 1000 once the batch's paths are typed; any other close
/// means nothing was typed and nothing is left on the server.
pub async fn send(endpoint: &str, ticket: &str, file: &File, size: u64) -> Result<()> {
    let mut socket = transport::socket(endpoint, "/api/data", PROTOCOL).await?;
    wire::handshake(&mut socket, ticket).await?;
    match &file.source {
        Source::Bytes(bytes) => {
            for chunk in bytes.chunks(CHUNK) {
                socket.send(Message::Binary(chunk.to_vec().into())).await?;
            }
        }
        Source::Path(path) => {
            let mut reader = tokio::fs::File::open(path)
                .await
                .with_context(|| format!("{} can't be read", file.name))?
                .take(size);
            let mut sent = 0;
            let mut buffer = vec![0; CHUNK];
            loop {
                let n = reader.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                sent += n as u64;
                socket
                    .send(Message::Binary(buffer[..n].to_vec().into()))
                    .await?;
            }
            ensure!(sent == size, "{} changed while it was uploading", file.name);
        }
    }
    socket.send(Message::Binary(Vec::new().into())).await?;
    let closed = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => {
                    return Ok(frame.map(|f| (u16::from(f.code), f.reason.to_string())));
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(anyhow::Error::from(error)),
                None => return Ok(None),
            }
        }
    })
    .await
    .context("Rex did not finish the upload")??;
    match closed {
        Some((1000, _)) => Ok(()),
        Some((code, reason)) if !reason.is_empty() => bail!("{reason} ({code})"),
        Some((code, _)) => bail!("Rex refused the upload ({code})"),
        None => bail!("The upload connection ended early"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_ids_are_lower_case_version_4_uuids() {
        let id = new_uuid().unwrap();
        assert_eq!(id.len(), 36);
        assert!(id.split('-').map(str::len).eq([8, 4, 4, 4, 12]));
        assert_eq!(&id[14..15], "4");
        assert!("89ab".contains(&id[19..20]));
        assert!(
            id.chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_ne!(id, new_uuid().unwrap());
    }

    #[test]
    fn only_regular_files_have_a_size() {
        let dir = std::env::temp_dir();
        assert!(File::path(dir).size().is_err());
        assert_eq!(
            File::bytes("clipboard.png", vec![1, 2, 3]).size().unwrap(),
            3
        );
        assert_eq!(File::path("/tmp/a b.txt".into()).name, "a b.txt");
    }
}
