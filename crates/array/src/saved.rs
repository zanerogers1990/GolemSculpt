//! The array as a scene file keeps it: an object's `ArrayState` as the plain
//! values of `sculpt_toolkit::Saved`, and back.
//!
//! The app writes whatever `Tool::save_object` hands it under the tool's id in
//! the object's entry of the file's JSON, so an array is one more field of the
//! object, `"array"`, and a file without it has none. The pose needs no field of
//! its own: it is the object's saved placement and `points[0]`, and the mirror
//! is the object's own live mirror, which the file already keeps.
//!
//! ```json
//! "array": { "kind": 2, "count": 5, "end_scale": 1, "mid_scale": 1, "closed": false,
//!            "follow": true, "space_by_size": true, "axis": 1, "sweep": 360, "spacing": 0,
//!            "points": [[0, 0, 0], [1, 0.5, 0], [2, 0, 0]] }
//! ```
//!
//! The names are the Unity file's, written the way the rest of the file is
//! (snake case). Reading is the Unity loader's rule and a little more careful:
//! a kind this build does not know, fewer than two points, or a point that is
//! not three numbers means no array at all rather than a guess, and a setting
//! that is missing or not a number takes the value a new array starts with.
//! Whatever comes through is put in range by `ArrayState::sanitized`, so a
//! hostile count or a million points cannot reach the layout.

use sculpt_toolkit::Saved;

use crate::state::ArrayState;
use crate::types::{ArrayKind, ArraySettings};

/// An object's array in the form a scene file keeps it.
pub fn to_saved(state: &ArrayState) -> Saved {
    let s = &state.settings;
    Saved::fields([
        ("kind", s.kind.id().into()),
        ("count", s.count.into()),
        ("end_scale", s.end_scale.into()),
        ("mid_scale", s.mid_scale.into()),
        ("closed", s.closed.into()),
        ("follow", s.follow.into()),
        ("space_by_size", s.space_by_size.into()),
        ("axis", s.axis.into()),
        ("sweep", s.sweep.into()),
        ("spacing", s.spacing.into()),
        ("points", Saved::List(state.points.iter().map(|p| Saved::numbers(&p.to_array())).collect())),
    ])
}

/// The array a file kept, or `None` when it holds no array that can be read.
/// The result is in range (see the module remarks).
pub fn from_saved(saved: &Saved) -> Option<ArrayState> {
    let kind = ArrayKind::from_id(saved.get("kind")?.whole()?);
    if kind == ArrayKind::None {
        return None;
    }
    let points = points_of(saved.get("points")?)?;
    if points.len() < 2 {
        return None;
    }
    let new = ArraySettings::new(kind);
    let number = |name: &str, fallback: f32| saved.get(name).and_then(Saved::number).map_or(fallback, |n| n as f32);
    let flag = |name: &str, fallback: bool| saved.get(name).and_then(Saved::bool).unwrap_or(fallback);
    let whole = |name: &str, fallback: i32| {
        // A count from a damaged file may be anything; it is rounded to the nearest whole number
        // and held to the range of `i32` here, and to the range of arrays by `clamped`.
        let n = saved.get(name).and_then(Saved::number).map_or(fallback as f64, f64::round);
        n.clamp(i32::MIN as f64, i32::MAX as f64) as i32
    };
    let settings = ArraySettings {
        kind,
        count: whole("count", new.count),
        end_scale: number("end_scale", new.end_scale),
        mid_scale: number("mid_scale", new.mid_scale),
        closed: flag("closed", new.closed),
        follow: flag("follow", new.follow),
        space_by_size: flag("space_by_size", new.space_by_size),
        axis: whole("axis", new.axis),
        sweep: number("sweep", new.sweep),
        spacing: number("spacing", new.spacing),
    };
    Some(ArrayState { settings, points }.sanitized())
}

