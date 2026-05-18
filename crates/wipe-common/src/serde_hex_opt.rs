//! Serde adapter for `Option<Vec<u8>>` as hex string in JSON.

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(bytes) => s.serialize_str(&hex::encode(bytes)),
        None => s.serialize_none(),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        Some(s) => hex::decode(&s).map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}
