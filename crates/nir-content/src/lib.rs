//! Content identity and bounded parsing. No filesystem, network or device access.
#![forbid(unsafe_code)]
use nir_format::*;
use serde::{
    de::{DeserializeOwned, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    if digest(bytes) != expected {
        return Err(Diagnostic::new(
            "E_DIGEST",
            expected,
            "content identity mismatch",
        ));
    }
    Ok(())
}

/// Install immutable content into an isolated candidate. The caller validates
/// the resulting program before replacing any live execution view.
pub fn install_module_code(p: &mut Program, bytes: &[u8], module: &str) -> Result<()> {
    let index = p
        .modules
        .get(module)
        .ok_or_else(|| Diagnostic::new("E_MODULE", module, "undeclared module"))?;
    verify(bytes, &index.code)?;
    let code: ModuleCode = parse(bytes, module)?;
    if code.format != 1
        || code.module != module
        || code.functions.len() != index.functions.len()
        || code
            .functions
            .iter()
            .any(|(id, f)| index.functions.get(id) != Some(&FunctionSignature::from(f)))
    {
        return Err(Diagnostic::new(
            "E_MODULE_INTERFACE",
            module,
            "module body does not match declared interface",
        ));
    }
    p.functions.extend(code.functions);
    Ok(())
}
pub fn install_module_texts(
    p: &mut Program,
    bytes: &[u8],
    module: &str,
    locale: &str,
) -> Result<()> {
    let index = p
        .modules
        .get(module)
        .ok_or_else(|| Diagnostic::new("E_MODULE", module, "undeclared module"))?;
    let hash = index
        .locales
        .get(locale)
        .ok_or_else(|| Diagnostic::new("E_LOCALE", module, locale))?;
    verify(bytes, hash)?;
    let text: ModuleTexts = parse(bytes, module)?;
    if text.format != 1
        || text.module != module
        || text.locale != locale
        || text.texts.keys().cloned().collect::<BTreeSet<_>>() != index.texts
    {
        return Err(Diagnostic::new(
            "E_MODULE_TEXT",
            module,
            "text bundle does not match declared ownership",
        ));
    }
    p.locales
        .get_mut(locale)
        .ok_or_else(|| Diagnostic::new("E_LOCALE", module, locale))?
        .extend(text.texts);
    Ok(())
}

/// Parse one runtime package after authenticating it against the root's typed
/// identity index. This does shape/ownership checks; `nir-core` still performs
/// semantic validation before making the resulting block resident.
pub fn parse_runtime_object(
    root: &RuntimeProgram,
    key: &ContentKey,
    bytes: &[u8],
) -> Result<RuntimeObject> {
    let requirement = root.content_requirement(key).ok_or_else(|| {
        Diagnostic::new("E_CONTENT_KEY", format!("{key:?}"), "undeclared content")
    })?;
    verify(bytes, &requirement.digest)?;
    match key {
        ContentKey::Static { module } => {
            let package: ModuleStatic = parse(bytes, module)?;
            if package.format != RUNTIME_FORMAT_VERSION || package.module != *module {
                return Err(Diagnostic::new(
                    "E_STATIC",
                    module,
                    "static package identity mismatch",
                ));
            }
            let names_owned = |owners: &BTreeMap<String, String>| -> BTreeSet<String> {
                owners
                    .iter()
                    .filter(|(_, owner)| owner.as_str() == module)
                    .map(|(id, _)| id.clone())
                    .collect()
            };
            if package.scenes.keys().cloned().collect::<BTreeSet<_>>()
                != names_owned(&root.scene_owners)
                || package.cues.keys().cloned().collect::<BTreeSet<_>>()
                    != names_owned(&root.cue_owners)
                || package.choices.keys().cloned().collect::<BTreeSet<_>>()
                    != names_owned(&root.choice_owners)
                || package
                    .text_contracts
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    != names_owned(&root.text_owners)
            {
                return Err(Diagnostic::new(
                    "E_STATIC_INDEX",
                    module,
                    "static package ownership differs from root",
                ));
            }
            Ok(RuntimeObject::Static(package))
        }
        ContentKey::Code { module } => {
            let package: ModuleCode = parse(bytes, module)?;
            let index = root.modules.get(module).unwrap();
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.functions.len() != index.functions.len()
                || package
                    .functions
                    .iter()
                    .any(|(id, f)| index.functions.get(id) != Some(&FunctionSignature::from(f)))
                || package.functions.keys().any(|id| {
                    root.function_index
                        .get(id)
                        .is_none_or(|entry| entry.module != *module)
                })
            {
                return Err(Diagnostic::new(
                    "E_MODULE_INTERFACE",
                    module,
                    "code package differs from root interface",
                ));
            }
            Ok(RuntimeObject::Code(package))
        }
        ContentKey::Text { module, locale } => {
            let package: ModuleTexts = parse(bytes, module)?;
            let index = root.modules.get(module).unwrap();
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.locale != *locale
                || package.texts.keys().cloned().collect::<BTreeSet<_>>() != index.texts
            {
                return Err(Diagnostic::new(
                    "E_MODULE_TEXT",
                    module,
                    "text package differs from root ownership",
                ));
            }
            Ok(RuntimeObject::Text(package))
        }
        ContentKey::Catalog { catalog } => {
            let package: AssetCatalog = parse(bytes, catalog)?;
            let expected: BTreeSet<_> = root
                .assets
                .iter()
                .filter(|(_, index)| index.catalog == *catalog)
                .map(|(id, _)| id.clone())
                .collect();
            if package.format != RUNTIME_FORMAT_VERSION
                || package.catalog != *catalog
                || package.assets.keys().cloned().collect::<BTreeSet<_>>() != expected
                || package.assets.iter().any(|(id, asset)| {
                    root.assets.get(id).is_none_or(|index| {
                        index.kind != asset.kind
                            || index.object != asset.object
                            || index.catalog != *catalog
                    })
                })
            {
                return Err(Diagnostic::new(
                    "E_CATALOG",
                    catalog,
                    "catalog descriptors differ from root index",
                ));
            }
            Ok(RuntimeObject::Catalog(package))
        }
    }
}
/// Check the complete, canonical index table without linking the compiler.
pub fn validate_executable(e: &Executable) -> Result<()> {
    let bad = || {
        Diagnostic::new(
            "E_EXECUTABLE",
            "executable",
            "invalid index, recovery map, cost or recipe",
        )
    };
    let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
    for module in e.program.modules.values() {
        if module
            .functions
            .keys()
            .any(|id| !e.program.functions.contains_key(id))
            && !hash(&module.code)
        {
            return Err(bad());
        }
        for (locale, texts) in &e.program.locales {
            if module.texts.iter().any(|id| !texts.contains_key(id))
                && !module.locales.get(locale).is_some_and(|s| hash(s))
            {
                return Err(bad());
            }
        }
    }
    let expected: usize = e
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .map(|b| b.ops.len() + 1)
        .sum();
    if e.format != FORMAT_VERSION
        || e.addresses.len() != expected
        || e.resume_map.len() != expected
        || e.semantic_cost_map.len() != expected
        || e.activation_recipes.len() != e.program.cues.len()
    {
        return Err(bad());
    }
    let mut i = 0;
    for (fid, f) in &e.program.functions {
        for (bid, b) in &f.blocks {
            for op in 0..=b.ops.len() {
                let a = &e.addresses[i];
                let stable = b
                    .ops
                    .get(op)
                    .map(|o| o.id.as_str())
                    .unwrap_or("@terminator");
                if a.function != *fid
                    || a.block != *bid
                    || a.op != op
                    || a.stable_id != stable
                    || e.resume_map.get(&a.key()) != Some(&(i as u32))
                    || e.semantic_cost_map[i] != 1
                {
                    return Err(bad());
                }
                i += 1;
            }
        }
    }
    for cue in e.program.cues.keys() {
        if e.activation_recipes.get(cue) != Some(&cue_assets(&e.program, cue)) {
            return Err(bad());
        }
    }
    Ok(())
}
struct Strict(serde_json::Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| Strict(v.into()))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(serde_json::Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut v = vec![];
                while let Some(Strict(x)) = a.next_element()? {
                    v.push(x);
                    if v.len() > 100_000 {
                        return Err(serde::de::Error::custom("E_LIMIT: array length"));
                    }
                }
                Ok(Strict(v.into()))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut v = serde_json::Map::new();
                while let Some((k, Strict(x))) = a.next_entry::<String, Strict>()? {
                    if v.insert(k.clone(), x).is_some() {
                        return Err(serde::de::Error::custom(format!("E_DUPLICATE: {k}")));
                    }
                    if v.len() > 100_000 {
                        return Err(serde::de::Error::custom("E_LIMIT: map length"));
                    }
                }
                Ok(Strict(v.into()))
            }
        }
        de.deserialize_any(V)
    }
}
pub fn parse<T: DeserializeOwned>(bytes: &[u8], at: &str) -> Result<T> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Diagnostic::new("E_LIMIT", at, "input exceeds 16 MiB"));
    }
    let v: Strict = serde_json::from_slice(bytes).map_err(|e| {
        let mut d = Diagnostic::new("E_JSON", at, e.to_string()).classified(
            ErrorDomain::Content,
            "parse",
            "json",
            vec![Recovery::FixContent],
        );
        d.details.as_mut().unwrap().source = Some(SourceRef {
            file: at.into(),
            line: e.line(),
            column: e.column(),
            pointer: String::new(),
        });
        d
    })?;
    serde_json::from_value(v.0).map_err(|e| {
        Diagnostic::new("E_SCHEMA", at, e.to_string()).classified(
            ErrorDomain::Content,
            "parse",
            "schema",
            vec![Recovery::FixContent],
        )
    })
}
pub fn validate_release(r: &ReleaseManifest) -> Result<()> {
    if r.format != 1 || !matches!(r.profile.as_str(), "dev" | "release") {
        return Err(Diagnostic::new(
            "E_VERSION",
            "release",
            "unsupported release",
        ));
    }
    for (hash, o) in &r.objects {
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || o.path.starts_with('/')
            || o.path.contains("..")
            || o.path.contains(':')
            || o.path.contains('\\')
            || !o.path.starts_with("objects/")
            || !o.path.contains(hash)
        {
            return Err(Diagnostic::new(
                "E_OBJECT",
                hash,
                "invalid object reference",
            ));
        }
    }
    for id in [
        &r.program,
        &r.engine.js,
        &r.engine.wasm,
        &r.engine.host,
        &r.launch.html,
        &r.launch.bootstrap,
    ]
    .into_iter()
    .chain(r.notices.iter())
    {
        if !r.objects.contains_key(id) {
            return Err(Diagnostic::new("E_OBJECT", id, "release root missing"));
        }
    }
    for (id, mime, ext) in [
        (&r.launch.html, "text/html; charset=utf-8", ".html"),
        (&r.launch.bootstrap, "text/javascript", ".js"),
    ] {
        let object = &r.objects[id];
        if object.media_type != mime || !object.path.ends_with(ext) {
            return Err(Diagnostic::new("E_LAUNCH", id, "invalid launch object"));
        }
    }
    Ok(())
}
pub fn cue_assets(p: &Program, cue: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Some(c) = p.cues.get(cue) {
        for def in &c.effects {
            match &def.effect {
                Effect::StagePresent { scene, .. } => {
                    if let Some(nodes) = p.scenes.get(scene) {
                        set.extend(nodes.iter().filter_map(|n| n.asset.clone()));
                    }
                }
                Effect::Audio { asset, .. } => {
                    set.insert(asset.clone());
                }
                _ => {}
            }
        }
    }
    set.extend(
        p.assets
            .iter()
            .filter(|(_, a)| a.kind == AssetKind::Font)
            .map(|(id, _)| id.clone()),
    );
    set
}
pub fn negotiate(requested: &str, available: &BTreeSet<String>, default: &str) -> String {
    if available.contains(requested) {
        return requested.into();
    }
    let alias = match requested {
        "zh" | "zh-CN" | "zh-SG" => "zh-Hans",
        s if s.starts_with("en-") => "en",
        _ => default,
    };
    if available.contains(alias) {
        alias.into()
    } else {
        default.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_keys_rejected() {
        assert!(parse::<serde_json::Value>(br#"{"a":1,"a":2}"#, "test").is_err());
    }
    #[test]
    fn corrupted_bytes() {
        assert!(verify(b"changed", &digest(b"original")).is_err());
    }
    #[test]
    fn script_not_guessed() {
        let a = BTreeSet::from(["zh-Hans".into(), "en".into()]);
        assert_eq!(negotiate("zh-Hant", &a, "en"), "en");
    }
}
