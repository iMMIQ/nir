use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub options: Vec<ChoiceOption>,
    #[serde(default)]
    pub timeout_us: Option<Micros>,
    #[serde(default)]
    pub default: Option<String>,
}
impl Choice {
    pub fn uses_float80(&self) -> bool {
        self.options.iter().any(|o| {
            o.value.as_ref().is_some_and(|v| v.ty() == ValueType::F80)
                || o.visible.iter().chain(&o.enabled).any(Expr::uses_float80)
        })
    }
}
impl Function {
    pub fn uses_float80(&self) -> bool {
        self.params
            .values()
            .chain(self.locals.values())
            .chain(self.returns.iter())
            .any(|ty| *ty == ValueType::F80)
            || self.blocks.values().any(|b| {
                b.ops.iter().any(|op| match &op.operation {
                    Operation::Assign { value, .. }
                    | Operation::ProfileValueAssign { value, .. } => value.uses_float80(),
                    _ => false,
                }) || match &b.terminator {
                    Terminator::Branch { condition, .. } => condition.uses_float80(),
                    Terminator::Switch { value, .. } => value.uses_float80(),
                    Terminator::Call { args, .. } => args.values().any(Expr::uses_float80),
                    Terminator::Return { value } => value.as_ref().is_some_and(Expr::uses_float80),
                    _ => false,
                }
            })
    }
}
impl Program {
    pub fn cue_media_assets(&self, id: &str) -> BTreeSet<String> {
        self.cues
            .get(id)
            .map(|cue| {
                cue.media_assets(
                    &self.theme,
                    |id| self.scenes.get(id).map(Vec::as_slice),
                    |id| self.texts.get(id),
                )
            })
            .unwrap_or_default()
    }
    pub fn uses_float80(&self) -> bool {
        self.variables.values().any(|v| v.ty() == ValueType::F80)
            || self.functions.values().any(Function::uses_float80)
            || self.choices.values().any(Choice::uses_float80)
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub visible: Option<Expr>,
    #[serde(default)]
    pub enabled: Option<Expr>,
    /// Typed result committed by the VM when an Interact declares `result`.
    /// Every option must carry one of the target variable's type.
    #[serde(default)]
    pub value: Option<Value>,
    /// Authored stage coordinates, sharing the stage's letterbox transform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ChoiceImage>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceImage {
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_asset: Option<String>,
    pub rect: [f32; 4],
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextContract {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub meaning_revision: u32,
    pub contract_digest: String,
    #[serde(default)]
    pub gates: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pauses: Vec<TextPauseContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<TextImageContract>,
    #[serde(default)]
    pub params: BTreeMap<String, ValueType>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextPauseContract {
    pub id: String,
    pub timeout_us: Option<Micros>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextImageContract {
    pub id: String,
    pub asset: String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InlineImageAlign {
    Top,
    Center,
    Bottom,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InlineImage {
    pub asset: String,
    pub width: u32,
    pub height: u32,
    pub align: InlineImageAlign,
    #[serde(default)]
    pub margins: [u32; 4],
}
impl InlineImage {
    pub fn valid(&self) -> bool {
        !self.asset.is_empty()
            && self.asset.len() <= 1024
            && (1..=1024).contains(&self.width)
            && (1..=1024).contains(&self.height)
            && self.margins.iter().all(|n| *n <= 512)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InlineImagePlacement {
    pub offset: u32,
    pub image: InlineImage,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDoc {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub spans: Vec<Span>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Span {
    Image {
        id: String,
        image: InlineImage,
    },
    Ruby {
        id: String,
        text: String,
        reading: String,
    },
    Text {
        id: String,
        text: String,
        #[serde(default)]
        emphasis: bool,
    },
    Break {
        id: String,
    },
    Param {
        id: String,
        name: String,
    },
    Gate {
        id: String,
    },
    /// Wait for an advance inside a paragraph, preserving already revealed
    /// text and playing voices. An optional timeout resumes automatically.
    Pause {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_us: Option<Micros>,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub kind: AssetKind,
    pub object: String,
    pub bytes: u64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub duration_us: Micros,
    #[serde(default)]
    pub decoded_bytes: u64,
}
impl Asset {
    /// Conservative stereo f32 payload at a platform decoder's sample rate.
    /// Authored duration is rounded down to microseconds; reserve one extra
    /// output frame for that loss of precision and resampling rounding. The
    /// source payload remains a floor, and impossible costs saturate instead
    /// of wrapping into an apparently small admission request.
    pub fn resampled_pcm_budget(&self, sample_rate: u32) -> u64 {
        let frames = (self.duration_us.0 as u128 * sample_rate as u128).div_ceil(1_000_000);
        let bytes = ((frames + 1) * 2 * 4).min(u64::MAX as u128) as u64;
        self.decoded_bytes.max(bytes)
    }
}

/// Canonical semantic text contract identity. Source copy edits do not change it.
pub fn text_contract_digest(c: &TextContract) -> String {
    use sha2::{Digest, Sha256};
    let legacy = (
        1u32,
        c.contract_revision,
        c.meaning_revision,
        &c.params,
        &c.gates,
    );
    // Existing contracts keep their identity. Authored pauses are semantic:
    // removing one or changing its deadline requires contract review.
    let bytes = if !c.images.is_empty() {
        serde_json::to_vec(&(legacy, &c.pauses, &c.images))
    } else if c.pauses.is_empty() {
        serde_json::to_vec(&legacy)
    } else {
        serde_json::to_vec(&(legacy, &c.pauses))
    }
    .expect("text contract serialization");
    format!("{:x}", Sha256::digest(bytes))
}
/// Shape rules shared by author checks and the runtime loader.
pub fn validate_text_spans(id: &str, c: &TextContract, spans: &[Span]) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut gates = Vec::new();
    let mut pauses = Vec::new();
    let mut images = Vec::new();
    let mut params = BTreeSet::new();
    for span in spans {
        let sid = match span {
            Span::Image { id, image } => {
                if !image.valid() || images.len() >= 256 {
                    return Err(Diagnostic::new("E_TEXT_IMAGE", id, "invalid inline image"));
                }
                images.push(TextImageContract {
                    id: id.clone(),
                    asset: image.asset.clone(),
                });
                id
            }
            Span::Ruby { id, text, reading } => {
                if text.is_empty()
                    || text.len() > 4096
                    || reading.is_empty()
                    || reading.len() > 4096
                {
                    return Err(Diagnostic::new(
                        "E_TEXT_RUBY",
                        id,
                        "empty or excessive annotation",
                    ));
                }
                id
            }
            Span::Text { id, text, .. } => {
                if text.len() > 128 * 1024 {
                    return Err(Diagnostic::new("E_LIMIT", id, "text too long"));
                }
                id
            }
            Span::Break { id } => id,
            Span::Pause { id, timeout_us } => {
                if timeout_us.is_some_and(|v| v.0 > 60_000_000) {
                    return Err(Diagnostic::new(
                        "E_LIMIT",
                        id,
                        "text pause exceeds 60 seconds",
                    ));
                }
                pauses.push(TextPauseContract {
                    id: id.clone(),
                    timeout_us: *timeout_us,
                });
                id
            }
            Span::Gate { id } => {
                gates.push(id.clone());
                id
            }
            Span::Param { id, name } => {
                if !c.params.contains_key(name) {
                    return Err(Diagnostic::new("E_TEXT_PARAM", id, name));
                }
                params.insert(name.clone());
                id
            }
        };
        if sid.is_empty() || !ids.insert(sid) {
            return Err(Diagnostic::new(
                "E_DUPLICATE",
                id,
                "empty or duplicate span identity",
            ));
        }
    }
    if gates != c.gates {
        return Err(Diagnostic::new("E_GATE", id, "gate order/count mismatch"));
    }
    if pauses != c.pauses {
        return Err(Diagnostic::new(
            "E_TEXT_PAUSE",
            id,
            "pause order/deadline mismatch",
        ));
    }
    if images != c.images {
        return Err(Diagnostic::new(
            "E_TEXT_IMAGE",
            id,
            "image identity/order mismatch",
        ));
    }
    if params != c.params.keys().cloned().collect() {
        return Err(Diagnostic::new(
            "E_TEXT_PARAM",
            id,
            "parameter coverage mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod audio_budget_tests {
    use super::*;
    fn asset(duration: u64, source_bytes: u64) -> Asset {
        Asset {
            kind: AssetKind::Audio,
            object: String::new(),
            bytes: 0,
            width: 0,
            height: 0,
            duration_us: Micros(duration),
            decoded_bytes: source_bytes,
        }
    }
    #[test]
    fn context_rate_stereo_budget_includes_resampling_and_rounding() {
        assert_eq!(
            asset(8_000_000, 768_000).resampled_pcm_budget(48_000),
            3_072_008
        );
        assert_eq!(asset(1, 4).resampled_pcm_budget(48_000), 16);
        assert_eq!(asset(22, 4).resampled_pcm_budget(48_000), 24);
    }
    #[test]
    fn source_payload_remains_a_floor_and_overflow_cannot_wrap() {
        assert_eq!(
            asset(1_000_000, 768_000).resampled_pcm_budget(24_000),
            768_000
        );
        assert_eq!(asset(u64::MAX, 4).resampled_pcm_budget(u32::MAX), u64::MAX);
    }
}
