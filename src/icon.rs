//! Clatter's keycap icon, rendered in code (no image assets).
//! Pure std so `build.rs` can reuse it to produce the .exe icon.

#[derive(Clone, Copy, PartialEq)]
pub enum IconStyle {
    /// Full-colour app icon.
    Color,
    /// Monochrome tray glyph. `dark` = black glyph for light taskbars.
    Tray { dark: bool, active: bool },
}

/// Signed distance from `p` to a rounded rectangle (centre `c`, half-size `h`, radius `r`).
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Composites `src` (straight alpha) over `dst` (straight alpha).
fn over(dst: [f32; 4], src: [f32; 3], a: f32) -> [f32; 4] {
    let out_a = a + dst[3] * (1.0 - a);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let mix = |s: f32, d: f32| (s * a + d * dst[3] * (1.0 - a)) / out_a;
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), out_a]
}

/// Renders the icon as straight-alpha RGBA8, `size` x `size`.
pub fn render(size: u32, style: IconStyle) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // Sample at pixel centre, in pixel units.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = [0.0f32; 4];
            match style {
                IconStyle::Color => {
                    // Keycap skirt.
                    let d_outer = sd_round_rect(fx, fy, s * 0.5, s * 0.53, s * 0.44, s * 0.41, s * 0.2);
                    let t = fy / s;
                    let skirt = lerp([1.0, 0.56, 0.24], [0.84, 0.29, 0.16], t);
                    c = over(c, skirt, coverage(d_outer));
                    // Top surface (dish).
                    let d_top = sd_round_rect(fx, fy, s * 0.5, s * 0.43, s * 0.31, s * 0.27, s * 0.14);
                    let top = lerp([1.0, 0.95, 0.88], [1.0, 0.83, 0.64], ((fy / s) - 0.16) / 0.54);
                    c = over(c, top, coverage(d_top) * c[3].min(1.0));
                    // Soft highlight along the top edge of the dish.
                    let d_hi = sd_round_rect(fx, fy, s * 0.5, s * 0.25, s * 0.22, s * 0.03, s * 0.03);
                    c = over(c, [1.0, 1.0, 1.0], coverage(d_hi / (s / 64.0).max(1.0)) * 0.55);
                }
                IconStyle::Tray { dark, active } => {
                    let ink = if dark { [0.0, 0.0, 0.0] } else { [1.0, 1.0, 1.0] };
                    let alpha = if active { 1.0 } else { 0.42 };
                    let stroke = (s * 0.085).max(1.2);
                    // Outline of the skirt.
                    let d_outer = sd_round_rect(fx, fy, s * 0.5, s * 0.52, s * 0.45, s * 0.42, s * 0.2);
                    let ring = coverage(d_outer.abs() - stroke * 0.5);
                    // Solid top surface.
                    let d_top = sd_round_rect(fx, fy, s * 0.5, s * 0.42, s * 0.25, s * 0.2, s * 0.1);
                    let a = ring.max(coverage(d_top)) * alpha;
                    c = over(c, ink, a);
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = (c[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            px[i + 1] = (c[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            px[i + 2] = (c[2].clamp(0.0, 1.0) * 255.0).round() as u8;
            px[i + 3] = (c[3].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    px
}
