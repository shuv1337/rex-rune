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
use std::hash::BuildHasher;
use std::ptr::NonNull;
use std::time::Duration;

/// There is one Notification Center, so its ids belong to one owner for the process's life.
const OWNER: &str = "macos-notification-centre";

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

/// What a permission request's completion handler reports: granted, and whether the request
/// itself failed.
type Answer = (bool, bool);

fn answered((granted, failed): Answer) -> Result<(), Service> {
    match (granted, failed) {
        (true, _) => Ok(()),
        // The request itself failed (an unsigned or unregistered copy of the app).
        (false, true) => Err(Service::Unavailable(REFUSED.into())),
        (false, false) => Err(Service::Unavailable(DENIED.into())),
    }
}

/// Notes left in Notification Center outlive the process, and ledger ids start again at 1,
/// so every request identifier carries this launch's own prefix, `rex-<launch>-`. Posting
/// with a live identifier replaces that note and removing one removes it; a note from an
/// earlier launch matches neither, and its click or dismissal is not ours.
fn launch_prefix() -> String {
    let launch = std::collections::hash_map::RandomState::new()
        .hash_one((std::time::SystemTime::now(), std::process::id()));
    format!("rex-{launch:016x}-")
}

/// The request identifier for a ledger id; the same id always replaces the same note.
fn request_identifier(prefix: &str, id: u32) -> String {
    format!("{prefix}{id}")
}

/// The ledger id a clicked notification came from, if this launch sent it.
fn note_id(prefix: &str, identifier: &str) -> Option<u32> {
    identifier
        .strip_prefix(prefix)
        .and_then(|rest| rest.parse().ok())
        .filter(|id| *id != 0)
}

struct Ivars {
    events: flume::Sender<Event>,
    prefix: String,
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
            if let Some(id) = note_id(&self.ivars().prefix, &identifier.to_string()) {
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
    fn new(events: flume::Sender<Event>, prefix: String) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { events, prefix });
        unsafe { msg_send_id![super(this), init] }
    }
}

pub struct Desktop {
    /// Why nothing can be posted, from the bundle check at startup.
    problem: Option<Service>,
    center: Option<Retained<UNUserNotificationCenter>>,
    /// The notification centre's delegate is weak; this keeps ours alive.
    _delegate: Option<Retained<Delegate>>,
    events: flume::Receiver<Event>,
    /// This launch's request identifier prefix.
    prefix: String,
    /// The answer to a permission prompt that is still up, while a person has not answered.
    asking: Option<flume::Receiver<Answer>>,
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
            _delegate: None,
            events: receiver,
            prefix: launch_prefix(),
            asking: None,
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
        let delegate = Delegate::new(events, self.prefix.clone());
        unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        self.center = Some(center);
        self._delegate = Some(delegate);
    }

    fn owner(&self) -> String {
        format!("{OWNER}:{}", self.prefix)
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
            // Never block the framework's callback thread, even if it calls twice.
            let _ = tx.try_send(status);
        });
        unsafe {
            self.center()
                .getNotificationSettingsWithCompletionHandler(&block)
        };
        receive(rx).await
    }

    /// Ask, alert only: badges and sounds are not Rex's (it has sounds of its own). A
    /// person answers the system prompt, so the answer can outlast the bound; it is then
    /// kept for later commands, and macOS is not asked again while the prompt is up.
    async fn ask(&mut self) -> Result<(), Service> {
        let (tx, rx) = flume::bounded(1);
        let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let _ = tx.try_send((granted.as_bool(), !error.is_null()));
        });
        unsafe {
            self.center()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::UNAuthorizationOptionAlert,
                    &block,
                );
        }
        match tokio::time::timeout(Duration::from_secs(3), rx.recv_async()).await {
            Ok(Ok(answer)) => answered(answer),
            Ok(Err(_)) => Err(Service::Unavailable(WAITING.into())),
            // A person has not answered the system prompt yet: a state to report, not a
            // failed call.
            Err(_) => {
                self.asking = Some(rx);
                Err(Service::Unavailable(WAITING.into()))
            }
        }
    }
}

