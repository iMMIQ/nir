//! Android binary XML (AXML) encoder and decoder.
//!
//! The encoder produces the subset of the format emitted by aapt2 for
//! manifests: a UTF-16 string pool, a resource map carrying framework
//! attribute IDs, one `android` namespace declaration wrapping the document,
//! and start/end element chunks with typed attributes. The decoder is a
//! minimal parser used to round-trip in tests and by the APK verifier.
//!
//! Every `android:*` attribute name must have a verified framework resource
//! ID (see [`FRAMEWORK_ATTRIBUTES`], cross-checked against aapt2 output for
//! platform 34); unknown names are rejected rather than guessed.
#![forbid(unsafe_code)]

use anyhow::{bail, ensure, Result};
use std::collections::{HashMap, HashSet};

pub const ANDROID_PREFIX: &str = "android";
pub const ANDROID_URI: &str = "http://schemas.android.com/apk/res/android";

/// Chunk types (all little-endian `u16`).
const CHUNK_XML_FILE: u16 = 0x0003;
const CHUNK_STRING_POOL: u16 = 0x0001;
const CHUNK_RESOURCE_MAP: u16 = 0x0180;
const CHUNK_START_NAMESPACE: u16 = 0x0100;
const CHUNK_END_NAMESPACE: u16 = 0x0101;
const CHUNK_START_ELEMENT: u16 = 0x0102;
const CHUNK_END_ELEMENT: u16 = 0x0103;

/// Typed-value data types.
const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
const TYPE_INT_BOOLEAN: u8 = 0x12;

const NO_INDEX: u32 = 0xffff_ffff;

/// Framework attribute resource IDs for every `android:*` attribute this
/// crate emits. Verified against `aapt2 link -I android.jar (platform 34)`
/// output; new entries must come with the same verification.
const FRAMEWORK_ATTRIBUTES: &[(&str, u32)] = &[
    ("theme", 0x0101_0000),
    ("label", 0x0101_0001),
    ("name", 0x0101_0003),
    ("hasCode", 0x0101_000c),
    ("exported", 0x0101_0010),
    ("launchMode", 0x0101_001d),
    ("configChanges", 0x0101_001f),
    ("value", 0x0101_0024),
    ("minSdkVersion", 0x0101_020c),
    ("versionCode", 0x0101_021b),
    ("versionName", 0x0101_021c),
    ("targetSdkVersion", 0x0101_0270),
    ("extractNativeLibs", 0x0101_04ea),
];

fn framework_id(name: &str) -> Option<u32> {
    FRAMEWORK_ATTRIBUTES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, id)| *id)
}

/// Attribute namespace: the `android` XML namespace or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Namespace {
    None,
    Android,
}

/// A typed attribute value, mirroring the Res_value encodings aapt2 emits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AttrValue {
    /// String pool reference (`rawValue` and `data` both set).
    String(String),
    /// Decimal integer.
    IntDec(u32),
    /// Hexadecimal integer (aapt2 uses this for flag sets).
    IntHex(u32),
    /// Boolean; encoded as 0xffffffff / 0.
    Bool(bool),
    /// Resource reference such as `@android:style/...`.
    Reference(u32),
}

/// An attribute on an element.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attr {
    pub ns: Namespace,
    pub name: String,
    pub value: AttrValue,
}

impl Attr {
    pub fn android(name: &str, value: AttrValue) -> Self {
        Self {
            ns: Namespace::Android,
            name: name.to_owned(),
            value,
        }
    }

    pub fn plain(name: &str, value: AttrValue) -> Self {
        Self {
            ns: Namespace::None,
            name: name.to_owned(),
            value,
        }
    }
}

/// An element with attributes and children.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }
}

