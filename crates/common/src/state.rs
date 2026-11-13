//! Tiny JSON key/value file (owner-only permissions).

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct State {
    path: PathBuf,
    data: Mutex<Map<String, Value>>,
}

impl State {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, data: Mutex::new(data) }
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let d = self.data.lock().unwrap();
        d.get(key).and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn set<T: Serialize>(&self, key: &str, value: &T) {
        let v = serde_json::to_value(value).unwrap_or(Value::Null);
        self.update(|d| {
            d.insert(key.to_string(), v);
        });
    }

    pub fn remove(&self, key: &str) {
        self.update(|d| {
            d.remove(key);
        });
    }

    fn update(&self, f: impl FnOnce(&mut Map<String, Value>)) {
        let mut d = self.data.lock().unwrap();
        f(&mut d);
        let text = serde_json::to_string_pretty(&*d).unwrap_or_default();
        let tmp = self.path.with_extension("tmp");
        if crate::write_private(&tmp, &text).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}
