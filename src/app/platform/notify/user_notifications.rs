//! The notification centre through `UNUserNotificationCenter`, on the worker thread.
//!
//! macOS posts notifications only for a process with a main bundle identifier and a code
//! signature, and only after LaunchServices knows the bundle. Without an identifier
//! `currentNotificationCenter` raises an Objective-C exception that ends the process, so
//! the bundle is checked before the framework is touched. Without a signature or
//! registration, requests are refused with `UNErrorDomain` 1. `scripts/macos-app.py`
//! builds and ad-hoc signs the bundle a source build needs.
use crate::app::notify::{Backend, Event, Kind, Note, Service, Session};
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSettings,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use rex_rune::connection::activity::ProgramState;
use std::ptr::NonNull;
use std::time::Duration;

/// There is one Notification Center, so its ids belong to one owner for the app's life.
const OWNER: &str = "macos-notification-centre";
/// Request identifiers. Posting with one that is live replaces that notification, and
/// removing one removes what it holds.
const IDENTIFIER: &str = "rex-";

const NOT_BUNDLED: &str = "Start Rex Rune.app to allow notifications";
const DENIED: &str = "Turned off in System Settings";
const WAITING: &str = "Waiting for permission from macOS";
const REFUSED: &str = "macOS did not allow notifications for this app";
const NO_ANSWER: &str = "Notification centre did not respond";

/// Why this process cannot post notifications at all, if it cannot. A bare executable has
/// no main bundle, and touching the notification centre from one would abort the process.
fn bundle_problem(identifier: Option<&str>) -> Option<Service> {
    identifier
        .is_none()
        .then(|| Service::Unavailable(NOT_BUNDLED.into()))
}

/// What one notification-settings answer means. Only the status is used: Rex decides which
/// tabs notify, and it has sounds of its own, so the request asks for alerts alone.
fn allowed(status: UNAuthorizationStatus) -> Result<(), Service> {
    if status == UNAuthorizationStatus::Authorized
        || status == UNAuthorizationStatus::Provisional
        || status == UNAuthorizationStatus::Ephemeral
    {
        Ok(())
    } else if status == UNAuthorizationStatus::Denied {
        Err(Service::Unavailable(DENIED.into()))
    } else {
        Err(Service::Unavailable(WAITING.into()))
    }
}

/// The request identifier for a ledger id; the same id always replaces the same note.
fn request_identifier(id: u32) -> String {
    format!("{IDENTIFIER}{id}")
}

/// The ledger id a clicked notification came from, if it is one of ours.
fn note_id(identifier: &str) -> Option<u32> {
    identifier
        .strip_prefix(IDENTIFIER)
        .and_then(|rest| rest.parse().ok())
        .filter(|id| *id != 0)
}

struct Ivars {
    events: flume::Sender<Event>,
}

declare_class!(
    struct Delegate;

    // SAFETY: NSObject has no subclassing requirements, and the delegate is only reached
    // through the sender it holds, which is itself thread-safe.
    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "RexNotificationDelegate";
    }

    impl DeclaredClass for Delegate {
        type Ivars = Ivars;
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // The client's own eligibility rules already decided this tab may notify.
            // While Rex is frontmost macOS asks whether to show it: a banner still should.
            let options = UNNotificationPresentationOptions::UNNotificationPresentationOptionBanner
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionList;
            completion.call((options,));
        }

        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &Block<dyn Fn()>,
        ) {
            // A click follows the same path as the freedesktop backend: one navigation,
            // then the tab being seen closes what is left. Any other response (a dismissal)
            // only means the notification is no longer ours to close.
            let action = unsafe { response.actionIdentifier() };
            let default = unsafe { UNNotificationDefaultActionIdentifier };
            let identifier = unsafe { response.notification().request().identifier() };
            if let Some(id) = note_id(&identifier.to_string()) {
                let event = if &*action == default {
                    Event::Click(id)
                } else {
                    Event::Closed(id)
                };
                let _ = self.ivars().events.send(event);
            }
            completion.call(());
        }
    }
);

