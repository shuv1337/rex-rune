//! The freedesktop notification service over D-Bus, on a worker thread so the bus never
//! blocks the UI. Calls and signals are pinned to the service's current owner, so ids
//! from a replaced service are never reused.
use crate::app::notify::{Backend, Event, Note, Service, Session};
use futures_util::StreamExt;
use std::collections::HashMap;
use zbus::{Connection, Proxy, Variant};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The picture each notification shows. GNOME draws only a small, plain app icon without one.
const IMAGE: &[u8] = include_bytes!("../../../resources/icons/128x128/apps/dev.shuv.rex-rune.png");

const DISCONNECTED: &str = "Session bus disconnected";

/// `IMAGE` as the spec's `image-data` hint, `(iiibiiay)`: width, height, row stride, alpha,
/// bits per sample, channels and RGBA bytes. Sent with every notification, so it needs no
/// installed icon file.
fn image_data() -> Option<Variant<'static>> {
    let image = image::load_from_memory_with_format(IMAGE, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let (width, height) = (width as i32, height as i32);
    Some(Variant::from((
        width,
        height,
        width * 4,
        true,
        8i32,
        4i32,
        image.into_raw(),
    )))
}

struct Bus {
    proxy: Proxy<'static>,
    signals: zbus::proxy::SignalStream<'static>,
    identity: String,
}

pub struct Desktop {
    bus: Option<Bus>,
    /// Decoded once for the worker's life.
    image: Option<Variant<'static>>,
}

/// The service answered a call on its pinned connection with an error: the connection
/// and its signals are still good. Only asked of `Notify` and `CloseNotification`, never
/// of connecting: an error while finding or starting the service comes from the bus.
fn answered(error: &zbus::Error) -> bool {
    use zbus::fdo::Error::*;
    match error {
        zbus::Error::MethodError(..) => true,
        zbus::Error::FDO(error) => !matches!(
            **error,
            ServiceUnknown(_)
                | NameHasNoOwner(_)
                | NoReply(_)
                | Timeout(_)
                | TimedOut(_)
                | NoServer(_)
                | Disconnected(_)
                | SpawnExecFailed(_)
                | SpawnForkFailed(_)
                | SpawnChildExited(_)
                | SpawnChildSignaled(_)
                | SpawnFailed(_)
                | SpawnFailedToSetup(_)
                | SpawnConfigInvalid(_)
                | SpawnServiceNotValid(_)
        ),
        _ => false,
    }
}

/// A call to the pinned service, bounded. `Rejected` is its refusal; the flag says the
/// connection can no longer be trusted.
async fn pinned<T>(
    future: impl std::future::Future<Output = zbus::Result<T>>,
) -> Result<T, (Service, bool)> {
    match tokio::time::timeout(std::time::Duration::from_secs(3), future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) if answered(&error) => Err((Service::Rejected, false)),
        Ok(Err(error)) => Err((failure(error), true)),
        Err(_) => Err((Service::Unreachable("Service did not respond".into()), true)),
    }
}

fn failure(error: zbus::Error) -> Service {
    // Short, stable reasons; don't put daemon-controlled diagnostics in Settings.
    let reason = match error {
        zbus::Error::FDO(error) => match *error {
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                return Service::NotFound;
            }
            zbus::fdo::Error::NoReply(_) | zbus::fdo::Error::Timeout(_) => {
                "Service did not respond"
            }
            _ => "Could not reach the service",
        },
        zbus::Error::InputOutput(_) => DISCONNECTED,
        _ => "Could not reach the service",
    };
    Service::Unreachable(reason.into())
}

async fn bounded<T>(
    future: impl std::future::Future<Output = zbus::Result<T>>,
) -> Result<T, Service> {
    tokio::time::timeout(std::time::Duration::from_secs(3), future)
        .await
        .map_err(|_| Service::Unreachable("Service did not respond".into()))?
        .map_err(failure)
}

