//! The one mapping between world units and real lengths, for everything that
//! measures, sizes or exports a model for printing.
//!
//! Fixed rather than a per-scene setting: an object is given its real size by
//! scaling its transform, so the ratio only decides how big geometry is in
//! world units, never what a model measures. Ten millimetres per unit keeps
//! typical printed parts (10-200mm) at 1-20 units, the range the brushes,
//! remesher and camera are tuned around (the startup sphere is a 10mm ball),
//! and it is the mold maker's own default scale, so a model sized here opens
//! there at the same size.
//!
//! Files are the other end of it: STL is read and written as millimetres, which
//! is what every slicer assumes.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    #[default]
    Millimetres,
    Inches,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 2] = [LengthUnit::Millimetres, LengthUnit::Inches];

    /// "mm" or "in".
    pub fn suffix(self) -> &'static str {
        match self {
            LengthUnit::Millimetres => "mm",
            LengthUnit::Inches => "in",
        }
    }
}

pub const MILLIMETRES_PER_UNIT: f32 = 10.0;
pub const MILLIMETRES_PER_INCH: f32 = 25.4;

pub fn to_millimetres(world_units: f32) -> f32 {
    world_units * MILLIMETRES_PER_UNIT
}

pub fn from_millimetres(mm: f32) -> f32 {
    mm / MILLIMETRES_PER_UNIT
}

/// World units to a length in `unit`.
pub fn to_unit(world_units: f32, unit: LengthUnit) -> f32 {
    let mm = to_millimetres(world_units);
    match unit {
        LengthUnit::Inches => mm / MILLIMETRES_PER_INCH,
        LengthUnit::Millimetres => mm,
    }
}

/// A length in `unit` to world units.
pub fn from_unit(value: f32, unit: LengthUnit) -> f32 {
    from_millimetres(match unit {
        LengthUnit::Inches => value * MILLIMETRES_PER_INCH,
        LengthUnit::Millimetres => value,
    })
}

/// A world length as text in `unit`: "25.40 mm", "1.000 in". Precision tracks
/// size so a small feature still shows a meaningful digit and a large one no
/// noise: a hundredth of a millimetre is below what any printer resolves, a
/// thousandth of an inch is the usual machinist's reading.
pub fn format(world_units: f32, unit: LengthUnit, with_suffix: bool) -> String {
    let number = format_number(to_unit(world_units, unit), unit);
    if with_suffix { format!("{number} {}", unit.suffix()) } else { number }
}

/// Just the number, in the same precision `format` uses: for an editable field.
pub fn format_number(value: f32, unit: LengthUnit) -> String {
    let a = value.abs();
    let decimals = match unit {
        LengthUnit::Inches if a >= 100.0 => 1,
        LengthUnit::Inches if a >= 10.0 => 2,
        LengthUnit::Inches => 3,
        LengthUnit::Millimetres if a >= 1000.0 => 0,
        LengthUnit::Millimetres if a >= 100.0 => 1,
        LengthUnit::Millimetres if a >= 1.0 => 2,
        LengthUnit::Millimetres => 3,
    };
    format!("{value:.decimals$}")
}

/// Reads a typed length back into world units. A bare number is taken in
/// `default_unit`; a suffix overrides it, so "3in" works while the display is in
/// millimetres and "76mm" while it is in inches. Also takes cm, m, ft and the
/// inch/foot marks (3" / 1'), and a simple fraction ("1/2", "1 1/4 in") because
/// that is how inch sizes get written. `None` for anything that is not a finite
/// length.
pub fn parse(text: &str, default_unit: LengthUnit) -> Option<f32> {
    if text.trim().is_empty() {
        return None;
    }
    let lowered = text.trim().to_lowercase().replace(',', ".");
    let mut s = lowered.as_str();

    let mut mm_per_value = match default_unit {
        LengthUnit::Inches => MILLIMETRES_PER_INCH,
        LengthUnit::Millimetres => 1.0,
    };
    if strip_any(&mut s, &["millimetres", "millimeters", "mm"]) {
        mm_per_value = 1.0;
    } else if strip_any(&mut s, &["cm"]) {
        mm_per_value = 10.0;
    } else if strip_any(&mut s, &["inches", "inch", "in", "\""]) {
        mm_per_value = MILLIMETRES_PER_INCH;
    } else if strip_any(&mut s, &["feet", "foot", "ft", "'"]) {
        mm_per_value = MILLIMETRES_PER_INCH * 12.0;
    } else if strip_any(&mut s, &["m"]) {
        mm_per_value = 1000.0;
    }

    let value = parse_number(s.trim())?;
    value.is_finite().then(|| from_millimetres(value * mm_per_value))
}

/// Strips the first of `suffixes` that `s` ends in, in order, as `strip_suffix` does.
fn strip_any(s: &mut &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suffix| strip_suffix(s, suffix))
}

