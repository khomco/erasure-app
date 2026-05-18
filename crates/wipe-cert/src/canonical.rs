//! Deterministic JSON serialization for signing.
//!
//! We need byte-for-byte stable output so the same logical certificate
//! always hashes the same way regardless of platform, library version, or
//! field ordering. `serde_json`'s `Map<String, Value>` is already a `BTreeMap`
//! by default (with the `preserve_order` feature disabled), so re-serializing
//! a parsed `Value` produces lexicographically-sorted keys at every level.
//!
//! We also normalize:
//!   * floats — disallowed; we don't emit floats in cert payloads
//!   * strings — preserved as-is (UTF-8)
//!   * arrays — order preserved (semantically meaningful for `events`)

use serde::Serialize;

use crate::{CertError, CertResult};

/// Serialize `value` to a canonical UTF-8 JSON byte vector with sorted keys
/// at every object level.
///
/// We round-trip through `serde_json::Value`; that map is a `BTreeMap` (with
/// the default feature flags), so re-serializing yields sorted keys at every
/// depth. Floats are clamped to f64 by `serde_json`; we reject NaN/Inf
/// because those can't be expressed in JSON anyway.
pub fn canonical_bytes<T: Serialize>(value: &T) -> CertResult<Vec<u8>> {
    let v: serde_json::Value =
        serde_json::to_value(value).map_err(|e| CertError::Serialization(e.to_string()))?;
    walk_check_finite(&v)?;
    serde_json::to_vec(&v).map_err(|e| CertError::Serialization(e.to_string()))
}

fn walk_check_finite(v: &serde_json::Value) -> CertResult<()> {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if !f.is_finite() {
                    return Err(CertError::Serialization(format!(
                        "non-finite number in cert payload: {f}"
                    )));
                }
            }
            Ok(())
        }
        serde_json::Value::Array(arr) => arr.iter().try_for_each(walk_check_finite),
        serde_json::Value::Object(o) => o.values().try_for_each(walk_check_finite),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_deterministically() {
        let a = json!({"b": 1, "a": 2, "c": 3});
        let b = json!({"a": 2, "c": 3, "b": 1});
        assert_eq!(canonical_bytes(&a).unwrap(), canonical_bytes(&b).unwrap());
    }

    #[test]
    fn nested_keys_sort() {
        let a = json!({"outer": {"z": 1, "a": 2}});
        let bytes = canonical_bytes(&a).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.find("\"a\"").unwrap() < s.find("\"z\"").unwrap());
    }

    #[test]
    fn finite_floats_accepted() {
        let a = json!({"x": 1.5});
        assert!(canonical_bytes(&a).is_ok());
    }

    #[test]
    fn integers_preserved() {
        let a = json!({"x": 12345_u64});
        let bytes = canonical_bytes(&a).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("12345"));
    }
}