impl Desktop {
    pub fn new() -> Self {
        Self {
            bus: None,
            image: image_data(),
        }
    }

    async fn connect() -> zbus::Result<Bus> {
        let conn = Connection::session().await?;
        let service = Proxy::new(&conn, DEST, PATH, DEST).await?;
        // A real answer, not merely a connection to the session bus. Also allows D-Bus
        // activation of an installed notification daemon.
        let _: (String, String, String, String) = service.call("GetServerInformation", &()).await?;
        let owner = zbus::fdo::DBusProxy::new(&conn)
            .await?
            .get_name_owner(DEST.try_into().expect("constant bus name"))
            .await?;
        let identity = format!("{}:{owner}", conn.server_guid());
        // Pin calls/signals to this daemon: ids from a replaced service are never reused.
        let proxy = Proxy::new_owned(conn, owner.to_string(), PATH, DEST).await?;
        let signals = proxy.receive_all_signals().await?;
        Ok(Bus {
            proxy,
            signals,
            identity,
        })
    }
}

impl Backend for Desktop {
    async fn ready(&mut self) -> Result<String, Service> {
        if let Some(bus) = &self.bus {
            let conn = bus.proxy.connection();
            let owner = bounded(async {
                Ok(zbus::fdo::DBusProxy::new(conn)
                    .await?
                    .get_name_owner(DEST.try_into().expect("constant bus name"))
                    .await?)
            })
            .await;
            match owner {
                Ok(owner) if format!("{}:{owner}", conn.server_guid()) == bus.identity => {
                    let _: (String, String, String, String) =
                        bounded(bus.proxy.call("GetServerInformation", &())).await?;
                    return Ok(bus.identity.clone());
                }
                _ => self.bus = None,
            }
        }
        let bus = bounded(Self::connect())
            .await
            .map_err(|error| match error {
                // There was no connection to lose.
                Service::Unreachable(reason) if reason == DISCONNECTED => {
                    Service::Unreachable("Could not connect to the session bus".into())
                }
                error => error,
            })?;
        let identity = bus.identity.clone();
        self.bus = Some(bus);
        Ok(identity)
    }

    async fn send(&mut self, note: &Note, replaces: u32) -> Result<u32, Service> {
        let bus = self.bus.as_ref().expect("ready before send");
        let mut hints = HashMap::from([
            ("desktop-entry", Variant::from("dev.shuv.rex-rune")),
            ("urgency", Variant::from(note.kind.urgency())),
        ]);
        if let Some(image) = self.image.as_ref().and_then(|i| i.try_clone().ok()) {
            hints.insert("image-data", image);
        }
        let actions = if note.target.is_some() {
            vec!["default", "Show"]
        } else {
            vec![]
        };
        let body = (
            "Rex",
            replaces,
            "dev.shuv.rex-rune",
            &note.summary,
            &note.body,
            actions,
            hints,
            -1i32,
        );
        // A refusal keeps the connection: reconnecting would take a new bus name and
        // lose the clicks and closes of notifications already sent.
        let result = pinned(bus.proxy.call::<_, _, u32>("Notify", &body)).await;
        result.map_err(|(error, lost)| {
            if lost {
                self.bus = None;
            }
            error
        })
    }

    async fn close(&mut self, id: u32) -> Result<(), Service> {
        let Some(bus) = &self.bus else {
            return Err(Service::Unreachable(DISCONNECTED.into()));
        };
        pinned(bus.proxy.call("CloseNotification", &(id,)))
            .await
            .map_err(|(error, _)| error)
    }
}

impl Session for Desktop {
    fn identity(&self) -> Option<String> {
        self.bus.as_ref().map(|bus| bus.identity.clone())
    }

