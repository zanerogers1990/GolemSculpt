//! What the hosted tools keep in a `.sclp`: each tool's `Saved` value for an
//! object, as JSON, in that object's entry of the file under the tool's id.
//!
//! The file code learns nothing about what is inside. It is the one part of a
//! file that is the tools' own, so an older build that has never heard of a
//! tool steps over its field (the format notes in `io.rs`), and a build whose
//! tool has since changed reads what it can of it (`Saved` is read leniently by
//! the tool).

use serde_json::{Map, Number, Value};
use sculpt_toolkit::Saved;

use super::{Details, OBJECT_FIELDS};

/// A whole number is written as one (`5`, not `5.0`), anything else as the
/// double it is; a number that is not a number cannot be written in JSON and
/// becomes `null`, which reads back as nothing.
pub fn to_json(saved: &Saved) -> Value {
    match saved {
        Saved::Null => Value::Null,
        Saved::Bool(b) => Value::Bool(*b),
        Saved::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Value::from(*n as i64),
        Saved::Number(n) => Number::from_f64(*n).map_or(Value::Null, Value::Number),
        Saved::Text(text) => Value::String(text.clone()),
        Saved::List(items) => Value::Array(items.iter().map(to_json).collect()),
        Saved::Fields(fields) => {
            Value::Object(fields.iter().map(|(name, value)| (name.clone(), to_json(value))).collect::<Map<_, _>>())
        }
    }
}

/// Whatever the JSON holds, as a `Saved`. (A file's JSON is parsed with a depth
/// limit, so this cannot be taken for a ride by deep nesting.)
pub fn from_json(value: &Value) -> Saved {
    match value {
        Value::Null => Saved::Null,
        Value::Bool(b) => Saved::Bool(*b),
        Value::Number(n) => n.as_f64().map_or(Saved::Null, Saved::Number),
        Value::String(text) => Saved::Text(text.clone()),
        Value::Array(items) => Saved::List(items.iter().map(from_json).collect()),
        Value::Object(fields) => Saved::Fields(fields.iter().map(|(name, value)| (name.clone(), from_json(value))).collect()),
    }
}

impl Details {
    /// Keeps what a tool saved for one object (by its row in the object list)
    /// under the tool's id. A tool whose id is one of the object entry's own
    /// fields (`name`, `mesh`...) is not kept: it would take the object's place.
    pub fn set_tool_object(&mut self, row: usize, tool: &str, saved: &Saved) {
        if !OBJECT_FIELDS.contains(&tool) {
            self.set_object(row, tool, &to_json(saved));
        }
    }

    /// What the file kept for one object under a tool's id, if it kept anything.
    pub fn get_tool_object(&self, row: usize, tool: &str) -> Option<Saved> {
        if OBJECT_FIELDS.contains(&tool) {
            return None;
        }
        self.objects.get(row)?.get(tool).map(from_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_go_through_json_and_come_back() {
        let saved = Saved::fields([
            ("count", 5.into()),
            ("scale", 0.3_f32.into()),
            ("closed", true.into()),
            ("name", "arc".into()),
            ("none", Saved::Null),
            ("points", Saved::List(vec![Saved::numbers(&[0.0, 1.5, -2.0]), Saved::numbers(&[1.0e-7, 3.0e9, -0.0])])),
        ]);
        let text = serde_json::to_string(&to_json(&saved)).unwrap();
        assert!(text.contains("\"count\":5,"), "a whole number is written whole: {text}");
        assert!(text.contains("\"scale\":0.3"), "{text}");
        let back = from_json(&serde_json::from_str(&text).unwrap());
        // Fields come back by name, in the order JSON keeps them; the numbers are the same numbers.
        for name in ["count", "scale", "closed", "name", "none", "points"] {
            assert_eq!(back.get(name), saved.get(name), "{name}");
        }
    }

    #[test]
    fn a_number_that_is_not_one_is_written_as_nothing() {
        assert_eq!(to_json(&Saved::Number(f64::NAN)), Value::Null);
        assert_eq!(to_json(&Saved::Number(f64::INFINITY)), Value::Null);
        assert_eq!(from_json(&Value::Null), Saved::Null);
        assert_eq!(to_json(&Saved::Number(1.0e300)), Value::from(1.0e300));
    }

    #[test]
    fn a_tool_is_kept_by_its_id_under_the_objects_row_and_nothing_else_of_the_entry_is_touched() {
        let mut details = Details::default();
        details.set_tool_object(2, "array", &Saved::fields([("kind", 1.into())]));
        assert!(details.get_tool_object(0, "array").is_none());
        assert!(details.get_tool_object(1, "array").is_none());
        assert_eq!(details.get_tool_object(2, "array").and_then(|s| s.get("kind").and_then(Saved::whole)), Some(1));
        assert!(details.get_tool_object(2, "other").is_none());
        assert!(details.get_tool_object(7, "array").is_none(), "a row the file does not have");
        // A tool named like a field of the entry itself would take the object's place: it is not kept.
        details.set_tool_object(0, "mesh", &Saved::from(3));
        assert!(details.get_tool_object(0, "mesh").is_none());
    }
}
