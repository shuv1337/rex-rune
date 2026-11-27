//! Colour operations on opaque, packed sRGB (`0xRRGGBB`).
//! OKLab transforms adapted from Reeper. Alpha compositing uses encoded sRGB,
//! matching the window renderer's `srgb_blend(true)` setting.

/// Decode one sRGB gamma channel (`0.0..=1.0`) to linear light.
pub fn srgb_decode(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Encode one linear light channel (`0.0..=1.0`) to sRGB gamma.
pub fn srgb_encode(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear sRGB `[r, g, b]` to OKLab `[L, a, b]` (Bjorn Ottosson's matrices).
pub fn linear_to_oklab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab `[L, a, b]` to unclamped linear sRGB `[r, g, b]`.
pub fn oklab_to_linear([ll, a, b]: [f32; 3]) -> [f32; 3] {
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn channels(color: u32) -> [f32; 3] {
    [16, 8, 0].map(|s| ((color >> s) & 255) as f32 / 255.)
}
fn pack(channels: [f32; 3]) -> u32 {
    channels.into_iter().fold(0, |rgb, c| {
        rgb << 8 | (255. * c.clamp(0., 1.)).round() as u32
    })
}
pub fn luminance(color: u32) -> f32 {
    let [r, g, b] = channels(color).map(srgb_decode);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}
pub fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// Above middle sRGB grey in linear luminance.
pub fn is_light(color: u32) -> bool {
    luminance(color) > srgb_decode(0.5)
}
/// Perceptual interpolation, gamut-clamped at the output.
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let lab = |c| {
        let [r, g, b] = channels(c).map(srgb_decode);
        linear_to_oklab(r, g, b)
    };
    let (a, b) = (lab(a), lab(b));
    pack(
        oklab_to_linear(std::array::from_fn(|i| {
            a[i] + (b[i] - a[i]) * t.clamp(0., 1.)
        }))
        .map(srgb_encode),
    )
}
pub fn on(fill: u32) -> u32 {
    if contrast(0xffffff, fill) >= contrast(0, fill) {
        0xffffff
    } else {
        0
    }
}
pub fn readable(ink: u32, bg: u32, toward: u32, floor: f32) -> u32 {
    (0..=32)
        .map(|i| blend(ink, toward, i as f32 / 32.))
        .find(|&c| contrast(c, bg) >= floor)
        .unwrap_or(toward)
}
pub fn soften(ink: u32, surfaces: &[u32], t: f32, floor: f32) -> u32 {
    let Some(&bg) = surfaces.first() else {
        return ink;
    };
    (0..=32)
        .rev()
        .map(|i| blend(ink, bg, t * i as f32 / 32.))
        .find(|&c| surfaces.iter().all(|&s| contrast(c, s) >= floor))
        .unwrap_or(ink)
}
/// Straight-alpha foreground over opaque background, rounded to display bytes.
pub fn over(fg: u32, bg: u32, alpha: f32) -> u32 {
    let (fg, bg, t) = (channels(fg), channels(bg), alpha.clamp(0., 1.));
    pack(std::array::from_fn(|i| fg[i] * t + bg[i] * (1. - t)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gamma_compositing_and_wcag_reference_points() {
        assert_eq!(over(0xffffff, 0, 0.5), 0x808080);
        assert!((contrast(0, 0xffffff) - 21.).abs() < 1e-5);
        assert!((luminance(0xff0000) - 0.2126).abs() < 0.00001);
        for byte in 0..=255 {
            let c = byte as f32 / 255.;
            assert!((srgb_encode(srgb_decode(c)) - c).abs() < 0.00001);
        }
    }
    #[test]
    fn perceptual_interpolation_and_text_floors() {
        assert_eq!(blend(0, 0xffffff, 0.5), 0x636363);
        for c in [0, 0xffffff, 0x7c83ff, 0x161d28, 0xff0000] {
            assert_eq!(blend(c, c, 0.5), c);
            assert!(contrast(on(c), c) >= 4.5);
        }
        let text = readable(0xaaaaaa, 0xffffff, 0, 4.5);
        assert!(contrast(text, 0xffffff) >= 4.5);
        let quiet = soften(0xffffff, &[0, 0x333333], 0.8, 4.5);
        assert!(quiet != 0xffffff && contrast(quiet, 0x333333) >= 4.5);
    }
}