/// "mm" must not be read as "m" by a later check, and "in" must follow a number,
/// not be the tail of some other word, so the suffix only comes off when a number
/// (or fraction) is left.
fn strip_suffix(s: &mut &str, suffix: &str) -> bool {
    let Some(rest) = s.strip_suffix(suffix) else { return false };
    let rest = rest.trim_end();
    match rest.chars().last() {
        Some(last) if last.is_ascii_digit() || last == '.' => {
            *s = rest;
            true
        }
        _ => false,
    }
}

/// "12", "12.5", "1/2", "1 1/4".
fn parse_number(s: &str) -> Option<f32> {
    let float = |text: &str| text.trim().parse::<f32>().ok();
    let Some(slash) = s.find('/') else { return float(s) };

    let (whole, fraction) = match s[..slash].rfind(' ') {
        Some(space) => (s[..space].trim(), s[space + 1..].trim()),
        None => ("0", s),
    };
    let slash = fraction.find('/')?;
    let whole = float(whole)?;
    let numerator = float(&fraction[..slash])?;
    let denominator = float(&fraction[slash + 1..])?;
    if denominator.abs() < 1e-9 {
        return None;
    }
    // "-1 1/4" is minus one and a quarter, not minus three quarters.
    let part = numerator / denominator;
    Some(if whole.is_sign_negative() { whole - part } else { whole + part })
}

#[cfg(test)]
mod tests {
    use super::*;
    use LengthUnit::{Inches, Millimetres};

    /// One millimetre in world units.
    const MM: f32 = 1.0 / MILLIMETRES_PER_UNIT;

    fn close(a: f32, b: f32, tolerance: f32) {
        assert!((a - b).abs() <= tolerance, "{a} vs {b}");
    }

    #[test]
    fn converts_both_ways_in_either_unit() {
        close(to_unit(25.4 * MM, Millimetres), 25.4, 1e-4);
        close(to_unit(25.4 * MM, Inches), 1.0, 1e-5);
        close(from_unit(1.0, Inches), 2.54, 1e-5);
        close(from_unit(75.0, Millimetres), 7.5, 1e-5);
    }

    #[test]
    fn formats_with_unit_and_sensible_precision() {
        assert_eq!(format(25.4 * MM, Millimetres, true), "25.40 mm");
        assert_eq!(format(25.4 * MM, Inches, true), "1.000 in");
        assert_eq!(format(150.0 * MM, Millimetres, true), "150.0 mm");
        assert_eq!(format(0.5 * MM, Millimetres, true), "0.500 mm");
        assert_eq!(format(12.0 * MM, Millimetres, false), "12.00");
        assert_eq!(format(2500.0 * MM, Millimetres, true), "2500 mm");
        assert_eq!(format(300.0 * 25.4 * MM, Inches, true), "300.0 in");
        assert_eq!(format(30.0 * 25.4 * MM, Inches, true), "30.00 in");
    }

    #[test]
    fn parses_typed_sizes() {
        let cases: [(&str, LengthUnit, f32); 17] = [
            ("50", Millimetres, 50.0),
            ("50", Inches, 1270.0),
            ("50mm", Inches, 50.0),
            (" 76 mm ", Inches, 76.0),
            ("3in", Millimetres, 76.2),
            ("3 inches", Millimetres, 76.2),
            ("3\"", Millimetres, 76.2),
            ("1/2", Inches, 12.7),
            ("1 1/4 in", Millimetres, 31.75),
            ("2.5cm", Inches, 25.0),
            ("0.1m", Inches, 100.0),
            ("1ft", Millimetres, 304.8),
            ("12,5", Millimetres, 12.5),
            ("2'", Millimetres, 609.6),
            ("1.5 MM", Inches, 1.5),
            ("-1 1/4 in", Millimetres, -31.75),
            ("1e1", Millimetres, 10.0),
        ];
        for (text, default_unit, expected_mm) in cases {
            let world = parse(text, default_unit).unwrap_or_else(|| panic!("{text:?} did not parse"));
            close(to_millimetres(world), expected_mm, 1e-3);
        }
    }

    #[test]
    fn rejects_non_sizes() {
        for text in ["", "   ", "mm", "abc", "1/0", "in", "1/", "/2", "inf", "nan", "1 2", "1/2/3"] {
            assert_eq!(parse(text, Millimetres), None, "{text:?}");
        }
    }

    /// What the panel writes into a field must read back as the same size.
    #[test]
    fn formatted_numbers_round_trip_through_the_parser() {
        for unit in LengthUnit::ALL {
            for world in [0.5f32, 1.0, 9.13, 48.7] {
                let shown = format_number(to_unit(world, unit), unit);
                let back = parse(&shown, unit).unwrap_or_else(|| panic!("{shown:?} did not parse"));
                close(back, world, world * 0.01);
            }
        }
    }

    #[test]
    fn suffixes_name_the_unit() {
        assert_eq!(Millimetres.suffix(), "mm");
        assert_eq!(Inches.suffix(), "in");
        assert_eq!(LengthUnit::default(), Millimetres);
    }
}
