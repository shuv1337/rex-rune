use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rexrs::AsyncRexClient;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::oneshot,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<Box<dyn Io>>>;

pub fn local_endpoint() -> String {
    crate::platform::local::default_endpoint()
}

async fn io(endpoint: &str, url: &url::Url) -> Result<Box<dyn Io>> {
    if let Some(path) = endpoint.strip_prefix("unix://") {
        return crate::platform::local::connect(std::path::Path::new(path)).await;
    }
    let host = url.host_str().context("endpoint needs a host")?;
    let port = url
        .port_or_known_default()
        .context("endpoint needs a port")?;
    let tcp = tokio::net::TcpStream::connect((host, port)).await?;
    tcp.set_nodelay(true)?;
    detect_silent_peer(&tcp)?;
    Ok(Box::new(tcp))
}

/// A server that drops this connection while the network is gone (a laptop asleep) never sends
/// a reset, so a client that only listens would wait forever. Probe idle connections, and give
/// up on data the peer never acknowledges, so the pending read fails and the host or terminal
/// reconnects within about 30 seconds of the network returning.
fn detect_silent_peer(tcp: &tokio::net::TcpStream) -> std::io::Result<()> {
    let socket = socket2::SockRef::from(tcp);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(15))
            .with_interval(Duration::from_secs(5))
            .with_retries(3),
    )?;
    crate::platform::net::set_unacknowledged_limit(tcp, Duration::from_secs(30))
}

pub async fn socket(endpoint: &str, path: &str, protocol: &str) -> Result<Socket> {
    socket_with_config(endpoint, path, protocol, None).await
}

/// [`socket`] with the WebSocket limits of `config` (messages, frames) instead of the library's
/// defaults, which allow 64 MiB a message.
pub async fn socket_with_config(
    endpoint: &str,
    path: &str,
    protocol: &str,
    config: Option<WebSocketConfig>,
) -> Result<Socket> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        open_socket(endpoint, path, protocol, config),
    )
    .await
    .context("Rex WebSocket connection timed out")?
}

async fn open_socket(
    endpoint: &str,
    path: &str,
    protocol: &str,
    config: Option<WebSocketConfig>,
) -> Result<Socket> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut request_url = if endpoint.starts_with("unix://") {
        url::Url::parse("ws://localhost")?
    } else {
        let mut url = url::Url::parse(endpoint)?;
        let scheme = match url.scheme() {
            "https" | "wss" => "wss",
            "http" | "ws" => "ws",
            _ => bail!("unsupported endpoint scheme"),
        };
        url.set_scheme(scheme)
            .map_err(|_| anyhow::anyhow!("invalid scheme"))?;
        url
    };
    request_url.set_path(path);
    request_url.set_query(None);
    let mut request = request_url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", protocol.parse()?);
    let stream = io(endpoint, &request_url).await?;
    let (socket, response) =
        tokio_tungstenite::client_async_tls_with_config(request, stream, config, None).await?;
    anyhow::ensure!(
        response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|h| h.to_str().ok())
            == Some(protocol),
        "Rex did not negotiate {protocol}"
    );
    Ok(socket)
}

pub struct Transport {
    pub socket: Socket,
    /// Requests sent past the client, whose responses go to their receiver instead.
    detached: HashMap<String, oneshot::Sender<String>>,
    next_detached: u64,
    max_message_bytes: usize,
}

impl Transport {
    /// Send a request without waiting for it, so other calls and events keep flowing. Its
    /// response arrives on the receiver as raw text, whichever call is reading the socket.
    /// Dropping the receiver abandons it.
    pub async fn detach(
        &mut self,
        method: &str,
        payload: impl serde::Serialize,
    ) -> rexrs::Result<oneshot::Receiver<String>> {
        self.next_detached += 1;
        // The client numbers its own requests; a prefix keeps these apart.
        let id = format!("detached:{}", self.next_detached);
        let text = serde_json::to_string(&serde_json::json!({
            "type": "request", "id": id, "method": method, "payload": payload,
        }))?;
        if text.len() > self.max_message_bytes {
            return Err(rexrs::Error::RequestTooLarge {
                size: text.len(),
                limit: self.max_message_bytes,
            });
        }
        let (reply, receiver) = oneshot::channel();
        self.detached.retain(|_, reply| !reply.is_closed());
        self.detached.insert(id, reply);
        rexrs::AsyncTransport::send(self, text).await?;
        Ok(receiver)
    }
}

#[derive(Deserialize)]
struct ControlFrame {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct DetachedEnvelope<'a, E> {
    #[serde(default)]
    pub seq: u64,
    #[serde(borrow)]
    pub payload: Option<&'a RawValue>,
    pub error: Option<E>,
}

/// Decode a response intercepted by `detach` with the same defaults as an SDK call.
pub(crate) fn detached_result<T: serde::de::DeserializeOwned>(text: &str) -> rexrs::Result<T> {
    Ok(detached_result_with_seq(text)?.value)
}

pub(crate) fn detached_result_with_seq<T: serde::de::DeserializeOwned>(
    text: &str,
) -> rexrs::Result<rexrs::Reply<T>> {
    #[derive(serde::Deserialize)]
    struct Response {
        #[serde(default)]
        seq: u64,
        payload: Option<serde_json::Value>,
        error: Option<Error>,
    }
    #[derive(serde::Deserialize)]
    struct Error {
        code: String,
        #[serde(default)]
        status: u16,
        #[serde(default)]
        message: String,
    }
    let reply: Response = serde_json::from_str(text)?;
    if let Some(error) = reply.error {
        return Err(rexrs::ApiError::from_wire(&error.code, error.status, error.message).into());
    }
    Ok(rexrs::Reply {
        seq: reply.seq,
        value: serde_json::from_value(reply.payload.unwrap_or_else(|| serde_json::json!({})))?,
    })
}

