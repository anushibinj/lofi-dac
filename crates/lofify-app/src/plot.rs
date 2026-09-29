//! Data for the live frequency-response plot.

use lofify_dsp::filters::frequency_response;
use lofify_dsp::params::FilterParams;

pub const MIN_HZ: f32 = 20.0;
pub const MAX_HZ: f32 = 20_000.0;
pub const POINTS: usize = 240;

/// `n` frequencies spaced evenly on a log axis from `MIN_HZ` to `MAX_HZ`.
pub fn log_frequencies(n: usize) -> Vec<f32> {
    let n = n.max(2);
    let (lo, hi) = (MIN_HZ.log10(), MAX_HZ.log10());
    (0..n)
        .map(|i| 10f32.powf(lo + (hi - lo) * i as f32 / (n - 1) as f32))
        .collect()
}

/// `[log10(frequency), dB]` points for `egui_plot`, computed analytically from the biquads.
pub fn response_points(filters: &FilterParams, sample_rate: f32) -> Vec<[f64; 2]> {
    let freqs = log_frequencies(POINTS);
    frequency_response(filters, sample_rate, &freqs)
        .into_iter()
        .zip(&freqs)
        .map(|(db, &f)| [f64::from(f.log10()), f64::from(db.clamp(-90.0, 30.0))])
        .collect()
}

/// Label for a `log10(frequency)` axis value: "100", "1k", "10k".
pub fn axis_label(log_hz: f64) -> String {
    let hz = 10f64.powf(log_hz);
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round() as u32)
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{}", hz.round() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofify_dsp::presets::builtin_preset;

    #[test]
    fn frequencies_are_log_spaced_and_cover_the_audible_range() {
        let f = log_frequencies(POINTS);
        assert_eq!(f.len(), POINTS);
        assert!((f[0] - 20.0).abs() < 0.01 && (f[POINTS - 1] - 20_000.0).abs() < 1.0);
        let ratios: Vec<f32> = f.windows(2).map(|w| w[1] / w[0]).collect();
        let first = ratios[0];
        assert!(
            ratios.iter().all(|r| (r - first).abs() < 1.0e-3),
            "constant ratio = log spacing"
        );
    }

    #[test]
    fn classic_lofi_curve_rolls_off_both_ends() {
        let p = builtin_preset("Classic Lofi").unwrap().params;
        let pts = response_points(&p.filters, 48_000.0);
        let at = |hz: f64| {
            pts.iter()
                .min_by(|a, b| {
                    (a[0] - hz.log10())
                        .abs()
                        .total_cmp(&(b[0] - hz.log10()).abs())
                })
                .map(|p| p[1])
                .unwrap()
        };
        assert!(
            at(1_000.0).abs() < 3.0,
            "flat-ish in the middle: {}",
            at(1_000.0)
        );
        assert!(at(20.0) < at(1_000.0) - 15.0, "high-pass");
        assert!(at(10_000.0) < at(1_000.0) - 20.0, "muffle");
        assert!(at(100.0) > at(2_000.0) - 3.0, "warmth shelf lifts the lows");
    }

    #[test]
    fn clean_curve_is_flat() {
        let pts = response_points(&FilterParams::default(), 48_000.0);
        assert!(pts.iter().all(|p| p[1].abs() < 0.01));
    }

    #[test]
    fn curve_values_are_clamped_for_display() {
        let f = FilterParams {
            lpf_enabled: true,
            lpf_hz: 500.0,
            lpf_q: 4.0,
            ..FilterParams::default()
        };
        let pts = response_points(&f, 48_000.0);
        assert!(pts.iter().all(|p| (-90.0..=30.0).contains(&p[1])));
    }

    #[test]
    fn axis_labels() {
        assert_eq!(axis_label(2.0), "100");
        assert_eq!(axis_label(3.0), "1k");
        assert_eq!(axis_label(4.0), "10k");
        assert_eq!(axis_label(1.301), "20");
        assert_eq!(axis_label(3.5), "3.2k");
    }
}