fn chunk_header(out: &mut Vec<u8>, chunk_type: u16, header_size: u16, size: u32) {
    out.extend_from_slice(&chunk_type.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
}

/// Encodes a document into binary XML. Attributes are ordered like aapt2:
/// `android:` attributes first sorted by resource ID, then unnamespaced
/// attributes sorted by name. The string pool lists attribute names first
/// (by resource ID) then all other strings (sorted, deduplicated against the
/// attribute names), matching aapt2's pool layout so byte-level diffs are
/// meaningful.
pub fn encode(root: &Element) -> Result<Vec<u8>> {
    let mut android_names: Vec<&str> = Vec::new();
    collect_android_attr_names(root, &mut android_names)?;
    android_names.sort_unstable_by_key(|name| framework_id(name).unwrap());
    android_names.dedup();

    let mut other_strings: Vec<String> = Vec::new();
    collect_other_strings(root, &mut other_strings);
    other_strings.extend([ANDROID_PREFIX.to_owned(), ANDROID_URI.to_owned()]);
    // aapt2 sorts the pool by UTF-16 code unit.
    other_strings.sort_unstable_by_key(|value| value.encode_utf16().collect::<Vec<u16>>());
    other_strings.dedup();
    // The pool is shared by attribute names and value strings: a value equal
    // to an attribute name (a game titled "theme", say) must reuse the
    // attribute's entry — the only range the resource map annotates —
    // instead of shadowing it with a later index the map cannot cover.
    let attribute_names: HashSet<&str> = android_names.iter().copied().collect();
    other_strings.retain(|value| !attribute_names.contains(value.as_str()));

    let pool: Vec<String> = android_names
        .iter()
        .map(|name| (*name).to_owned())
        .chain(other_strings)
        .collect();
    let indices: HashMap<&str, u32> = pool
        .iter()
        .enumerate()
        .map(|(index, value)| (value.as_str(), index as u32))
        .collect();
    let index = |value: &str| -> u32 { indices[value] };

    let mut string_data = Vec::new();
    let mut offsets = Vec::with_capacity(pool.len());
    for value in &pool {
        offsets.push(string_data.len() as u32);
        let units: Vec<u16> = value.encode_utf16().collect();
        ensure!(
            units.len() <= u32::MAX as usize,
            "E_AXML_STRING: string too long"
        );
        write_pool_length(&mut string_data, units.len() as u32);
        for unit in &units {
            string_data.extend_from_slice(&unit.to_le_bytes());
        }
        string_data.extend_from_slice(&0u16.to_le_bytes());
        while string_data.len() % 4 != 0 {
            string_data.extend_from_slice(&0u16.to_le_bytes());
        }
    }

    let pool_size = 28 + 4 * pool.len() as u32 + string_data.len() as u32;
    let mut pool_chunk = Vec::with_capacity(pool_size as usize);
    chunk_header(&mut pool_chunk, CHUNK_STRING_POOL, 28, pool_size);
    pool_chunk.extend_from_slice(&(pool.len() as u32).to_le_bytes());
    pool_chunk.extend_from_slice(&0u32.to_le_bytes()); // style count
    pool_chunk.extend_from_slice(&0u32.to_le_bytes()); // flags: UTF-16
    pool_chunk.extend_from_slice(&((28 + 4 * pool.len()) as u32).to_le_bytes()); // strings start
    pool_chunk.extend_from_slice(&0u32.to_le_bytes()); // styles start
    for offset in &offsets {
        pool_chunk.extend_from_slice(&offset.to_le_bytes());
    }
    pool_chunk.extend_from_slice(&string_data);

    let map_size = 8 + 4 * android_names.len() as u32;
    let mut map_chunk = Vec::with_capacity(map_size as usize);
    chunk_header(&mut map_chunk, CHUNK_RESOURCE_MAP, 8, map_size);
    for name in &android_names {
        map_chunk.extend_from_slice(&framework_id(name).unwrap().to_le_bytes());
    }

    let mut body = Vec::new();
    body.extend_from_slice(&pool_chunk);
    body.extend_from_slice(&map_chunk);

    let prefix = index(ANDROID_PREFIX);
    let uri = index(ANDROID_URI);
    namespace_chunk(&mut body, CHUNK_START_NAMESPACE, prefix, uri);
    encode_element(&mut body, root, &index)?;
    namespace_chunk(&mut body, CHUNK_END_NAMESPACE, prefix, uri);

    let mut out = Vec::with_capacity(8 + body.len());
    chunk_header(&mut out, CHUNK_XML_FILE, 8, (8 + body.len()) as u32);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Writes the pool string length, using the high-bit escape form above
/// 0x7fff characters.
fn write_pool_length(out: &mut Vec<u8>, length: u32) {
    if length <= 0x7fff {
        out.extend_from_slice(&(length as u16).to_le_bytes());
    } else {
        out.extend_from_slice(&((length >> 16) as u16 | 0x8000).to_le_bytes());
        out.extend_from_slice(&((length & 0xffff) as u16).to_le_bytes());
    }
}

fn namespace_chunk(out: &mut Vec<u8>, chunk_type: u16, prefix: u32, uri: u32) {
    chunk_header(out, chunk_type, 16, 24);
    out.extend_from_slice(&1u32.to_le_bytes()); // line number
    out.extend_from_slice(&NO_INDEX.to_le_bytes()); // comment
    out.extend_from_slice(&prefix.to_le_bytes());
    out.extend_from_slice(&uri.to_le_bytes());
}

fn encode_element(out: &mut Vec<u8>, element: &Element, index: &dyn Fn(&str) -> u32) -> Result<()> {
    let mut attrs = element.attrs.clone();
    attrs.sort_by_key(sort_key);
    let mut content = Vec::new();
    content.extend_from_slice(&1u32.to_le_bytes()); // line number
    content.extend_from_slice(&NO_INDEX.to_le_bytes()); // comment
    content.extend_from_slice(&NO_INDEX.to_le_bytes()); // element namespace
    content.extend_from_slice(&index(&element.name).to_le_bytes());
    content.extend_from_slice(&(20u16).to_le_bytes()); // attribute start
    content.extend_from_slice(&(20u16).to_le_bytes()); // attribute size
    content.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
    content.extend_from_slice(&0u16.to_le_bytes()); // id index
    content.extend_from_slice(&0u16.to_le_bytes()); // class index
    content.extend_from_slice(&0u16.to_le_bytes()); // style index
    for attr in &attrs {
        content.extend_from_slice(&match attr.ns {
            Namespace::Android => index(ANDROID_URI).to_le_bytes(),
            Namespace::None => NO_INDEX.to_le_bytes(),
        });
        content.extend_from_slice(&index(&attr.name).to_le_bytes());
        match &attr.value {
            AttrValue::String(value) => {
                let position = index(value);
                content.extend_from_slice(&position.to_le_bytes()); // rawValue
                content.extend_from_slice(&8u16.to_le_bytes()); // typed value size
                content.push(0); // res0
                content.push(TYPE_STRING);
                content.extend_from_slice(&position.to_le_bytes());
            }
            typed => {
                content.extend_from_slice(&NO_INDEX.to_le_bytes()); // rawValue
                content.extend_from_slice(&8u16.to_le_bytes());
                content.push(0);
                let (data_type, data) = match typed {
                    AttrValue::IntDec(v) => (TYPE_INT_DEC, *v),
                    AttrValue::IntHex(v) => (TYPE_INT_HEX, *v),
                    AttrValue::Bool(v) => (TYPE_INT_BOOLEAN, u32::from(*v).wrapping_neg()),
                    AttrValue::Reference(v) => (TYPE_REFERENCE, *v),
                    AttrValue::String(_) => unreachable!(),
                };
                content.push(data_type);
                content.extend_from_slice(&data.to_le_bytes());
            }
        }
    }
    let size = 16 + 20 + 20 * attrs.len();
    let mut chunk = Vec::with_capacity(size);
    chunk_header(&mut chunk, CHUNK_START_ELEMENT, 16, size as u32);
    chunk.extend_from_slice(&content);
    out.extend_from_slice(&chunk);
    for child in &element.children {
        encode_element(out, child, index)?;
    }
    let mut end = Vec::with_capacity(24);
    chunk_header(&mut end, CHUNK_END_ELEMENT, 16, 24);
    end.extend_from_slice(&1u32.to_le_bytes()); // line number
    end.extend_from_slice(&NO_INDEX.to_le_bytes()); // comment
    end.extend_from_slice(&NO_INDEX.to_le_bytes());
    end.extend_from_slice(&index(&element.name).to_le_bytes());
    out.extend_from_slice(&end);
    Ok(())
}

/// aapt2 orders android-namespaced attributes by resource ID before
/// unnamespaced attributes sorted by name.
fn sort_key(attr: &Attr) -> (u8, u32, String) {
    match attr.ns {
        Namespace::Android => (0, framework_id(&attr.name).unwrap_or(0), String::new()),
        Namespace::None => (1, 0, attr.name.clone()),
    }
}

fn collect_android_attr_names<'a>(element: &'a Element, names: &mut Vec<&'a str>) -> Result<()> {
    for attr in &element.attrs {
        if attr.ns == Namespace::Android {
            ensure!(
                framework_id(&attr.name).is_some(),
                "E_AXML_ATTR: no verified framework ID for android:{}",
                attr.name
            );
            names.push(&attr.name);
        }
    }
    for child in &element.children {
        collect_android_attr_names(child, names)?;
    }
    Ok(())
}

fn collect_other_strings(element: &Element, strings: &mut Vec<String>) {
    strings.push(element.name.clone());
    for attr in &element.attrs {
        if attr.ns == Namespace::None {
            strings.push(attr.name.clone());
        }
        if let AttrValue::String(value) = &attr.value {
            strings.push(value.clone());
        }
    }
    for child in &element.children {
        collect_other_strings(child, strings);
    }
}

/// A decoded attribute value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DecodedValue {
    String(String),
    IntDec(u32),
    IntHex(u32),
    Bool(bool),
    Reference(u32),
    Other(u8, u32),
}

