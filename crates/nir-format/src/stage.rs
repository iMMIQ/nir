use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    X,
    Y,
    Scale,
    Opacity,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default, skip_serializing_if = "tween::offset_is_zero")]
    pub offset: [f32; 2],
    /// Paint-only affine geometry for a leaf sprite. Coordinates remain local
    /// to the node, so ancestor translation/scale and scalar tracks still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite_transform: Option<SpriteTransform>,
    /// Immutable timeline declaration presented by this scene root. A task
    /// holds the clock; snapshots retain only this resource identity and pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_binding: Option<String>,
    /// A continuation recipe keeps a missing animated root missing. Explicit
    /// creation uses false, so a later authored recreation still takes effect.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_existence: bool,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
    #[serde(default = "white", skip_serializing_if = "is_white")]
    pub color: [f32; 4],
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<[f32; 4]>,
    /// A scene recipe, expanded into ordinary frozen sprites at StagePresent.
    /// The slot is global and explicitly captured by authored Assign operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitmap_text: Option<BitmapText>,
    /// Preserve these local pose properties when this node already exists in
    /// the prior scene; session/frame tweens retain their clock and binding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preserve_pose: Vec<Property>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpriteTransform {
    pub origin: [f32; 2],
    pub basis_x: [f32; 2],
    pub basis_y: [f32; 2],
}
impl SpriteTransform {
    pub fn corners(self, width: f32, height: f32) -> [[f32; 2]; 4] {
        [[0., 0.], [width, 0.], [0., height], [width, height]].map(|[x, y]| {
            [
                self.origin[0] + x * self.basis_x[0] + y * self.basis_y[0],
                self.origin[1] + x * self.basis_x[1] + y * self.basis_y[1],
            ]
        })
    }
    pub fn valid(self, width: f32, height: f32) -> bool {
        self.origin
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 8192.)
            && self
                .basis_x
                .iter()
                .chain(self.basis_y.iter())
                .all(|v| v.is_finite() && v.abs() <= 64.)
            && self
                .corners(width, height)
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= 32768.)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BitmapAnchor {
    Start,
    Center,
    End,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BitmapText {
    pub slot: String,
    /// Horizontal atlas with one fixed-size cell per Unicode scalar.
    pub alphabet: String,
    pub cell: [u32; 2],
    pub line_spacing: u32,
    pub align: BitmapAnchor,
    /// For center/end anchors the node coordinate is the reference extent.
    pub x_anchor: BitmapAnchor,
    pub y_anchor: BitmapAnchor,
}
impl BitmapText {
    pub fn valid(&self, variables: &BTreeMap<String, Value>) -> bool {
        let chars: Vec<_> = self.alphabet.chars().collect();
        !chars.is_empty()
            && chars.len() <= 256
            && chars.iter().collect::<BTreeSet<_>>().len() == chars.len()
            && !chars.iter().any(|c| c.is_control())
            && self.cell.iter().all(|n| (1..=8192).contains(n))
            && self.cell[0]
                .checked_mul(chars.len() as u32)
                .is_some_and(|n| n <= 8192)
            && self.line_spacing <= 8192
            && matches!(
                variables.get(&self.slot),
                Some(Value::I32(_) | Value::String(_))
            )
    }
}
pub(super) fn one() -> f32 {
    1.0
}
pub(super) fn is_one(value: &f32) -> bool {
    *value == 1.0
}
pub(super) fn is_white(value: &[f32; 4]) -> bool {
    *value == [1.0; 4]
}
pub(super) fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}
pub(super) fn is_false(value: &bool) -> bool {
    !value
}
pub(super) fn white() -> [f32; 4] {
    [1.; 4]
}
impl Node {
    pub fn get(&self, p: Property) -> f32 {
        match p {
            Property::X => self.x,
            Property::Y => self.y,
            Property::Scale => self.scale,
            Property::Opacity => self.opacity,
        }
    }
    pub fn set(&mut self, p: Property, v: f32) {
        match p {
            Property::X => self.x = v,
            Property::Y => self.y = v,
            Property::Scale => self.scale = v,
            Property::Opacity => self.opacity = v,
        }
    }
}