/// The answer to a prompt left up by an earlier command, without waiting for it. `None`
/// once there is nothing left to wait for, so the caller reads the settings again.
fn pending(asking: &mut Option<flume::Receiver<Answer>>) -> Option<Result<(), Service>> {
    let answer = match asking.as_ref()?.try_recv() {
        Err(flume::TryRecvError::Empty) => return Some(Err(Service::Unavailable(WAITING.into()))),
        Ok(answer) => Some(answered(answer)),
        Err(flume::TryRecvError::Disconnected) => None,
    };
    *asking = None;
    answer
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
        if let Some(answer) = pending(&mut self.asking) {
            return answer.map(|()| self.owner());
        }
        let status = self.settings().await?;
        match allowed(status) {
            Ok(()) => Ok(self.owner()),
            Err(_) if status == UNAuthorizationStatus::NotDetermined => {
                // First need, including Send test notification.
                self.ask().await?;
                Ok(self.owner())
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
        let identifier = NSString::from_str(&request_identifier(&self.prefix, id));
        let request = unsafe {
            UNNotificationRequest::requestWithIdentifier_content_trigger(
                &identifier,
                &content,
                None,
            )
        };
        let (tx, rx) = flume::bounded(1);
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.try_send(!error.is_null());
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
        let identifier = NSString::from_str(&request_identifier(&self.prefix, id));
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
        self.problem.is_none().then(|| self.owner())
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
        let prefix = launch_prefix();
        for id in [1, 2, 64, u32::MAX] {
            assert_eq!(note_id(&prefix, &request_identifier(&prefix, id)), Some(id));
        }
    }

    #[test]
    fn identifiers_carry_this_launch_and_no_other() {
        let prefix = launch_prefix();
        let identifier = request_identifier(&prefix, 1);
        let launch = identifier
            .strip_prefix("rex-")
            .and_then(|rest| rest.strip_suffix("-1"))
            .expect("rex-<launch>-<id>");
        assert_eq!(launch.len(), 16);
        assert!(launch.chars().all(|c| c.is_ascii_hexdigit()));
        // An earlier launch's note keeps its click and close to itself, though its ledger
        // id is one this launch uses too.
        let earlier = launch_prefix();
        assert_ne!(earlier, prefix);
        assert_eq!(note_id(&prefix, &request_identifier(&earlier, 1)), None);
        let desktop = Desktop {
            problem: None,
            center: None,
            _delegate: None,
            events: flume::unbounded().1,
            prefix: prefix.clone(),
            asking: None,
            next_id: 0,
        };
        assert_eq!(desktop.owner(), format!("{OWNER}:{prefix}"));
        assert_eq!(desktop.identity(), Some(desktop.owner()));
    }

    #[test]
    fn only_rex_identifiers_are_ours() {
        let prefix = launch_prefix();
        for other in [
            String::new(),
            prefix.clone(),
            format!("{prefix}0"),
            format!("{prefix}one"),
            format!("notifications.{prefix}1"),
            "rex-1".into(),
            "Rex-1".into(),
            "1".into(),
        ] {
            assert_eq!(note_id(&prefix, &other), None, "{other:?}");
        }
    }

    #[test]
    fn an_unanswered_prompt_is_waited_on_without_asking_again() {
        let waiting = Err(Service::Unavailable(WAITING.into()));
        let mut asking = None;
        assert_eq!(
            pending(&mut asking),
            None,
            "nothing asked yet: read the settings"
        );
        let (tx, rx) = flume::bounded::<Answer>(1);
        asking = Some(rx);
        // Every command while the prompt is up says so at once and keeps the request.
        for _ in 0..3 {
            assert_eq!(pending(&mut asking), Some(waiting.clone()));
            assert!(asking.is_some());
        }
        tx.try_send((true, false)).expect("one answer");
        assert!(
            tx.try_send((false, false)).is_err(),
            "a second call never blocks"
        );
        assert_eq!(pending(&mut asking), Some(Ok(())));
        assert!(
            asking.is_none(),
            "answered: the next command reads the settings"
        );
        assert_eq!(pending(&mut asking), None);
        // A handler that was dropped unanswered leaves nothing to wait for.
        let (tx, rx) = flume::bounded::<Answer>(1);
        asking = Some(rx);
        drop(tx);
        assert_eq!(pending(&mut asking), None);
        assert!(asking.is_none());
    }

    #[test]
    fn a_prompt_answer_has_its_own_reason() {
        assert_eq!(answered((true, false)), Ok(()));
        assert_eq!(
            answered((false, false)),
            Err(Service::Unavailable(DENIED.into()))
        );
        assert_eq!(
            answered((false, true)),
            Err(Service::Unavailable(REFUSED.into()))
        );
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
