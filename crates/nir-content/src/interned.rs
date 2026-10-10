//! Lossless immutable package sharing. References point backward in a bounded
//! pool; typed deserialization reads that pool directly, without constructing
//! an expanded JSON tree or changing any execution/declaration identity.
use nir_format::{Diagnostic, Result, MAX_INPUT_BYTES};
use serde::{
    de::{self, value, DeserializeOwned, IntoDeserializer, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CAPABILITY: &str = "content.interned-json.v1";
const ENCODING: &str = "interned_json_v1";
const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: u16 = 128;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    package_encoding: String,
    root: u32,
    pool: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Entry {
    V(Value),
    A(Vec<u32>),
    O(Vec<(u32, u32)>),
}

fn failure(at: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("E_JSON_POOL", at, message)
}

/// Choose sharing only when it reduces a substantial package. Small ordinary
/// JSON objects keep their existing encoding and digest.
pub fn encode<T: Serialize>(value: &T) -> Result<(Vec<u8>, bool)> {
    let raw = serde_json::to_vec(value).map_err(|e| failure("package", e.to_string()))?;
    if raw.len() < 64 * 1024 {
        return Ok((raw, false));
    }
    if raw.len() as u64 > MAX_EXPANDED_BYTES {
        return Err(failure("package", "expanded package exceeds 64 MiB"));
    }
    let value: Value =
        serde_json::from_slice(&raw).map_err(|e| failure("package", e.to_string()))?;
    let mut pool = Vec::new();
    let mut canonical = BTreeMap::new();
    fn intern(value: Value, pool: &mut Vec<Entry>, canonical: &mut BTreeMap<Vec<u8>, u32>) -> u32 {
        let entry = match value {
            Value::Array(values) => Entry::A(
                values
                    .into_iter()
                    .map(|v| intern(v, pool, canonical))
                    .collect(),
            ),
            Value::Object(values) => Entry::O(
                values
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            intern(Value::String(key), pool, canonical),
                            intern(value, pool, canonical),
                        )
                    })
                    .collect(),
            ),
            scalar => Entry::V(scalar),
        };
        // Exact entry bytes, rather than a truncated fingerprint, decide
        // equality. Every child has already obtained its canonical index.
        let bytes = serde_json::to_vec(&entry).expect("serializable pool entry");
        if let Some(index) = canonical.get(&bytes) {
            return *index;
        }
        let index = pool.len() as u32;
        canonical.insert(bytes, index);
        pool.push(entry);
        index
    }
    let root = intern(value, &mut pool, &mut canonical);
    let package = Package {
        package_encoding: ENCODING.into(),
        root,
        pool,
    };
    check(&package, "package")?;
    let encoded = serde_json::to_vec(&package).map_err(|e| failure("package", e.to_string()))?;
    if encoded.len() < raw.len() - raw.len() / 20 {
        Ok((encoded, true))
    } else {
        Ok((raw, false))
    }
}

fn check(package: &Package, at: &str) -> Result<()> {
    if package.package_encoding != ENCODING
        || package.pool.is_empty()
        || package.pool.len() > MAX_INPUT_BYTES / 2
        || package.root as usize != package.pool.len() - 1
    {
        return Err(failure(at, "invalid pool encoding, size or root"));
    }
    let mut lengths = Vec::<u64>::with_capacity(package.pool.len());
    let mut depths = Vec::<u16>::with_capacity(package.pool.len());
    let mut edges = vec![false; package.pool.len()];
    for (index, entry) in package.pool.iter().enumerate() {
        let mut length = 2u64;
        let mut depth = 1;
        let mut visit = |reference: u32| -> Result<()> {
            let child = reference as usize;
            if child >= index {
                return Err(failure(at, "pool reference must point backward"));
            }
            length = length
                .checked_add(lengths[child])
                .filter(|n| *n <= MAX_EXPANDED_BYTES)
                .ok_or_else(|| failure(at, "expanded package exceeds 64 MiB"))?;
            depth = depth.max(depths[child] + 1);
            edges[child] = true;
            Ok(())
        };
        match entry {
            Entry::V(value) if !value.is_array() && !value.is_object() => {
                length = serde_json::to_vec(value)
                    .map_err(|e| failure(at, e.to_string()))?
                    .len() as u64;
                depth = 0;
            }
            Entry::V(_) => return Err(failure(at, "scalar entry contains a container")),
            Entry::A(children) => {
                if children.len() > 100_000 {
                    return Err(failure(at, "logical array exceeds 100000 entries"));
                }
                for child in children {
                    visit(*child)?;
                }
                length += children.len().saturating_sub(1) as u64;
            }
            Entry::O(members) => {
                if members.len() > 100_000 {
                    return Err(failure(at, "logical object exceeds 100000 members"));
                }
                let mut previous: Option<&str> = None;
                for (key, value) in members {
                    visit(*key)?;
                    visit(*value)?;
                    let Entry::V(Value::String(key)) = &package.pool[*key as usize] else {
                        return Err(failure(at, "object key must reference a string"));
                    };
                    if previous.is_some_and(|last| last >= key.as_str()) {
                        return Err(failure(at, "duplicate or unsorted object keys"));
                    }
                    previous = Some(key);
                }
                length += members.len() as u64 + members.len().saturating_sub(1) as u64;
            }
        }
        if length > MAX_EXPANDED_BYTES || depth > MAX_DEPTH {
            return Err(failure(at, "expanded package exceeds byte or depth bound"));
        }
        lengths.push(length);
        depths.push(depth);
    }
    // Every entry other than the final root must contribute to a later one.
    // Backward references make this equivalent to full root reachability.
    if edges[..edges.len() - 1]
        .iter()
        .any(|referenced| !referenced)
    {
        return Err(failure(at, "unreachable pool entry"));
    }
    Ok(())
}

