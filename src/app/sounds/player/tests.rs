use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Tree(Vec<PathBuf>);
impl Files for Tree {
    fn executable(&self, p: &Path) -> bool {
        self.file(p)
    }
    fn file(&self, p: &Path) -> bool {
        self.0.iter().any(|f| f == p)
    }
}

#[derive(Default)]
pub(crate) struct ChildState {
    pub killed: bool,
    pub reaped: bool,
    pub polls: usize,
}
pub(crate) struct FakeChild {
    state: Arc<Mutex<ChildState>>,
    result: Option<bool>,
}
impl Running for FakeChild {
    fn poll(&mut self) -> io::Result<Option<bool>> {
        self.state.lock().unwrap().polls += 1;
        Ok(self.result)
    }
    fn kill(&mut self) {
        self.state.lock().unwrap().killed = true;
    }
    fn reap(&mut self) {
        self.state.lock().unwrap().reaped = true;
    }
}

pub(crate) struct FakePlayer {
    pub lookups: Vec<Sound>,
    pub invocations: Vec<Invocation>,
    pub clock: Duration,
    pub missing: bool,
    pub spawn_failure: bool,
    pub result: Option<bool>,
    pub child: Arc<Mutex<ChildState>>,
}
impl Default for FakePlayer {
    fn default() -> Self {
        Self {
            lookups: vec![],
            invocations: vec![],
            clock: Duration::ZERO,
            missing: false,
            spawn_failure: false,
            result: Some(true),
            child: Default::default(),
        }
    }
}
impl Player for FakePlayer {
    type Child = FakeChild;
    fn find(&mut self, sound: Sound) -> Option<Invocation> {
        self.lookups.push(sound);
        if self.missing {
            return None;
        }
        select(
            &Tree(vec!["/private/canberra-gtk-play".into()]),
            &["/private".into()],
            &[],
            sound,
        )
    }
    fn spawn(&mut self, invocation: &Invocation) -> io::Result<FakeChild> {
        self.invocations.push(Invocation {
            program: invocation.program.clone(),
            args: invocation.args.clone(),
        });
        if self.spawn_failure {
            return Err(io::Error::other("fake spawn failure"));
        }
        self.child = Default::default();
        Ok(FakeChild {
            state: self.child.clone(),
            result: self.result,
        })
    }
    fn now(&self) -> Duration {
        self.clock
    }
    fn pause(&mut self) {
        self.clock += Duration::from_millis(20);
    }
}

#[test]
fn selection_order_exact_vectors_and_xdg_file_search() {
    let paths = vec!["/one".into(), "/two".into()];
    let data = vec![
        "/home/person/.local/share".into(),
        "/custom".into(),
        "/usr/share".into(),
    ];
    for sound in [Sound::NeedsYou, Sound::Error, Sound::Done] {
        // Arguments are OsStrings, so preserve the native separators from each join,
        // not just paths that compare equal as PathBufs on Windows.
        let file = Path::new("/custom")
            .join("sounds/freedesktop/stereo")
            .join(format!("{}.oga", sound.id()));
        let mut tree = Tree(vec![
            "/two/canberra-gtk-play".into(),
            "/one/pw-play".into(),
            "/one/paplay".into(),
            file.clone(),
        ]);
        assert_eq!(
            select(&tree, &paths, &data, sound),
            Some(Invocation {
                program: "/two/canberra-gtk-play".into(),
                args: vec!["-i".into(), sound.id().into()]
            })
        );
        tree.0.remove(0);
        assert_eq!(
            select(&tree, &paths, &data, sound),
            Some(Invocation {
                program: "/one/pw-play".into(),
                args: vec![file.clone().into_os_string()]
            })
        );
        tree.0.remove(0);
        assert_eq!(
            select(&tree, &paths, &data, sound),
            Some(Invocation {
                program: "/one/paplay".into(),
                args: vec![file.into_os_string()]
            })
        );
        tree.0.pop();
        assert!(
            select(&tree, &paths, &data, sound).is_none(),
            "no theme file"
        );
        assert!(
            select(&Tree::default(), &paths, &data, sound).is_none(),
            "no player"
        );
    }
}

#[test]
fn xdg_defaults_overrides_and_first_matching_file() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let mine = root.path().join("mine");
    let first = root.path().join("first");
    let second = root.path().join("second");
    // The home default works for native absolute paths. Unix system defaults have no
    // drive prefix on Windows and are correctly excluded there.
    let defaults: Vec<_> = [
        home.join(".local/share"),
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ]
    .into_iter()
    .filter(|path| path.is_absolute())
    .collect();
    assert_eq!(data_dirs(Some(home.clone()), None, None), defaults);
    let data = data_dirs(
        Some(home),
        Some(mine.clone()),
        Some(std::env::join_paths([Path::new("relative"), &first, &second]).unwrap()),
    );
    assert_eq!(data, vec![mine, first.clone(), second.clone()]);
    let file = |dir: &Path| dir.join("sounds/freedesktop/stereo").join("complete.oga");
    let tree = Tree(vec!["/bin/paplay".into(), file(&first), file(&second)]);
    assert_eq!(
        select(&tree, &["/bin".into()], &data, Sound::Done)
            .unwrap()
            .args,
        vec![file(&first).into_os_string()]
    );
}