impl Delegate {
    fn new(events: flume::Sender<Event>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { events });
        unsafe { msg_send_id![super(this), init] }
    }
}

pub struct Desktop {
    /// Why nothing can be posted, from the bundle check at startup.
    problem: Option<Service>,
    center: Option<Retained<UNUserNotificationCenter>>,
    /// The notification centre's delegate is weak; this keeps ours alive.
    #[allow(dead_code)]
    delegate: Option<Retained<Delegate>>,
    events: flume::Receiver<Event>,
    next_id: u32,
}

impl Desktop {
    pub fn new() -> Self {
        // The bundle check comes first: `currentNotificationCenter` raises for a process
        // without one, so a bare executable must never reach it.
        let identifier = unsafe { NSBundle::mainBundle().bundleIdentifier() };
        let identifier = identifier.as_ref().map(|id| id.to_string());
        let problem = bundle_problem(identifier.as_deref());
        let (events, receiver) = flume::unbounded();
        let mut desktop = Self {
            problem,
            center: None,
            delegate: None,
            events: receiver,
            next_id: 0,
        };
        if desktop.problem.is_none() {
            desktop.install(events);
        }
        desktop
    }

    /// Called only after the bundle check: from a bare executable the notification centre
    /// raises before this returns.
    fn install(&mut self, events: flume::Sender<Event>) {
        let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
        let delegate = Delegate::new(events);
        unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        self.center = Some(center);
        self.delegate = Some(delegate);
    }

    fn center(&self) -> &UNUserNotificationCenter {
        self.center
            .as_ref()
            .expect("the bundle was checked before the notification centre")
    }

    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    /// The system's answer for this app, bounded like every other desktop call.
    async fn settings(&self) -> Result<UNAuthorizationStatus, Service> {
        let (tx, rx) = flume::bounded(1);
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            let status = unsafe { settings.as_ref().authorizationStatus() };
            let _ = tx.send(status);
        });
        unsafe {
            self.center()
                .getNotificationSettingsWithCompletionHandler(&block)
        };
        receive(rx).await
    }

    /// Ask, alert only: badges and sounds are not Rex's (it has sounds of its own). A
    /// person answers the system prompt, so this can outlast the bound.
    async fn ask(&self) -> Result<(), Service> {
        let (tx, rx) = flume::bounded(1);
        let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let _ = tx.send((granted.as_bool(), !error.is_null()));
        });
        unsafe {
            self.center()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::UNAuthorizationOptionAlert,
                    &block,
                );
        }
        match tokio::time::timeout(Duration::from_secs(3), rx.recv_async()).await {
            Ok(Ok((true, _))) => Ok(()),
            // The request itself failed (an unsigned or unregistered copy of the app).
            Ok(Ok((false, true))) => Err(Service::Unavailable(REFUSED.into())),
            Ok(Ok((false, false))) => Err(Service::Unavailable(DENIED.into())),
            // A person has not answered the system prompt yet: a state to report, not a
            // failed call. The next command reads the answer.
            Ok(Err(_)) | Err(_) => Err(Service::Unavailable(WAITING.into())),
        }
    }
}

/// Wait for a completion handler, with the same three-second bound as every desktop call.
async fn receive<T>(rx: flume::Receiver<T>) -> Result<T, Service> {
    tokio::time::timeout(Duration::from_secs(3), rx.recv_async())
        .await
        .map_err(|_| Service::Unreachable(NO_ANSWER.into()))?
        .map_err(|_| Service::Unreachable(NO_ANSWER.into()))
}

impl Backend for Desktop {
    async fn ready(&mut self) -> Result<String, Service> {
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        let status = self.settings().await?;
        match allowed(status) {
            Ok(()) => Ok(OWNER.into()),
            Err(_) if status == UNAuthorizationStatus::NotDetermined => {
                // First need, including Send test notification.
                self.ask().await?;
                Ok(OWNER.into())
            }
            Err(problem) => Err(problem),
        }
    }

