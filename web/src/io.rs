//! The few pieces of the desktop app's file module that the core reaches for.
//! The web build keeps scenes in memory and in OBJ/.sclp downloads, so the
//! scene-detail slots are a plain map here.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

pub mod slot {
    pub const PAINT: &str = "paint";
}

#[derive(Clone, Debug, Default)]
pub struct Details {
    scene: Map<String, Value>,
}

impl Details {
    pub fn set<T: Serialize>(&mut self, slot: &str, value: &T) {
        if let Ok(value) = serde_json::to_value(value) {
            self.scene.insert(slot.to_owned(), value);
        }
    }

    pub fn get_over<T: Serialize + DeserializeOwned>(&self, slot: &str, _base: &T) -> Option<T> {
        T::deserialize(self.scene.get(slot)?).ok()
    }
}

pub mod vec3 {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec3, s: S) -> Result<S::Ok, S::Error> {
        v.to_array().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec3, D::Error> {
        <[f32; 3]>::deserialize(d).map(Vec3::from_array)
    }
}