/// A decoded attribute.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodedAttr {
    pub namespace: Option<String>,
    pub name: String,
    /// Framework resource ID from the resource map, when the attribute name
    /// is covered by the map.
    pub resource_id: Option<u32>,
    pub value: DecodedValue,
}

/// A decoded element.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodedElement {
    pub name: String,
    pub attrs: Vec<DecodedAttr>,
    pub children: Vec<DecodedElement>,
}

/// A decoded binary XML document.
#[derive(Clone, Debug)]
pub struct DecodedXml {
    pub strings: Vec<String>,
    pub resource_map: Vec<u32>,
    pub root: DecodedElement,
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: truncated u16"))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: truncated u32"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn string_at(strings: &[String], index: u32) -> Result<String> {
    strings
        .get(index as usize)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: string index out of range"))
}

/// Parses binary XML into a semantic tree. Namespace chunks and line numbers
/// are validated for structure but not preserved.
pub fn decode(data: &[u8]) -> Result<DecodedXml> {
    ensure!(
        u16_at(data, 0)? == CHUNK_XML_FILE,
        "E_AXML_DECODE: not a binary XML file"
    );
    let file_size = u32_at(data, 4)? as usize;
    ensure!(file_size == data.len(), "E_AXML_DECODE: file size mismatch");
    let mut strings: Option<Vec<String>> = None;
    let mut resource_map = Vec::new();
    let mut stack: Vec<DecodedElement> = Vec::new();
    let mut root = None;
    let mut pos = 8;
    while pos < data.len() {
        let chunk_type = u16_at(data, pos)?;
        let header_size = u16_at(data, pos + 2)? as usize;
        let size = u32_at(data, pos + 4)? as usize;
        ensure!(
            size >= header_size && header_size >= 8 && pos + size <= data.len(),
            "E_AXML_DECODE: chunk bounds"
        );
        match chunk_type {
            CHUNK_STRING_POOL => {
                ensure!(strings.is_none(), "E_AXML_DECODE: duplicate string pool");
                strings = Some(decode_string_pool(data, pos, header_size, size)?);
            }
            CHUNK_RESOURCE_MAP => {
                ensure!(
                    (size - header_size).is_multiple_of(4),
                    "E_AXML_DECODE: bad resource map"
                );
                for offset in (pos + header_size..pos + size).step_by(4) {
                    resource_map.push(u32_at(data, offset)?);
                }
            }
            CHUNK_START_NAMESPACE | CHUNK_END_NAMESPACE => {
                ensure!(size >= 24, "E_AXML_DECODE: bad namespace chunk");
            }
            CHUNK_START_ELEMENT => {
                let element =
                    decode_element(data, pos, strings.as_deref().unwrap_or(&[]), &resource_map)?;
                stack.push(element);
            }
            CHUNK_END_ELEMENT => {
                ensure!(size >= 24, "E_AXML_DECODE: bad end element chunk");
                let element = stack
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: unbalanced end element"))?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(element);
                } else {
                    ensure!(root.is_none(), "E_AXML_DECODE: multiple roots");
                    root = Some(element);
                }
            }
            other => bail!("E_AXML_DECODE: unsupported chunk {other:#06x}"),
        }
        pos += size;
    }
    ensure!(stack.is_empty(), "E_AXML_DECODE: unterminated elements");
    let root = root.ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: no root element"))?;
    Ok(DecodedXml {
        strings: strings.unwrap_or_default(),
        resource_map,
        root,
    })
}