    async fn send(&mut self, note: &Note, replaces: u32) -> Result<u32, Service> {
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        let id = if replaces == 0 {
            self.next_id()
        } else {
            replaces
        };
        let content = unsafe { UNMutableNotificationContent::new() };
        unsafe {
            content.setTitle(&NSString::from_str(&note.summary));
            content.setBody(&NSString::from_str(&note.body));
        }
        let identifier = NSString::from_str(&request_identifier(id));
        let request = unsafe {
            UNNotificationRequest::requestWithIdentifier_content_trigger(
                &identifier,
                &content,
                None,
            )
        };
        let (tx, rx) = flume::bounded(1);
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(!error.is_null());
        });
        unsafe {
            self.center()
                .addNotificationRequest_withCompletionHandler(&request, Some(&block));
        }
        // Accepted, not displayed: macOS can accept a note while the app is denied.
        match receive(rx).await? {
            false => Ok(id),
            true => Err(Service::Rejected),
        }
    }

    async fn close(&mut self, id: u32) -> Result<(), Service> {
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        let identifier = NSString::from_str(&request_identifier(id));
        let identifiers = NSArray::from_vec(vec![identifier.clone()]);
        unsafe {
            self.center()
                .removeDeliveredNotificationsWithIdentifiers(&identifiers);
        }
        Ok(())
    }
}

impl Session for Desktop {
    fn identity(&self) -> Option<String> {
        self.problem.is_none().then(|| OWNER.to_string())
    }

    async fn event(&mut self) -> Event {
        match self.events.recv_async().await {
            Ok(event) => event,
            // The delegate holds the sender for as long as this worker exists.
            Err(_) => std::future::pending().await,
        }
    }
}

/// The window and its worker are gone after a failed re-exec. Post one notification before
/// exiting, without asking for permission: there is no window left to explain a prompt.
pub fn restart_failure(message: &str) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut desktop = Desktop::new();
        if let Some(problem) = &desktop.problem {
            anyhow::bail!("{}", problem.reason());
        }
        let status = match desktop.settings().await {
            Ok(status) => status,
            Err(problem) => anyhow::bail!("{}", problem.reason()),
        };
        if let Err(problem) = allowed(status) {
            anyhow::bail!("{}", problem.reason());
        }
        let note = Note {
            summary: "Rex restart failed".into(),
            body: message.into(),
            target: None,
            kind: Kind::Activity(ProgramState::Error),
        };
        desktop
            .send(&note, 0)
            .await
            .map_err(|error| anyhow::anyhow!("{}", error.reason()))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_identifier_round_trips_through_a_click() {
        for id in [1, 2, 64, u32::MAX] {
            assert_eq!(note_id(&request_identifier(id)), Some(id));
        }
    }

    #[test]
    fn only_rex_identifiers_are_ours() {
        for other in [
            "",
            "rex-",
            "rex-0",
            "rex-one",
            "notifications.rex-1",
            "Rex-1",
            "1",
        ] {
            assert_eq!(note_id(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_bare_executable_says_to_start_the_app_bundle() {
        assert_eq!(
            bundle_problem(None),
            Some(Service::Unavailable(NOT_BUNDLED.into()))
        );
        assert_eq!(bundle_problem(Some("dev.shuv.rex-rune")), None);
        assert_eq!(
            bundle_problem(None).expect("no bundle").reason(),
            "Start Rex Rune.app to allow notifications"
        );
    }

    #[test]
    fn every_authorization_status_has_its_own_reason() {
        assert_eq!(allowed(UNAuthorizationStatus::Authorized), Ok(()));
        assert_eq!(allowed(UNAuthorizationStatus::Provisional), Ok(()));
        assert_eq!(
            allowed(UNAuthorizationStatus::Denied),
            Err(Service::Unavailable(DENIED.into()))
        );
        assert_eq!(
            allowed(UNAuthorizationStatus::NotDetermined),
            Err(Service::Unavailable(WAITING.into()))
        );
        assert_eq!(Service::Unavailable(DENIED.into()).reason(), DENIED);
        assert_eq!(Service::Unavailable(WAITING.into()).reason(), WAITING);
    }
}