/// Every point of a list, or `None` when any of those that count is not a place. Points past
/// the most an array keeps are not looked at.
fn points_of(saved: &Saved) -> Option<Vec<glam::Vec3>> {
    saved
        .list()?
        .iter()
        .take(ArrayState::MAX_POINTS)
        .map(|point| point.array::<3>().map(glam::Vec3::from_array))
        .collect()
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    fn state(kind: ArrayKind) -> ArrayState {
        let mut settings = ArraySettings::new(kind);
        settings.count = 7;
        settings.end_scale = 0.3;
        settings.mid_scale = 1.7;
        settings.closed = kind == ArrayKind::Curve;
        settings.follow = false;
        settings.space_by_size = false;
        settings.axis = 2;
        settings.sweep = if kind == ArrayKind::Radial { -135.5 } else { 360.0 };
        settings.spacing = if kind == ArrayKind::Radial { 0.0 } else { 1.25 };
        let points = vec![Vec3::new(0.0, 0.1, -0.7), Vec3::new(2.0, 1.0, 0.3), Vec3::new(3.5, -1.25, 2.0), Vec3::Z];
        ArrayState { settings, points }
    }

    #[test]
    fn every_kind_comes_back_exactly_as_it_was_written() {
        for kind in [ArrayKind::Line, ArrayKind::Curve, ArrayKind::Radial] {
            let state = state(kind);
            let saved = to_saved(&state);
            assert_eq!(from_saved(&saved), Some(state.sanitized()), "{kind:?}");
        }
    }

    #[test]
    fn the_numbers_are_kept_to_the_last_bit_and_written_the_short_way() {
        let mut odd = state(ArrayKind::Line);
        odd.points[1] = Vec3::new(0.1 + 0.2, 1.0 / 3.0, 12345.678);
        odd.settings.end_scale = 0.3;
        let saved = to_saved(&odd);
        assert_eq!(from_saved(&saved).unwrap().points, odd.points);
        assert_eq!(saved.get("end_scale"), Some(&Saved::Number(0.3)), "0.3, not 0.30000001192092896");
    }

    #[test]
    fn what_is_written_is_what_the_file_documents() {
        let saved = to_saved(&state(ArrayKind::Radial));
        let names: Vec<&str> = match &saved {
            Saved::Fields(fields) => fields.iter().map(|(name, _)| name.as_str()).collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            names,
            ["kind", "count", "end_scale", "mid_scale", "closed", "follow", "space_by_size", "axis", "sweep", "spacing", "points"]
        );
        assert_eq!(saved.get("kind").and_then(Saved::whole), Some(3));
        assert_eq!(saved.get("points").and_then(Saved::list).map(<[Saved]>::len), Some(4));
    }

    #[test]
    fn a_missing_or_unreadable_setting_is_what_a_new_array_starts_with() {
        let bare = Saved::fields([
            ("kind", 1.into()),
            ("points", Saved::List(vec![Saved::numbers(&[0.0; 3]), Saved::numbers(&[4.0, 0.0, 0.0])])),
            ("count", "many".into()),
            ("follow", 3.into()),
        ]);
        let read = from_saved(&bare).expect("a line with two points");
        assert_eq!(read.settings, ArraySettings::new(ArrayKind::Line));
    }

    #[test]
    fn no_array_when_there_is_nothing_to_lay_out() {
        let line = |kind: Saved, points: Saved| Saved::fields([("kind", kind), ("points", points)]);
        let two = || Saved::List(vec![Saved::numbers(&[0.0; 3]), Saved::numbers(&[1.0, 0.0, 0.0])]);
        assert!(from_saved(&line(1.into(), two())).is_some());
        assert!(from_saved(&line(0.into(), two())).is_none(), "kind none");
        assert!(from_saved(&line(9.into(), two())).is_none(), "a kind from a later version");
        assert!(from_saved(&line(1.5.into(), two())).is_none(), "a kind that is not a kind");
        assert!(from_saved(&line("line".into(), two())).is_none());
        assert!(from_saved(&line(1.into(), Saved::List(vec![Saved::numbers(&[0.0; 3])]))).is_none(), "one point");
        assert!(from_saved(&line(1.into(), Saved::List(Vec::new()))).is_none());
        assert!(from_saved(&line(1.into(), 5.into())).is_none(), "points that are not a list");
        assert!(from_saved(&Saved::fields([("kind", 1.into())])).is_none(), "no points");
        assert!(from_saved(&Saved::Null).is_none());
        assert!(from_saved(&Saved::from("array")).is_none());
        assert!(from_saved(&Saved::List(vec![1.into()])).is_none());
    }

    #[test]
    fn a_point_that_is_not_a_place_costs_the_array() {
        let with = |second: Saved| {
            Saved::fields([("kind", 2.into()), ("points", Saved::List(vec![Saved::numbers(&[0.0; 3]), second]))])
        };
        assert!(from_saved(&with(Saved::numbers(&[1.0, 2.0, 3.0]))).is_some());
        assert!(from_saved(&with(Saved::numbers(&[1.0, f32::NAN, 3.0]))).is_none(), "NaN");
        assert!(from_saved(&with(Saved::numbers(&[1.0, f32::INFINITY, 3.0]))).is_none(), "infinity");
        assert!(from_saved(&with(Saved::numbers(&[1.0, 2.0]))).is_none(), "two numbers");
        assert!(from_saved(&with(Saved::numbers(&[1.0, 2.0, 3.0, 4.0]))).is_none(), "four numbers");
        assert!(from_saved(&with(Saved::List(vec![1.into(), "x".into(), 3.into()]))).is_none(), "a word");
        assert!(from_saved(&with(Saved::Null)).is_none());
    }

    #[test]
    fn a_hostile_count_is_brought_into_range_whatever_it_is() {
        let counts = [0.0, 1.0, 2.0, 100.0, 101.0, 1.0e9, 1.0e30, -5.0, 2.6, f64::MAX, f64::MIN];
        let expected = [2, 2, 2, 100, 100, 100, 100, 2, 3, 100, 2];
        for (count, expected) in counts.into_iter().zip(expected) {
            let mut saved = to_saved(&state(ArrayKind::Line));
            if let Saved::Fields(fields) = &mut saved {
                fields.iter_mut().find(|(name, _)| name == "count").unwrap().1 = Saved::Number(count);
            }
            assert_eq!(from_saved(&saved).unwrap().settings.count, expected, "count {count}");
        }
        // Not a number at all: what a new array starts with.
        let mut saved = to_saved(&state(ArrayKind::Radial));
        if let Saved::Fields(fields) = &mut saved {
            fields.iter_mut().find(|(name, _)| name == "count").unwrap().1 = Saved::Number(f64::NAN);
        }
        assert_eq!(from_saved(&saved).unwrap().settings.count, ArraySettings::new(ArrayKind::Radial).count);
    }

    #[test]
    fn hostile_settings_and_a_great_many_points_come_out_in_range() {
        let mut saved = to_saved(&state(ArrayKind::Curve));
        if let Saved::Fields(fields) = &mut saved {
            for (name, value) in fields.iter_mut() {
                match name.as_str() {
                    "end_scale" => *value = Saved::Number(-3.0),
                    "mid_scale" => *value = Saved::Number(1.0e30),
                    "axis" => *value = Saved::Number(7.0),
                    "sweep" => *value = Saved::Number(f64::NAN),
                    "points" => {
                        *value = Saved::List(vec![Saved::numbers(&[1.0e30, -1.0e30, 0.0]); 100_000]);
                    }
                    _ => {}
                }
            }
        }
        let read = from_saved(&saved).expect("a curve");
        let s = read.settings;
        assert_eq!(
            (s.end_scale, s.mid_scale, s.axis, s.sweep),
            (ArraySettings::MIN_SCALE, ArraySettings::MAX_SCALE, 2, 360.0)
        );
        assert_eq!(read.points.len(), ArrayState::MAX_POINTS);
        assert!(read.points.iter().all(|p| p.abs().max_element() <= ArrayState::MAX_REACH));
        // And it lays out without fuss.
        let layout = crate::layout::compute(&read.settings, &read.points);
        assert_eq!(layout.len(), s.count as usize);
        assert!(layout.iter().all(|p| p.offset.is_finite() && p.rotation.is_finite() && p.scale.is_finite()));
    }
}
