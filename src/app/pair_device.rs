//! Local display of the server's last-reported Tailscale HTTPS address. No requests.
use super::workspace::HostId;
use super::{
    dialogs,
    panel::PanelKind,
    settings_kit,
    theme::*,
    widgets::{Pill, pill},
    workspace::Workspace,
};
use rex_rune::qr;
use rune::{El, col, el, row, text};

pub const ACTION: &str = "client.pair_device";

#[derive(Clone, Debug)]
pub struct Dialog {
    host: HostId,
    epoch: u64,
    pub address: String,
    pub symbol: Result<qr::Symbol, qr::TooLong>,
    // Horizontal dark runs, computed once, rather than one element per dark module.
    runs: Vec<(usize, usize, usize)>,
    label: String,
}

impl Workspace {
    pub fn pair_address(&self) -> Option<String> {
        let host = self.hosts.get(self.active)?;
        if !host.model.connected {
            return None;
        }
        super::server_page::tailscale_address(&host.model)
    }

    pub fn pair_device(&mut self) {
        let Some(address) = self.pair_address() else {
            return;
        };
        let symbol = qr::encode(address.as_bytes());
        let mut runs = Vec::new();
        if let Ok(symbol) = &symbol {
            for y in 0..symbol.size() {
                let mut x = 0;
                while x < symbol.size() {
                    if !symbol.dark(x, y) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < symbol.size() && symbol.dark(x, y) {
                        x += 1;
                    }
                    runs.push((start, y, x - start));
                }
            }
        }
        let dialog = Dialog {
            host: self.hosts[self.active].id,
            epoch: self.hosts[self.active].model.connection_epoch,
            address,
            symbol,
            runs,
            label: self.settings.hosts[self.active].label.clone(),
        };
        self.open_panel(PanelKind::PairDevice(dialog), "Pair device", &[]);
    }
}

pub fn dialog(s: &Workspace, d: &Dialog, viewport: [f32; 2], px: f32) -> El {
    let current = s
        .hosts
        .get(s.active)
        .is_some_and(|h| h.id == d.host && h.model.connection_epoch == d.epoch)
        && s.pair_address().as_deref() == Some(&d.address);
    let mut body = col()
        .w(520f32.min(viewport[0] * 0.9))
        .max_h(viewport[1] - 24.)
        .padding(16.)
        .gap(8.)
        .child(dialogs::title("Pair device"));
    if current {
        if let Ok(symbol) = &d.symbol {
            let modules = (symbol.size() + 8) as f32;
            let budget = if viewport[1] < 600. { 240. } else { 280. };
            let unit = (budget * px / modules).floor().max(1.) / px;
            let size = modules * unit;
            // Rune snaps fills to device pixels. Integral device-pixel modules and merged
            // runs keep the four-module quiet zone intact, without filtered image sampling.
            let mut code = el()
                .key("pair-code")
                .size(size, size)
                .pixel_snap(true)
                .fill(rune::Color::WHITE)
                .role("img")
                .label("QR code of the address below");
            for &(x, y, len) in &d.runs {
                code = code.child(
                    el().absolute((x + 4) as f32 * unit, (y + 4) as f32 * unit)
                        .size(len as f32 * unit, unit)
                        .fill(rune::Color::BLACK),
                );
            }
            body = body.child(row().w_full().justify_center().child(code));
        } else {
            body = body.child(
                text("This address is too long to show as a code.")
                    .font(ui(13.))
                    .color(rgb(warning()))
                    .wrap(),
            );
        }
        body = body.child(col().gap(4.)
            .child(text(d.label.clone()).font(strong(13.)).color(rgb(ink())).truncate().help_when_truncated(d.label.clone()))
            .child(settings_kit::value(d.address.clone()).key("pair-address").w_full().wrap().wrap_anywhere()
                .max_h(80.).scroll(rune::ScrollSpec { stick_bottom:false })))
            .child(text("Scan with Rex on the other device. It must be able to reach this last-reported address, for example on the same Tailscale network.")
                .font(ui(12.)).color(rgb(muted())).wrap());
    } else {
        // Host switches, disconnects and changed reports immediately hide the captured code.
        // Do not silently substitute a different host's code in an already open dialog.
        body = body.child(text("Pairing is no longer available for this host. Close this dialog and check Server settings.")
            .font(ui(13.)).color(rgb(muted())).wrap());
    }
    body = body.child(
        row().justify_end().child(
            pill("pair-done", "Done", Pill::Primary, 32., |s, tx| {
                s.cancel_panel(tx)
            })
            .default_button(),
        ),
    );
    dialogs::modal_frame(s, body)
}