fn decode_string_pool(
    data: &[u8],
    pos: usize,
    header_size: usize,
    size: usize,
) -> Result<Vec<String>> {
    ensure!(header_size >= 28, "E_AXML_DECODE: bad string pool header");
    let count = u32_at(data, pos + 8)? as usize;
    let flags = u32_at(data, pos + 16)?;
    let strings_start = u32_at(data, pos + 20)? as usize;
    ensure!(flags & 0x100 == 0, "E_AXML_DECODE: UTF-8 pools unsupported");
    ensure!(
        28 + 4 * count <= size,
        "E_AXML_DECODE: string offsets exceed chunk"
    );
    let mut strings = Vec::with_capacity(count);
    for index in 0..count {
        let offset = u32_at(data, pos + 28 + 4 * index)? as usize;
        let start = pos + strings_start + offset;
        let length = pool_length(data, start)?;
        let raw = data
            .get(start + length.1..start + length.1 + length.0 * 2)
            .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: string exceeds pool"))?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes(*chunk))
            .collect();
        strings.push(
            String::from_utf16(&units).map_err(|_| anyhow::anyhow!("E_AXML_DECODE: bad UTF-16"))?,
        );
    }
    Ok(strings)
}

/// Reads a pool string length, returning `(units, header_size)`.
fn pool_length(data: &[u8], start: usize) -> Result<(usize, usize)> {
    let first = u16_at(data, start)?;
    if first & 0x8000 == 0 {
        return Ok((first as usize, 2));
    }
    let second = u16_at(data, start + 2)?;
    Ok((((first & 0x7fff) as usize) << 16 | second as usize, 4))
}