/// The caller has already authenticated and strictly parsed the envelope.
pub fn decode<T: DeserializeOwned>(value: Value, at: &str) -> Result<T> {
    let package: Package = serde_json::from_value(value).map_err(|e| failure(at, e.to_string()))?;
    decode_package(package, at)
}

/// The pool is an encoding table, not a logical content array. Deserialize
/// its typed envelope without applying ordinary JSON container limits to the
/// table; byte, reference, expansion and depth limits still apply.
pub fn decode_bytes<T: DeserializeOwned>(bytes: &[u8], at: &str) -> Result<T> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(failure(at, "input exceeds 16 MiB"));
    }
    let package: Package = serde_json::from_slice(bytes).map_err(|e| failure(at, e.to_string()))?;
    decode_package(package, at)
}

fn decode_package<T: DeserializeOwned>(package: Package, at: &str) -> Result<T> {
    check(&package, at)?;
    T::deserialize(PoolValue {
        package: &package,
        index: package.root,
    })
    .map_err(|e| failure(at, e.to_string()))
}

#[derive(Clone, Copy)]
struct PoolValue<'a> {
    package: &'a Package,
    index: u32,
}
impl<'a> PoolValue<'a> {
    fn entry(self) -> &'a Entry {
        &self.package.pool[self.index as usize]
    }
    fn child(self, index: u32) -> Self {
        Self { index, ..self }
    }
}
impl<'de> IntoDeserializer<'de, value::Error> for PoolValue<'de> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}
impl<'de> Deserializer<'de> for PoolValue<'de> {
    type Error = value::Error;
    fn deserialize_any<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        match self.entry() {
            Entry::V(Value::Null) => visitor.visit_unit(),
            Entry::V(Value::Bool(value)) => visitor.visit_bool(*value),
            Entry::V(Value::String(value)) => visitor.visit_borrowed_str(value),
            Entry::V(Value::Number(value)) => {
                if let Some(value) = value.as_i64() {
                    visitor.visit_i64(value)
                } else if let Some(value) = value.as_u64() {
                    visitor.visit_u64(value)
                } else {
                    visitor.visit_f64(value.as_f64().expect("JSON number"))
                }
            }
            Entry::A(children) => visitor.visit_seq(value::SeqDeserializer::new(
                children.iter().map(|index| self.child(*index)),
            )),
            Entry::O(members) => visitor.visit_map(value::MapDeserializer::new(
                members
                    .iter()
                    .map(|(key, value)| (self.child(*key), self.child(*value))),
            )),
            Entry::V(_) => unreachable!("validated scalar entry"),
        }
    }
    fn deserialize_option<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        if matches!(self.entry(), Entry::V(Value::Null)) {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        match self.entry() {
            Entry::V(Value::String(tag)) => {
                value::BorrowedStrDeserializer::<value::Error>::new(tag)
                    .deserialize_enum(name, variants, visitor)
            }
            Entry::O(members) => value::MapDeserializer::new(
                members
                    .iter()
                    .map(|(key, value)| (self.child(*key), self.child(*value))),
            )
            .deserialize_enum(name, variants, visitor),
            _ => Err(de::Error::custom("invalid enum representation")),
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_packages_preserve_typed_values_without_expanding_json() {
        let mut source: nir_format::Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        for index in 0..1000 {
            source.scenes.insert(
                format!("duplicate_{index}"),
                source.scenes["station"].clone(),
            );
        }
        let (bytes, encoded) = encode(&source).unwrap();
        assert!(encoded);
        assert!(bytes.len() < serde_json::to_vec(&source).unwrap().len() / 2);
        let restored: nir_format::Program =
            decode(super::super::parse(&bytes, "pool").unwrap(), "pool").unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(source).unwrap()
        );
    }

    #[test]
    fn pool_rejects_cycles_duplicate_keys_unused_entries_and_expansion_bombs() {
        let invalid = [
            serde_json::json!({"package_encoding":ENCODING,"root":0,"pool":[{"a":[0]}]}),
            serde_json::json!({"package_encoding":ENCODING,"root":1,"pool":[{"v":"key"},{"o":[[0,0],[0,0]]}]}),
            serde_json::json!({"package_encoding":ENCODING,"root":1,"pool":[{"v":0},{"v":1}]}),
            serde_json::json!({"package_encoding":ENCODING,"root":1,"pool":[{"v":1},{"o":[[0,0]]}]}),
            serde_json::json!({"package_encoding":ENCODING,"root":0,"pool":[{"v":{}}]}),
        ];
        for value in invalid {
            assert!(decode::<Value>(value, "invalid").is_err());
        }
        let mut pool = vec![Entry::V(Value::String("x".repeat(1024)))];
        for index in 0..17 {
            pool.push(Entry::A(vec![index; 2]));
        }
        let package = Package {
            package_encoding: ENCODING.into(),
            root: 17,
            pool,
        };
        assert!(check(&package, "bomb").is_err());
    }

    #[test]
    fn large_encoding_tables_keep_ordinary_content_arrays_bounded() {
        let source: BTreeMap<_, _> = (0..40_000)
            .map(|index| {
                (
                    format!("item_{index:05}"),
                    serde_json::json!({"number":index,"payload":"x".repeat(128)}),
                )
            })
            .collect();
        let (bytes, encoded) = encode(&source).unwrap();
        assert!(encoded);
        let envelope: Package = serde_json::from_slice(&bytes).unwrap();
        assert!(envelope.pool.len() > 100_000);
        assert!(crate::parse::<Value>(&bytes, "ordinary").is_err());
        let restored: BTreeMap<String, Value> = decode_bytes(&bytes, "pool").unwrap();
        assert_eq!(source, restored);
        let mut root: nir_format::RuntimeProgram = serde_json::from_value(serde_json::json!({
            "format":2,"game_id":"pool","revision":"1","entry":"main",
            "stage":{"width":1280,"height":720},"function_index":{},
            "locale_config":{"default_ui":"en","default_text":"en","ui":{},"text":{}},
            "default_locale":"en","requires":[CAPABILITY]
        }))
        .unwrap();
        let restored: BTreeMap<String, Value> =
            crate::parse_package(&root, &bytes, "pool").unwrap();
        assert_eq!(source, restored);
        root.requires.clear();
        assert_eq!(
            crate::parse_package::<Value>(&root, &bytes, "pool")
                .unwrap_err()
                .code,
            "E_CAPABILITY"
        );
        let ordinary = serde_json::to_vec(&vec![0; 100_001]).unwrap();
        assert!(crate::parse::<Value>(&ordinary, "ordinary").is_err());
        let too_many = serde_json::json!({"package_encoding":ENCODING,"root":1,"pool":[{"v":0},{"a":vec![0;100_001]}]});
        assert!(decode_bytes::<Value>(&serde_json::to_vec(&too_many).unwrap(), "invalid").is_err());
        for invalid in [
            br#"{"package_encoding":"interned_json_v1","root":0,"root":0,"pool":[{"v":1}]}"#
                .as_slice(),
            br#"{"package_encoding":"interned_json_v1","root":0,"pool":[{"v":1,"v":2}]}"#
                .as_slice(),
            br#"{"package_encoding":"interned_json_v1","root":0,"pool":[{"v":{"key":1,"key":2}}]}"#
                .as_slice(),
        ] {
            assert!(decode_bytes::<Value>(invalid, "invalid").is_err());
        }
    }
}
