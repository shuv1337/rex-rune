use super::{Effect, Event, State, step};

fn effects(state: &mut State, event: Event) -> Vec<Effect> {
    step(state, event).into_iter().collect()
}

#[test]
fn limits_stay_locked_through_unmapped_and_first_frame_paints_until_post_callback_paint() {
    let mut state = State::new(true);
    assert_eq!(state, State::Fixed);
    assert!(state.is_locked());

    // Initial unmapped/configure/missed/covered paints and Frame 1's own paint entry must keep
    // min/max locked so the window maps at a fixed size.
    for _ in 0..3 {
        assert_eq!(effects(&mut state, Event::Paint), vec![]);
        assert_eq!(state, State::Fixed);
        assert!(state.is_locked());
    }

    // Frame 1 finishes in `Host::first_frame_ready` (during `AboutToWait`), arming the Wayland
    // frame callback: limits must STILL remain locked; only a follow-up redraw is requested.
    assert_eq!(effects(&mut state, Event::FirstFrameDrawn), vec![Effect::RequestRedraw]);
    assert_eq!(state, State::Owed);
    assert!(state.is_locked());

    // Duplicate FirstFrameDrawn events before the compositor's callback do not release or
    // request another redraw.
    assert_eq!(effects(&mut state, Event::FirstFrameDrawn), vec![]);
    assert_eq!(state, State::Owed);
    assert!(state.is_locked());

    // Once the compositor applies the mapping commit and answers `wl_callback.done`, winit
    // dispatches `WindowEvent::RedrawRequested` -> `Host::paint`, which releases the limits.
    assert_eq!(effects(&mut state, Event::Paint), vec![Effect::Release]);
    assert_eq!(state, State::Done);
    assert!(!state.is_locked());

    // Subsequent paints and notifications are strictly no-ops (limits are released once).
    assert_eq!(effects(&mut state, Event::Paint), vec![]);
    assert_eq!(effects(&mut state, Event::FirstFrameDrawn), vec![]);
    assert_eq!(state, State::Done);
}

#[test]
fn disabled_float_on_open_starts_done_and_emits_no_effects() {
    let mut state = State::new(false);
    assert_eq!(state, State::Done);
    assert_eq!(State::default(), State::Done);
    assert!(!state.is_locked());

    assert_eq!(effects(&mut state, Event::Paint), vec![]);
    assert_eq!(effects(&mut state, Event::FirstFrameDrawn), vec![]);
    assert_eq!(effects(&mut state, Event::Paint), vec![]);
    assert_eq!(state, State::Done);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fixed_locks_min_and_max_to_opening_size() {
    use winit::dpi::{LogicalSize, PhysicalSize, Size};
    use winit::window::Window;

    use super::fixed;
    use crate::windows::WindowOptions;

    // Fallback to logical opening size when neither attrs.inner_size nor physical_size is set.
    let opts = WindowOptions::default();
    let attrs = fixed(Window::default_attributes(), &opts, (900.0, 640.0));
    let expected: Size = LogicalSize::new(900.0_f32, 640.0_f32).into();
    assert_eq!(attrs.min_inner_size, Some(expected));
    assert_eq!(attrs.max_inner_size, Some(expected));

    // Preserves an explicit attrs.inner_size (for example restored bounds on reopen).
    let restored: Size = LogicalSize::new(720.0_f32, 480.0_f32).into();
    let attrs = fixed(Window::default_attributes().with_inner_size(restored), &opts, (900.0, 640.0));
    assert_eq!(attrs.min_inner_size, Some(restored));
    assert_eq!(attrs.max_inner_size, Some(restored));

    // Falls back to options.physical_size when attrs.inner_size is absent.
    let mut phys_opts = WindowOptions::default();
    phys_opts.physical_size = Some((1200, 800));
    let attrs = fixed(Window::default_attributes(), &phys_opts, (900.0, 640.0));
    let expected_phys: Size = PhysicalSize::new(1200_u32, 800_u32).into();
    assert_eq!(attrs.min_inner_size, Some(expected_phys));
    assert_eq!(attrs.max_inner_size, Some(expected_phys));
}

#[test]
fn host_wires_float_on_open_release_through_paint_not_first_frame_ready() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let stepping = std::fs::read_to_string(manifest.join("src/window/host/stepping.rs")).unwrap();
    let events = std::fs::read_to_string(manifest.join("src/window/host/events.rs")).unwrap();
    let create = std::fs::read_to_string(manifest.join("src/window/host/create.rs")).unwrap();
    let cpu_frame = std::fs::read_to_string(manifest.join("src/window/linux/cpu_frame.rs")).unwrap();

    assert!(
        !stepping.contains("float_on_open::release("),
        "Host::first_frame_ready must not release float_on_open size limits during AboutToWait before the compositor frame callback",
    );
    assert!(
        stepping.contains("float_on_open::Event::FirstFrameDrawn"),
        "Host::first_frame_ready must step float_on_open with Event::FirstFrameDrawn",
    );
    assert!(
        events.contains("float_on_open::Event::Paint") && events.contains("float_on_open::release("),
        "Host::paint must step float_on_open with Event::Paint and release size limits on Effect::Release",
    );
    assert!(
        create.contains("float_on_open: float_on_open::State::new(self.float_on_open)"),
        "GPU window creation must initialize Linux Native::float_on_open from self.float_on_open",
    );
    assert!(
        cpu_frame.contains("float_on_open: crate::window::chrome::float_on_open::State::new(self.float_on_open)"),
        "CPU-first window creation must initialize Linux Native::float_on_open from self.float_on_open",
    );
}
