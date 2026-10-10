//! What a tool keeps in a saved scene, and how a file's objects reach it.
//!
//! A tool's scaffold (an array's handle points, say) is the tool's, not the
//! scene's, so the app's file code cannot know its shape. It does not need to:
//! the tool turns what it holds for one object into a [`Saved`] value, the
//! app writes that into the file under the tool's id in that object's entry,
//! and on the way back hands each tool the values its objects had. Neither
//! the toolkit nor the file code learns anything about what is inside.
//!
//! A [`Saved`] is the shapes JSON has and nothing else, so it can be written
//! and read by any version of the app and ignored by one that has never heard
//! of it. Read one the way a hostile file would be read: every accessor says
//! `None` for the wrong kind, and a number that is not a number is no number.

/// A value a tool writes into a scene file for one object.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Saved {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Saved>),
    /// Named values, in the order they were written.
    Fields(Vec<(String, Saved)>),
}

impl Saved {
    /// A set of named values: `Saved::fields([("count", 5.into()), ("closed", false.into())])`.
    pub fn fields<const N: usize>(fields: [(&str, Saved); N]) -> Saved {
        Saved::Fields(fields.into_iter().map(|(name, value)| (name.to_owned(), value)).collect())
    }

    /// A list of numbers, such as the three of a point.
    pub fn numbers(values: &[f32]) -> Saved {
        Saved::List(values.iter().map(|&v| Saved::from(v)).collect())
    }

    /// The value written under `name`, if this is a set of named values that has one.
    pub fn get(&self, name: &str) -> Option<&Saved> {
        match self {
            Saved::Fields(fields) => fields.iter().find(|(key, _)| key == name).map(|(_, value)| value),
            _ => None,
        }
    }

    /// The number, if this is one that is not infinite or not-a-number.
    pub fn number(&self) -> Option<f64> {
        match *self {
            Saved::Number(n) if n.is_finite() => Some(n),
            _ => None,
        }
    }

    /// The number as the whole number it holds, if it holds one in range.
    pub fn whole(&self) -> Option<i32> {
        let n = self.number()?;
        (n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64).then_some(n as i32)
    }

    pub fn bool(&self) -> Option<bool> {
        match *self {
            Saved::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Saved::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn list(&self) -> Option<&[Saved]> {
        match self {
            Saved::List(items) => Some(items),
            _ => None,
        }
    }

    /// A list of exactly `N` numbers, all of them numbers (a point, a colour).
    pub fn array<const N: usize>(&self) -> Option<[f32; N]> {
        let items = self.list().filter(|items| items.len() == N)?;
        let mut out = [0.0; N];
        for (slot, item) in out.iter_mut().zip(items) {
            *slot = item.number()? as f32;
        }
        Some(out)
    }
}

impl From<bool> for Saved {
    fn from(value: bool) -> Saved {
        Saved::Bool(value)
    }
}

impl From<f32> for Saved {
    /// The number as the shortest decimal that is this `f32` (0.3, not
    /// 0.30000001192092896), which is what a person reading the file sees, and reads
    /// back as the very same `f32`.
    fn from(value: f32) -> Saved {
        Saved::Number(value.to_string().parse().unwrap_or(value as f64))
    }
}

impl From<f64> for Saved {
    fn from(value: f64) -> Saved {
        Saved::Number(value)
    }
}

impl From<i32> for Saved {
    fn from(value: i32) -> Saved {
        Saved::Number(value as f64)
    }
}

impl From<&str> for Saved {
    fn from(value: &str) -> Saved {
        Saved::Text(value.to_owned())
    }
}

/// How the objects of a file came into the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    /// Open: the file's objects took the place of every object there was. Those
    /// are still there for Undo to bring back, but nothing the tool was doing
    /// with them carries on: its own undo history is over, and the scene's
    /// answers the next Z.
    Replaced,
    /// Import ("Add"): the file's objects joined the scene beside the others,
    /// which are untouched.
    Added,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_values_are_found_by_name_and_only_in_a_set_of_them() {
        let saved = Saved::fields([("count", 5.into()), ("closed", false.into()), ("name", "arc".into())]);
        assert_eq!(saved.get("count").and_then(Saved::whole), Some(5));
        assert_eq!(saved.get("closed").and_then(Saved::bool), Some(false));
        assert_eq!(saved.get("name").and_then(Saved::text), Some("arc"));
        assert!(saved.get("missing").is_none());
        assert!(Saved::from(3).get("count").is_none());
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_nothing() {
        assert_eq!(Saved::from("5").number(), None);
        assert_eq!(Saved::from(5).bool(), None);
        assert_eq!(Saved::Null.text(), None);
        assert_eq!(Saved::from(5).list(), None);
    }

    #[test]
    fn a_single_precision_number_is_written_short_and_reads_back_the_same() {
        for value in [0.3_f32, 1.0 / 3.0, 12345.678, -0.0, 1.0e-30, 3.4e38, f32::MIN_POSITIVE] {
            let saved = Saved::from(value);
            assert_eq!(saved.number().map(|n| n as f32).map(f32::to_bits), Some(value.to_bits()), "{value}");
        }
        assert_eq!(Saved::from(0.3_f32), Saved::Number(0.3));
    }

    #[test]
    fn a_number_that_is_not_a_number_is_no_number() {
        assert_eq!(Saved::Number(f64::NAN).number(), None);
        assert_eq!(Saved::Number(f64::INFINITY).number(), None);
        assert_eq!(Saved::Number(2.5).number(), Some(2.5));
    }

    #[test]
    fn a_whole_number_is_one_that_fits() {
        assert_eq!(Saved::Number(7.0).whole(), Some(7));
        assert_eq!(Saved::Number(-7.0).whole(), Some(-7));
        assert_eq!(Saved::Number(7.5).whole(), None);
        assert_eq!(Saved::Number(3.0e10).whole(), None);
        assert_eq!(Saved::Number(f64::NAN).whole(), None);
    }

    #[test]
    fn an_array_of_numbers_must_be_the_right_length_and_all_numbers() {
        let point = Saved::numbers(&[1.0, 2.0, 3.0]);
        assert_eq!(point.array::<3>(), Some([1.0, 2.0, 3.0]));
        assert_eq!(point.array::<2>(), None);
        let bad = Saved::List(vec![1.into(), "x".into(), 3.into()]);
        assert_eq!(bad.array::<3>(), None);
        let nan = Saved::List(vec![1.into(), Saved::Number(f64::NAN), 3.into()]);
        assert_eq!(nan.array::<3>(), None);
    }
}
