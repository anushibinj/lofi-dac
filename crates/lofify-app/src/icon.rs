//! The app icon, drawn procedurally so no image assets are needed: a little vinyl record.

/// Renders a `size` x `size` RGBA icon. `active` picks the teal label; otherwise it is grey
/// (used for the bypassed / stopped tray icon).
pub fn rgba(size: u32, active: bool) -> Vec<u8> {
    let s = size.max(8) as f32;
    let centre = (s - 1.0) / 2.0;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let label = if active {
        [38, 198, 178]
    } else {
        [130, 130, 140]
    };
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - centre, y as f32 - centre);
            let r = (dx * dx + dy * dy).sqrt() / (s / 2.0);
            // anti-aliased edge over the last ~1.2 px
            let edge = ((1.0 - r) * (s / 2.0) / 1.2).clamp(0.0, 1.0);
            let (rgb, alpha) = if r > 1.0 {
                ([0, 0, 0], 0.0)
            } else if r < 0.10 {
                ([245, 245, 245], 1.0) // spindle hole highlight
            } else if r < 0.38 {
                (label, 1.0)
            } else {
                // dark vinyl with faint concentric grooves
                let groove = if ((r * 22.0) as u32).is_multiple_of(2) {
                    26.0
                } else {
                    40.0
                };
                ([groove as u8, groove as u8, (groove + 6.0) as u8], 1.0)
            };
            let i = ((y * size + x) * 4) as usize;
            out[i] = rgb[0];
            out[i + 1] = rgb[1];
            out[i + 2] = rgb[2];
            out[i + 3] = (alpha * edge * 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(v: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [v[i], v[i + 1], v[i + 2], v[i + 3]]
    }

    #[test]
    fn has_the_right_size_and_transparent_corners() {
        let v = rgba(64, true);
        assert_eq!(v.len(), 64 * 64 * 4);
        assert_eq!(px(&v, 64, 0, 0)[3], 0);
        assert_eq!(px(&v, 64, 63, 63)[3], 0);
    }

    #[test]
    fn label_colour_depends_on_state() {
        let on = px(&rgba(64, true), 64, 32, 22);
        let off = px(&rgba(64, false), 64, 32, 22);
        assert_eq!(on[3], 255);
        assert!(on[1] > on[0] && on[1] > 150, "teal label: {on:?}");
        assert_eq!(off[0], off[1], "grey label is neutral: {off:?}");
        assert_ne!(on, off);
    }

    #[test]
    fn tiny_sizes_do_not_panic() {
        for s in [1, 2, 8, 16, 32] {
            assert_eq!(rgba(s, true).len(), (s.max(1) * s.max(1) * 4) as usize);
        }
    }
}