fn decode_element(
    data: &[u8],
    pos: usize,
    strings: &[String],
    resource_map: &[u32],
) -> Result<DecodedElement> {
    let name = string_at(strings, u32_at(data, pos + 20)?)?;
    let attribute_start = u16_at(data, pos + 24)? as usize;
    let attribute_size = u16_at(data, pos + 26)? as usize;
    let count = u16_at(data, pos + 28)? as usize;
    ensure!(
        attribute_size >= 20 && attribute_start >= 20,
        "E_AXML_DECODE: bad attribute layout"
    );
    ensure!(
        16 + attribute_start + attribute_size * count <= u32_at(data, pos + 4)? as usize,
        "E_AXML_DECODE: attributes exceed chunk"
    );
    let mut attrs = Vec::with_capacity(count);
    for index in 0..count {
        let offset = pos + 16 + attribute_start + attribute_size * index;
        let ns = u32_at(data, offset)?;
        let name_index = u32_at(data, offset + 4)?;
        let data_type = u8_at(data, offset + 15)?;
        let value = u32_at(data, offset + 16)?;
        let value = match data_type {
            TYPE_STRING => DecodedValue::String(string_at(strings, value)?),
            TYPE_INT_DEC => DecodedValue::IntDec(value),
            TYPE_INT_HEX => DecodedValue::IntHex(value),
            TYPE_INT_BOOLEAN => DecodedValue::Bool(value != 0),
            TYPE_REFERENCE => DecodedValue::Reference(value),
            other => DecodedValue::Other(other, value),
        };
        attrs.push(DecodedAttr {
            namespace: if ns == NO_INDEX {
                None
            } else {
                Some(string_at(strings, ns)?)
            },
            name: string_at(strings, name_index)?,
            // The resource map covers the leading attribute-name strings.
            resource_id: resource_map.get(name_index as usize).copied(),
            value,
        });
    }
    Ok(DecodedElement {
        name,
        attrs,
        children: Vec::new(),
    })
}

