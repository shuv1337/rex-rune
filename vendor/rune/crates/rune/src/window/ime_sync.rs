/// What was last sent to the native input method. On Wayland, submitting a cursor area
/// commits `zwp_text_input_v3`, whose `done` reply wakes the window for another frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum ImeSync {
    #[default]
    Disabled,
    Enabled {
        sent: Option<([f32; 4], f32)>,
    },
}

/// Native window IME calls needed for one sync.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ImeStep {
    pub(super) cancel: bool,
    pub(super) set_allowed: Option<bool>,
    pub(super) cursor_area: Option<[f32; 4]>,
}

impl ImeSync {
    pub(super) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Forget state after the host disables IME directly.
    pub(super) fn clear(&mut self) {
        *self = Self::Disabled;
    }

    /// Resend the area without toggling allowance when a native text input enters again.
    pub(super) fn invalidate_area(&mut self) {
        if let Self::Enabled { sent } = self {
            *sent = None;
        }
    }

    pub(super) fn step(
        &mut self,
        reset: bool,
        area: Option<[f32; 4]>,
        secure_focus: bool,
        focused: bool,
        scale: f32,
    ) -> ImeStep {
        let allowed = area.is_some() && !secure_focus && focused;
        let set_allowed = if reset || allowed != self.is_enabled() {
            *self = if allowed {
                Self::Enabled { sent: None }
            } else {
                Self::Disabled
            };
            Some(allowed)
        } else {
            None
        };
        let cursor_area = match (self, area) {
            (Self::Enabled { sent }, Some(a)) if *sent != Some((a, scale)) => {
                *sent = Some((a, scale));
                Some(a)
            }
            _ => None,
        };
        ImeStep {
            cancel: reset,
            set_allowed,
            cursor_area,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::platform_sync::ime_sync_step;
    use super::*;
    use crate::{Cx, El, Env, Ime, Runtime, el};

    const AREA: [f32; 4] = [16.0, 20.0, 2.0, 18.0];
    const MOVED: [f32; 4] = [40.0, 20.0, 2.0, 18.0];
    const IDLE: ImeStep = ImeStep {
        cancel: false,
        set_allowed: None,
        cursor_area: None,
    };
    const ENABLE: ImeStep = ImeStep {
        set_allowed: Some(true),
        cursor_area: Some(AREA),
        ..IDLE
    };
    const DISABLE: ImeStep = ImeStep {
        set_allowed: Some(false),
        ..IDLE
    };
    const RESEND: ImeStep = ImeStep {
        cursor_area: Some(AREA),
        ..IDLE
    };

    #[test]
    fn unchanged_area_and_scale_do_not_resend_but_either_change_does() {
        let mut ime = ImeSync::default();
        assert_eq!(ime.step(false, None, false, true, 1.0), IDLE);
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), IDLE);
        let moved = ImeStep {
            cursor_area: Some(MOVED),
            ..IDLE
        };
        assert_eq!(ime.step(false, Some(MOVED), false, true, 1.0), moved);
        assert_eq!(ime.step(false, Some(MOVED), false, true, 1.0), IDLE);
        assert_eq!(ime.step(false, Some(MOVED), false, true, 2.0), moved);
        assert_eq!(ime.step(false, Some(MOVED), false, true, 2.0), IDLE);
    }

    #[test]
    fn reset_cancels_and_reenables_ime_with_a_fresh_area() {
        let mut ime = ImeSync::default();
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
        assert_eq!(
            ime.step(true, Some(AREA), false, true, 1.0),
            ImeStep {
                cancel: true,
                ..ENABLE
            }
        );
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), IDLE);
        assert_eq!(
            ime.step(true, None, false, true, 1.0),
            ImeStep {
                cancel: true,
                ..DISABLE
            }
        );
        assert!(!ime.is_enabled());
    }

    #[test]
    fn focus_loss_hide_secure_focus_and_missing_area_forget_the_last_area() {
        let mut ime = ImeSync::default();
        for (area, secure, focused) in [
            (Some(AREA), false, false),
            (Some(AREA), true, true),
            (None, false, true),
        ] {
            assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
            assert_eq!(ime.step(false, area, secure, focused, 1.0), DISABLE);
            assert_eq!(ime.step(false, area, secure, focused, 1.0), IDLE);
        }
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
        ime.clear();
        assert!(!ime.is_enabled());
        assert_eq!(ime.step(false, Some(AREA), false, false, 1.0), IDLE);
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
    }

    #[test]
    fn platform_ime_reentry_resends_area_without_toggling_allowance() {
        let mut ime = ImeSync::default();
        ime.invalidate_area();
        assert_eq!(ime.step(false, None, false, true, 1.0), IDLE);
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), ENABLE);
        ime.invalidate_area();
        assert!(ime.is_enabled());
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), RESEND);
        assert_eq!(ime.step(false, Some(AREA), false, true, 1.0), IDLE);
    }

    #[test]
    fn app_ime_cursor_rests_after_an_empty_preedit_and_resends_when_moved() {
        fn view(x: &f32, _: &mut Cx<f32>) -> El {
            el().size(200.0, 100.0)
                .child(el().absolute(*x, 20.0).size(2.0, 18.0).ime_cursor())
        }
        let mut rt = Runtime::new(16.0, view, (200.0, 100.0), Env::default());
        let mut ime = ImeSync::default();
        let mut sync = |rt: &mut Runtime<f32>| ime_sync_step(rt, &mut ime, true, 1.0);
        rt.frame(0.0);
        let first = sync(&mut rt);
        assert_eq!(first.set_allowed, Some(true));
        assert_eq!(first.cursor_area, Some(AREA));

        rt.ime(Ime::Preedit(String::new(), None));
        rt.frame(0.0);
        assert_eq!(sync(&mut rt), IDLE);

        rt.update(|x, tx| {
            *x = 40.0;
            tx.instant();
        });
        rt.frame(0.0);
        assert_eq!(
            sync(&mut rt),
            ImeStep {
                cursor_area: Some(MOVED),
                ..IDLE
            }
        );
        rt.frame(0.0);
        assert_eq!(sync(&mut rt), IDLE);
    }
}