#[test]
fn a_relative_path_entry_is_never_searched_for_a_player() {
    // `.`, an empty entry and `bin` all mean "wherever the app was started".
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    let tools = root.path().join("tools");
    let path = std::env::join_paths([
        Path::new("."),
        Path::new(""),
        Path::new("bin"),
        &bin,
        Path::new("relative/dir"),
        &tools,
    ])
    .unwrap();
    assert_eq!(search_path(Some(path)), vec![bin, tools]);
    assert!(search_path(None).is_empty());
    assert!(search_path(Some(".".into())).is_empty());
}

#[test]
fn hung_player_is_killed_and_reaped_at_limit_without_real_time() {
    let mut player = FakePlayer {
        result: None,
        ..Default::default()
    };
    assert_eq!(
        attempt(&mut player, Some(Sound::Done), || false),
        Health::TimedOut
    );
    assert_eq!(player.clock, LIMIT);
    let child = player.child.lock().unwrap();
    assert!(child.killed && child.reaped);
    assert_eq!(player.invocations.len(), 1);
}

#[test]
fn shutdown_kills_and_reaps_its_child_and_never_spawns_after_stop() {
    let mut player = FakePlayer {
        result: None,
        ..Default::default()
    };
    let calls = std::cell::Cell::new(0);
    attempt(&mut player, Some(Sound::Done), || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    });
    {
        let child = player.child.lock().unwrap();
        assert!(child.killed && child.reaped);
    }
    assert!(player.clock < LIMIT);
    attempt(&mut player, Some(Sound::Done), || true);
    assert_eq!(player.invocations.len(), 1);
}

#[test]
fn failures_are_health_only_and_checks_never_spawn() {
    let mut player = FakePlayer::default();
    assert_eq!(attempt(&mut player, None, || false), Health::Available);
    assert!(player.invocations.is_empty());
    player.missing = true;
    assert_eq!(
        attempt(&mut player, Some(Sound::Done), || false),
        Health::Missing
    );
    assert!(player.invocations.is_empty());
    player.missing = false;
    player.spawn_failure = true;
    assert_eq!(
        attempt(&mut player, Some(Sound::Done), || false),
        Health::Failed
    );
    player.spawn_failure = false;
    player.result = Some(false);
    assert_eq!(
        attempt(&mut player, Some(Sound::Done), || false),
        Health::Failed
    );
    assert!(player.child.lock().unwrap().reaped);
    assert!(!player.child.lock().unwrap().killed);
}

#[test]
fn a_mac_plays_its_own_alert_sounds_with_afplay_when_no_linux_player_is_there() {
    let paths = ["/usr/bin".into()];
    let sounds = [
        (Sound::NeedsYou, "Ping"),
        (Sound::Error, "Basso"),
        (Sound::Done, "Glass"),
    ];
    for (sound, name) in sounds {
        let file = Path::new("/System/Library/Sounds").join(format!("{name}.aiff"));
        let tree = Tree(vec!["/usr/bin/afplay".into(), file.clone()]);
        assert_eq!(
            select(&tree, &paths, &[], sound),
            Some(Invocation {
                program: "/usr/bin/afplay".into(),
                args: vec![file.clone().into_os_string()],
            })
        );
        // The player without the sound, or the sound without the player, is no player.
        assert_eq!(
            select(&Tree(vec!["/usr/bin/afplay".into()]), &paths, &[], sound),
            None
        );
        assert_eq!(select(&Tree(vec![file]), &paths, &[], sound), None);
    }
    // A Linux player Homebrew put on PATH does not take the sound away: the Mac's own
    // player comes first, and a player whose sound file is missing is passed over.
    let tree = Tree(vec![
        "/opt/homebrew/bin/canberra-gtk-play".into(),
        "/opt/homebrew/bin/paplay".into(),
        "/usr/bin/afplay".into(),
        "/System/Library/Sounds/Glass.aiff".into(),
    ]);
    let paths = ["/opt/homebrew/bin".into(), "/usr/bin".into()];
    let chosen = select(&tree, &paths, &[], Sound::Done).unwrap();
    assert_eq!(chosen.program, PathBuf::from("/usr/bin/afplay"));
    let no_sound_file = Tree(vec!["/opt/homebrew/bin/paplay".into()]);
    assert_eq!(select(&no_sound_file, &paths, &[], Sound::Done), None);
    let with_canberra = Tree(vec![
        "/opt/homebrew/bin/paplay".into(),
        "/usr/bin/canberra-gtk-play".into(),
    ]);
    assert_eq!(
        select(&with_canberra, &paths, &[], Sound::Done)
            .unwrap()
            .program,
        PathBuf::from("/usr/bin/canberra-gtk-play"),
        "a later player that can play is used"
    );
}