fn u8_at(data: &[u8], offset: usize) -> Result<u8> {
    data.get(offset)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("E_AXML_DECODE: truncated u8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Element {
        let mut action = Element::new("action");
        action.attrs.push(Attr::android(
            "name",
            AttrValue::String("android.intent.action.MAIN".to_owned()),
        ));
        let mut category = Element::new("category");
        category.attrs.push(Attr::android(
            "name",
            AttrValue::String("android.intent.category.LAUNCHER".to_owned()),
        ));
        let mut intent = Element::new("intent-filter");
        intent.children.push(action);
        intent.children.push(category);
        let mut activity = Element::new("activity");
        activity.attrs.push(Attr::android(
            "name",
            AttrValue::String("android.app.NativeActivity".to_owned()),
        ));
        activity
            .attrs
            .push(Attr::android("exported", AttrValue::Bool(true)));
        activity.children.push(intent);
        let mut root = Element::new("manifest");
        root.attrs.push(Attr::plain(
            "package",
            AttrValue::String("one.nir.test".to_owned()),
        ));
        root.attrs
            .push(Attr::android("versionCode", AttrValue::IntDec(3)));
        root.children.push(activity);
        root
    }

    #[test]
    fn round_trip() {
        let document = sample();
        let encoded = encode(&document).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.root.name, "manifest");
        assert_eq!(decoded.root.children.len(), 1);
        let activity = &decoded.root.children[0];
        assert_eq!(activity.name, "activity");
        assert_eq!(activity.attrs[0].name, "name");
        assert_eq!(activity.attrs[0].namespace.as_deref(), Some(ANDROID_URI));
        assert_eq!(
            activity.attrs[0].value,
            DecodedValue::String("android.app.NativeActivity".to_owned())
        );
        assert_eq!(activity.attrs[1].value, DecodedValue::Bool(true));
        assert_eq!(decoded.root.attrs[0].name, "versionCode");
        assert_eq!(decoded.root.attrs[0].value, DecodedValue::IntDec(3));
        assert_eq!(decoded.root.attrs[1].name, "package");
        assert_eq!(decoded.root.attrs[1].namespace, None);
        let main = &activity.children[0].children[0];
        assert_eq!(main.name, "action");
        assert_eq!(
            main.attrs[0].value,
            DecodedValue::String("android.intent.action.MAIN".to_owned())
        );
    }

    #[test]
    fn value_colliding_with_an_attribute_name_shares_its_pool_entry() {
        // Only the leading attribute-name indices carry resource IDs; a value
        // string equal to an attribute name (a game titled "name") must reuse
        // that entry instead of shadowing it, or every occurrence of the
        // attribute loses its framework resource ID.
        let mut document = sample();
        document
            .attrs
            .push(Attr::android("label", AttrValue::String("name".to_owned())));
        let decoded = decode(&encode(&document).unwrap()).unwrap();
        let mut unique = decoded.strings.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            decoded.strings.len(),
            "duplicate pool strings"
        );
        let mut stack = vec![&decoded.root];
        while let Some(element) = stack.pop() {
            for attr in &element.attrs {
                if attr.namespace.is_some() {
                    assert!(
                        attr.resource_id.is_some(),
                        "android:{} lost its resource id",
                        attr.name
                    );
                }
            }
            stack.extend(element.children.iter());
        }
    }

    #[test]
    fn rejects_unknown_android_attribute() {
        let mut document = sample();
        document
            .attrs
            .push(Attr::android("bogus", AttrValue::Bool(true)));
        assert!(encode(&document).is_err());
    }

    #[test]
    fn rejects_truncated_input() {
        let encoded = encode(&sample()).unwrap();
        assert!(decode(&encoded[..encoded.len() - 3]).is_err());
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn chinese_label_survives_utf16_pool() {
        let mut document = sample();
        document.attrs.insert(
            0,
            Attr::android("label", AttrValue::String("星之梦 ～开辟之诗～".to_owned())),
        );
        let encoded = encode(&document).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(
            decoded.root.attrs[0].value,
            DecodedValue::String("星之梦 ～开辟之诗～".to_owned())
        );
    }
}