    async fn event(&mut self) -> Event {
        loop {
            let Some(bus) = &mut self.bus else {
                return std::future::pending().await;
            };
            let Some(signal) = bus.signals.next().await else {
                self.bus = None;
                return Event::Lost(DISCONNECTED.into());
            };
            // Anything else on this connection is not about our notes.
            match signal.header().member().map(|m| m.as_str()) {
                Some("ActivationToken") => {
                    if let Ok((id, token)) = signal.body().deserialize::<(u32, String)>() {
                        return Event::ActivationToken(id, token);
                    }
                }
                Some("ActionInvoked") => {
                    if let Ok((id, action)) = signal.body().deserialize::<(u32, String)>()
                        && action == "default"
                    {
                        return Event::Click(id);
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((id, _reason)) = signal.body().deserialize::<(u32, u32)>() {
                        return Event::Closed(id);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The window and its worker are gone after a failed re-exec. Send one critical,
/// persistent notification before exiting (also for desktop launches).
pub fn restart_failure(message: &str) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let conn = Connection::session().await?;
            let proxy = Proxy::new(&conn, DEST, PATH, DEST).await?;
            let hints = HashMap::from([
                ("desktop-entry", Variant::from("dev.shuv.rex-rune")),
                ("urgency", Variant::from(2u8)),
            ]);
            let _: u32 = proxy
                .call(
                    "Notify",
                    &(
                        "Rex",
                        0u32,
                        "dev.shuv.rex-rune",
                        "Rex restart failed",
                        message,
                        Vec::<String>::new(),
                        hints,
                        0i32,
                    ),
                )
                .await?;
            Ok::<_, zbus::Error>(())
        })
        .await??;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_missing_service_errors_are_not_found() {
        for error in [
            zbus::fdo::Error::ServiceUnknown("not installed".into()),
            zbus::fdo::Error::NameHasNoOwner("not running".into()),
        ] {
            assert_eq!(failure(error.into()), Service::NotFound);
        }
    }

    #[test]
    fn only_an_error_reply_from_the_service_counts_as_its_answer() {
        // The service replied with an error: it is there, and its reason is not shown.
        let refused: zbus::Error = zbus::fdo::Error::Failed("private daemon details".into()).into();
        assert!(answered(&refused));
        assert_eq!(Service::Rejected.reason(), "Service rejected the request");
        // Nothing answered, or the bus answered for a service that is not there.
        for error in [
            zbus::fdo::Error::NoReply("slow".into()).into(),
            zbus::fdo::Error::TimedOut("slow".into()).into(),
            zbus::fdo::Error::ServiceUnknown("gone".into()).into(),
            zbus::fdo::Error::Disconnected("bus".into()).into(),
            zbus::fdo::Error::SpawnChildExited("daemon failed to start".into()).into(),
            zbus::fdo::Error::SpawnExecFailed("daemon missing".into()).into(),
            zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::other("closed"))),
        ] {
            assert!(!answered(&error), "{error:?}");
        }
        // Finding or starting the service never reads as its refusal: a daemon that failed
        // to start is unreachable, and its diagnostics stay out of Settings.
        for error in [
            zbus::fdo::Error::SpawnChildExited("private daemon details".into()),
            zbus::fdo::Error::Failed("private daemon details".into()),
        ] {
            assert_eq!(
                failure(error.into()),
                Service::Unreachable("Could not reach the service".into())
            );
        }
        assert_eq!(
            failure(zbus::fdo::Error::NoReply("slow".into()).into()),
            Service::Unreachable("Service did not respond".into())
        );
    }

    #[test]
    fn the_notification_picture_matches_the_freedesktop_image_data_format() {
        let image = image_data().expect("embedded Rex icon decodes");
        let (width, height, stride, alpha, bits, channels, pixels): (
            i32,
            i32,
            i32,
            bool,
            i32,
            i32,
            Vec<u8>,
        ) = image.try_into().expect("image-data is (iiibiiay)");
        assert_eq!((width, height, stride), (128, 128, 128 * 4));
        assert_eq!((alpha, bits, channels), (true, 8, 4));
        assert_eq!(pixels.len(), (stride * height) as usize);
        assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
    }
}
