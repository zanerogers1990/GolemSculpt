//! Typing a number into a slider (`theme::slider`): a double tap on it opens
//! a box over its readout, and what is typed there becomes its value.
//!
//! A slider does not know what its number means. It holds a value along its
//! track and shows it through a readout of the caller's: "45%" for a strength
//! of 0.45, "256" for a resolution kept as its logarithm, "1.20M" for a count.
//! What is typed is in the readout's terms, so it is found on the track by
//! reading the readout back: the value whose readout says the number typed.
//! A readout rounds ("45%" stands for 0.445 to 0.455), so of all the values
//! that read the same the middle one is taken: typing 45 gives 0.45 and not
//! 0.445. A number between two readouts lands between them, and one past
//! either end of the track at that end.
//!
//! This needs a readout that grows (or shrinks) steadily along the track and
//! is a number at both ends; a slider whose readout is not offers no box.

/// The number a readout or a typed text starts with: "45%" is 45, "120 px"
/// 120, "-0.3" -0.3, "1.2k" 1200 and "3.5M" 3,500,000. `None` when it does
/// not start with one.
pub fn number(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', "");
    let end = text
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(text.len(), |(i, _)| i);
    let value: f64 = text[..end].parse().ok()?;
    let scale = match text[end..].trim_start().chars().next() {
        Some('k') | Some('K') => 1e3,
        Some('M') => 1e6,
        _ => 1.0,
    };
    Some(value * scale).filter(|v| v.is_finite())
}

/// The slider value in `min..=max` whose readout says `typed`, or `None` if
/// this readout cannot be read back (see the top of this file).
pub fn value_for(typed: f64, min: f32, max: f32, readout: &dyn Fn(f32) -> String) -> Option<f32> {
    let read = |v: f32| number(&readout(v));
    let (low, high) = (read(min)?, read(max)?);
    if !typed.is_finite() || low == high {
        return None;
    }
    // Along the track the readout grows (or, flipped, shrinks): `says` is
    // the readout made to grow either way.
    let flip = if high > low { 1.0 } else { -1.0 };
    let says = |v: f32| read(v).map(|n| n * flip);
    let want = typed * flip;
    // The first value that says `want` or more, and the first that says
    // more: between them every value says `want` exactly. Past the end of
    // the track, the end.
    let first = search(min, max, |v| says(v).is_some_and(|n| n >= want)).unwrap_or(max);
    let last = search(min, max, |v| says(v).is_some_and(|n| n > want));
    let exact = says(first).is_some_and(|n| (n - want).abs() <= 1e-9 * want.abs().max(1.0));
    // A number the track's end reads as is that end: 100% is 1, not 0.9975.
    let value = match (exact, last) {
        (false, _) => first,
        (true, None) => max,
        (true, Some(_)) if first == min => min,
        (true, Some(last)) => 0.5 * (first + last),
    };
    Some(value.clamp(min, max))
}

/// The smallest value in `min..=max` for which `past` holds, `past` being
/// false and then true along the range. `None` if it never holds.
fn search(min: f32, max: f32, past: impl Fn(f32) -> bool) -> Option<f32> {
    if past(min) {
        return Some(min);
    }
    if !past(max) {
        return None;
    }
    let (mut lo, mut hi) = (min, max);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if past(mid) { hi = mid } else { lo = mid }
    }
    Some(hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Option<f32>, b: f32) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-3 * b.abs().max(1.0))
    }

    #[test]
    fn a_readout_is_read_back_as_its_number() {
        assert_eq!(number("45%"), Some(45.0));
        assert_eq!(number(" 120 px"), Some(120.0));
        assert_eq!(number("-0.30"), Some(-0.3));
        assert_eq!(number("1.2k"), Some(1200.0));
        assert_eq!(number("3.5M"), Some(3_500_000.0));
        assert_eq!(number("12,000"), Some(12_000.0));
        assert_eq!(number("Off"), None);
        assert_eq!(number(""), None);
    }

    #[test]
    fn a_typed_percent_is_the_strength_it_reads_as() {
        let percent = |v: f32| format!("{:.0}%", v * 100.0);
        assert!(close(value_for(45.0, 0.01, 1.0, &percent), 0.45), "the middle of what reads 45%, not its edge");
        assert!(close(value_for(100.0, 0.01, 1.0, &percent), 1.0));
        assert!(close(value_for(500.0, 0.01, 1.0, &percent), 1.0), "past the end: the end");
        assert!(close(value_for(0.0, 0.01, 1.0, &percent), 0.01), "before the start: the start");
    }

    #[test]
    fn a_log_slider_takes_the_number_it_shows() {
        // The remesh resolution: kept as its logarithm, shown as itself.
        let shown = |v: f32| format!("{:.0}", v.exp2());
        let v = value_for(256.0, 4f32.log2(), 1024f32.log2(), &shown).unwrap();
        assert!((v.exp2() - 256.0).abs() < 0.5, "{}", v.exp2());
    }

    #[test]
    fn a_shrinking_readout_and_a_signed_one_work_too() {
        let focal = |v: f32| format!("{:.0}", v * 100.0);
        assert!(close(value_for(-30.0, -1.0, 1.0, &focal), -0.3));
        let backwards = |v: f32| format!("{:.2}", 1.0 - v);
        assert!(close(value_for(0.25, 0.0, 1.0, &backwards), 0.75));
        let words = |v: f32| if v < 0.5 { "Off".to_owned() } else { format!("{v:.1}") };
        assert_eq!(value_for(0.7, 0.0, 1.0, &words), None, "not a number at both ends: no box");
    }
}