impl rexrs::AsyncTransport for Transport {
    async fn send(&mut self, text: String) -> rexrs::Result<()> {
        self.socket
            .send(Message::Text(text.into()))
            .await
            .map_err(rexrs::Error::transport)
    }
    async fn recv(&mut self) -> rexrs::Result<rexrs::transport::Frame> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(frame) = serde_json::from_str::<ControlFrame>(&text)
                        && frame.kind == "response"
                        && let Some(id) =
                            frame.id.as_deref().filter(|id| id.starts_with("detached:"))
                    {
                        // A cancelled or timed-out read still owns its response ID.
                        // Never hand the late payload to the SDK's generic parser.
                        if let Some(reply) = self.detached.remove(id) {
                            let _ = reply.send(text.to_string());
                        }
                        continue;
                    }
                    return Ok(rexrs::transport::Frame::Text(text.to_string()));
                }
                Some(Ok(Message::Close(frame))) => {
                    return Ok(rexrs::transport::Frame::Closed {
                        code: frame.as_ref().map(|f| f.code.into()),
                        reason: frame.map(|f| f.reason.to_string()).unwrap_or_default(),
                    });
                }
                Some(Ok(_)) => self.socket.flush().await.map_err(rexrs::Error::transport)?,
                Some(Err(error)) => return Err(rexrs::Error::transport(error)),
                None => {
                    return Ok(rexrs::transport::Frame::Closed {
                        code: None,
                        reason: "connection ended".into(),
                    });
                }
            }
        }
    }
    async fn close(&mut self) -> rexrs::Result<()> {
        self.socket
            .close(None)
            .await
            .map_err(rexrs::Error::transport)
    }
}

pub type Client = AsyncRexClient<Transport>;
pub const MIN_API_REVISION: u32 = 10;
pub const API_REVISION: u32 = 24;
pub async fn control(endpoint: &str, reconnect: Option<&str>) -> Result<Client> {
    control_with_label(endpoint, reconnect, &crate::settings::hostname()).await
}

pub async fn control_with_label(
    endpoint: &str,
    reconnect: Option<&str>,
    label: &str,
) -> Result<Client> {
    control_with_max_revision(endpoint, reconnect, label, API_REVISION).await
}

/// Like [`control_with_label`], but caps the client's offered maximum API revision
/// (clamped to `MIN_API_REVISION..=API_REVISION`). Used by protocol probes to verify
/// lower-revision negotiation behavior.
pub async fn control_with_max_revision(
    endpoint: &str,
    reconnect: Option<&str>,
    label: &str,
    max_api_revision: u32,
) -> Result<Client> {
    // This is an attachment-only client. Server lifetime and listener configuration
    // belong to the existing upstream Rex server, including its Tailscale identity.
    let max_rev = max_api_revision.clamp(MIN_API_REVISION, API_REVISION);
    let socket = socket(endpoint, "/api/control", "rex.control.v1").await?;
    connect(
        socket,
        reconnect,
        "Rex Rune",
        label,
        rexrs::ClientKind::App,
        max_rev,
    )
    .await
}

/// Connect a short-lived discovery probe as `ClientKind::Other` so rev 17+ servers never
/// select it as a `rex.ui` prompt target.
pub(crate) async fn discovery_from_socket(socket: Socket, name: &str) -> Result<Client> {
    connect(
        socket,
        None,
        name,
        &crate::settings::hostname(),
        rexrs::ClientKind::Other,
        API_REVISION,
    )
    .await
}

async fn connect(
    socket: Socket,
    reconnect: Option<&str>,
    name: &str,
    label: &str,
    kind: rexrs::ClientKind,
    max_api_revision: u32,
) -> Result<Client> {
    let mut builder = AsyncRexClient::builder()
        .label(label)
        .name(name)
        .version(env!("CARGO_PKG_VERSION"))
        .api_revisions(MIN_API_REVISION..=max_api_revision)
        .kind(kind);
    if let Some(token) = reconnect {
        builder = builder.reconnect_token(token);
    }
    let mut client = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        builder.connect_with(Transport {
            socket,
            detached: HashMap::new(),
            next_detached: 0,
            max_message_bytes: 0,
        }),
    )
    .await
    .context("Rex control WELCOME timed out")??;
    anyhow::ensure!(
        (MIN_API_REVISION..=max_api_revision).contains(&client.welcome().api_revision),
        "Rex negotiated API revision {}; this client supports {MIN_API_REVISION}–{max_api_revision}",
        client.welcome().api_revision
    );
    client.transport().max_message_bytes = client.welcome().limits.max_message_bytes;
    Ok(client)
}

// socket2 exposes the keepalive time and interval getters only on Unix.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_connections_are_set_to_notice_a_silent_peer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        detect_silent_peer(&tcp).unwrap();
        let socket = socket2::SockRef::from(&tcp);
        assert!(socket.keepalive().unwrap());
        assert_eq!(
            socket.tcp_keepalive_time().unwrap(),
            Duration::from_secs(15)
        );
        assert_eq!(
            socket.tcp_keepalive_interval().unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), 3);
        // Linux and macOS have an option for it; elsewhere keepalive is all there is.
        let limit = crate::platform::net::unacknowledged_limit(&tcp).unwrap();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(limit, Some(Duration::from_secs(30)));
        } else {
            assert_eq!(limit, None);
        }
    }
}
