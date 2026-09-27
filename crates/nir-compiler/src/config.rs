//! Author configuration is resolved explicitly, with no recursive merge or local overrides.
use crate::project::{json, toml_file};
use crate::{relative, GameManifest};
use anyhow::{bail, Result};
use nir_format::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocaleManifest {
    pub format: u32,
    pub default_ui: String,
    pub default_text: String,
    pub ui: BTreeMap<String, Vec<String>>,
    pub text: BTreeMap<String, Vec<String>>,
}
impl LocaleManifest {
    pub fn resolve(&self) -> Result<LocaleConfig> {
        let valid_ui = |locale: &str| matches!(locale, "zh-Hans" | "en");
        let valid_text = |locale: &str| matches!(locale, "zh-Hans" | "en" | "ja");
        if self.format != 1
            || !valid_ui(&self.default_ui)
            || !valid_text(&self.default_text)
            || self.ui.is_empty()
            || self.text.is_empty()
            || self.ui.keys().any(|locale| !valid_ui(locale))
            || self.text.keys().any(|locale| !valid_text(locale))
        {
            bail!("E_LOCALE_CONFIG: expected format 1; UI supports zh-Hans/en, text supports zh-Hans/en/ja");
        }
        for (surface, plans) in [("ui", &self.ui), ("text", &self.text)] {
            for (locale, fonts) in plans {
                if fonts.is_empty()
                    || fonts.iter().any(|font| font.trim().is_empty())
                    || fonts
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != fonts.len()
                {
                    bail!("E_FONT_PLAN: {surface}.{locale} needs an ordered, nonempty list of unique font asset IDs");
                }
            }
        }
        if !self.ui.contains_key(&self.default_ui) || !self.text.contains_key(&self.default_text) {
            bail!("E_LOCALE_DEFAULT: defaults must name supported locales");
        }
        Ok(LocaleConfig {
            default_ui: self.default_ui.clone(),
            default_text: self.default_text.clone(),
            ui: self
                .ui
                .iter()
                .map(|(l, f)| (l.clone(), LocaleFontPlan::new(f.clone())))
                .collect(),
            text: self
                .text
                .iter()
                .map(|(l, f)| (l.clone(), LocaleFontPlan::new(f.clone())))
                .collect(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeManifest {
    pub format: u32,
    pub id: String,
    pub base: String,
    pub tokens: String,
    #[serde(default)]
    pub slots: ThemeSlots,
    #[serde(default)]
    pub dialogue: DialogueProps,
    #[serde(default)]
    pub choice: ChoiceProps,
    #[serde(default)]
    pub image_menus: BTreeMap<String, ImageMenu>,
    #[serde(default)]
    pub return_to_title: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeTokens {
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub accent: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayerConfig {
    pub format: u32,
    #[serde(default)]
    pub defaults: PlayerDefaults,
}
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedField {
    pub value: Value,
    /// Project-relative file plus JSON Pointer, or the built-in SDK preset.
    pub source: String,
}
pub type ResolvedConfig = BTreeMap<String, ResolvedField>;
fn record(
    out: &mut ResolvedConfig,
    prefix: &str,
    value: &Value,
    explicit: &Value,
    source: &str,
    pointer: &str,
) {
    if let Value::Object(fields) = value {
        for (key, value) in fields {
            record(
                out,
                &format!("{prefix}.{key}"),
                value,
                &explicit[key],
                source,
                &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
            );
        }
    } else {
        out.insert(
            prefix.into(),
            ResolvedField {
                value: value.clone(),
                source: if explicit.is_null() {
                    "builtin:web-standard".into()
                } else {
                    format!("{source}#{pointer}")
                },
            },
        );
    }
}
fn toml_value(path: &Path) -> Result<Value> {
    Ok(serde_json::to_value(toml::from_str::<toml::Value>(
        &fs::read_to_string(path)?,
    )?)?)
}
pub(crate) fn resolve_config(
    root: &Path,
    manifest: &GameManifest,
) -> Result<(Theme, PlayerDefaults, ResolvedConfig)> {
    if manifest.engine.runtime_preset != "web-standard" {
        bail!("E_RUNTIME_PRESET: only web-standard is supported");
    }
    let mut resolved = BTreeMap::new();
    let authored = toml_value(&root.join("game.toml"))?;
    let manifest_value = serde_json::to_value(manifest)?;
    for key in ["game", "engine", "stage"] {
        record(
            &mut resolved,
            key,
            &manifest_value[key],
            &authored[key],
            "game.toml",
            &format!("/{key}"),
        );
    }
    let path = relative(root, root, &manifest.inputs.theme)?;
    let mut theme = Theme::default();
    let tokens_path;
    if path.extension().and_then(|s| s.to_str()) == Some("json") {
        // Legacy color-only projects remain loadable; component contracts require TOML.
        tokens_path = path.clone();
        for (key, value) in [
            ("slots", serde_json::to_value(&theme.slots)?),
            ("dialogue", serde_json::to_value(&theme.dialogue)?),
            ("choice", serde_json::to_value(&theme.choice)?),
        ] {
            record(
                &mut resolved,
                &format!("theme.{key}"),
                &value,
                &Value::Null,
                "",
                "",
            );
        }
    } else {
        let m: ThemeManifest = toml_file(&path)?;
        if m.format != 1 || m.base != "builtin.reader" {
            bail!("E_THEME_CONTRACT: expected format 1 and base builtin.reader");
        }
        if m.id.is_empty()
            || !m
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            bail!("E_THEME_ID: expected a nonempty ASCII identifier");
        }
        let value = serde_json::to_value(&m)?;
        let explicit = toml_value(&path)?;
        let source = crate::project::forward_slashes(path.strip_prefix(root)?);
        for key in [
            "id",
            "base",
            "slots",
            "dialogue",
            "choice",
            "image_menus",
            "return_to_title",
        ] {
            record(
                &mut resolved,
                &format!("theme.{key}"),
                &value[key],
                &explicit[key],
                &source,
                &format!("/{key}"),
            );
        }
        tokens_path = relative(root, path.parent().unwrap(), &m.tokens)?;
        theme.slots = m.slots;
        theme.dialogue = m.dialogue;
        theme.choice = m.choice;
        theme.image_menus = m.image_menus;
        theme.return_to_title = m.return_to_title;
    }
    let tokens: ThemeTokens = json(&tokens_path)?;
    let value = serde_json::to_value(&tokens)?;
    record(
        &mut resolved,
        "theme.tokens",
        &value,
        &value,
        &crate::project::forward_slashes(tokens_path.strip_prefix(root)?),
        "",
    );
    theme.background = tokens.background;
    theme.panel = tokens.panel;
    theme.accent = tokens.accent;
    theme.text = tokens.text;
    theme.muted = tokens.muted;
    let player = if let Some(input) = &manifest.inputs.player {
        let path = relative(root, root, input)?;
        let config: PlayerConfig = toml_file(&path)?;
        if config.format != 1 {
            bail!("E_PLAYER_CONFIG: expected player format 1");
        }
        record(
            &mut resolved,
            "player",
            &serde_json::to_value(&config.defaults)?,
            &toml_value(&path)?["defaults"],
            &crate::project::forward_slashes(path.strip_prefix(root)?),
            "/defaults",
        );
        config.defaults
    } else {
        let defaults = PlayerDefaults::default();
        record(
            &mut resolved,
            "player",
            &serde_json::to_value(&defaults)?,
            &Value::Null,
            "",
            "",
        );
        defaults
    };
    validate_ui_config(&theme, &player)?;
    Ok((theme, player, resolved))
}
