use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub width: u32,
    pub height: u32,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    #[serde(default)]
    pub params: BTreeMap<String, ValueType>,
    #[serde(default)]
    pub locals: BTreeMap<String, ValueType>,
    #[serde(default)]
    pub returns: Option<ValueType>,
    pub entry: String,
    pub blocks: BTreeMap<String, Block>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    #[serde(default)]
    pub ops: Vec<Op>,
    pub terminator: Terminator,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    pub id: String,
    pub operation: Operation,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Read an independent, monotonic profile fact as an I32 flag (0 or 1).
    /// Saves freeze the destination slot; subsequent reads use current profile.
    ProfileRead {
        target: String,
        key: String,
    },
    /// Typed profile values survive NewGame and Story snapshot restoration.
    ProfileValueRead {
        target: String,
        key: String,
    },
    ProfileValueAssign {
        target: String,
        key: String,
        value: Expr,
    },
    /// Pause one Story audio bus without stopping/restarting its instances.
    /// Playback and envelope clocks freeze; background/menu pauses compose.
    AudioPause {
        bus: AudioBus,
        paused: bool,
    },
    DialogueVisibility {
        visible: bool,
        /// Styled reveal; absent or zero-duration commits flip instantly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition: Option<StageTransition>,
        #[serde(default, skip_serializing_if = "Micros::is_zero")]
        duration_us: Micros,
    },
    Assign {
        target: String,
        value: Expr,
    },
    Random {
        target: String,
        min: i32,
        max: i32,
    },
    DraftPatch {
        node: String,
        property: Property,
        value: f32,
    },
    TaskControl {
        task: String,
        action: TaskAction,
    },
    MenuAccess {
        enabled: bool,
    },
    DialogueContinue {
        task: String,
    },
    DialogueVoice {
        task: String,
        voice: Option<String>,
        wait: VoiceWaitPolicy,
    },
    ProfileMerge {
        key: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceWaitPolicy {
    /// Freeze remaining audible voice duration at the beginning of the Auto cycle.
    SampledRemaining,
    /// Reading delay starts once the bound voice is no longer running.
    AfterVoice,
    /// Reading delay and bound voice run concurrently; both must complete.
    Parallel,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Cancel,
    Finish,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Terminator {
    Goto {
        target: String,
    },
    Branch {
        condition: Expr,
        yes: String,
        no: String,
    },
    Switch {
        value: Expr,
        cases: BTreeMap<String, String>,
        default: String,
    },
    Call {
        function: String,
        #[serde(default)]
        args: BTreeMap<String, Expr>,
        next: String,
        #[serde(default)]
        result: Option<String>,
    },
    Return {
        #[serde(default)]
        value: Option<Expr>,
    },
    Activate {
        cue: String,
        next: String,
    },
    Await {
        conditions: Vec<WaitCondition>,
        next: String,
        on_cancelled: String,
        on_failed: String,
        /// An explicit user advance may leave this wait; the awaited task
        /// keeps running. Hosts cannot interrupt an ordinary Await.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_advance: Option<String>,
    },
    Interact {
        choice: String,
        branches: BTreeMap<String, String>,
        on_empty: String,
        /// Typed-result mode: the chosen option's declared value is written to
        /// this variable by the VM before the branch. The host never writes.
        #[serde(default)]
        result: Option<String>,
        /// Explicit cancel target; absent means the interaction is modal.
        #[serde(default)]
        on_cancel: Option<String>,
    },
    End {
        outcome: String,
    },
    Fault {
        code: String,
        message: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitCondition {
    pub task: String,
    pub milestone: Milestone,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(
    tag = "type",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Milestone {
    Started,
    Finished,
    Marker(String),
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    pub effects: Vec<EffectDef>,
}
impl Cue {
    /// Media needed to prepare this cue. Callers supply their resident scene
    /// and text views; font selection and cached recipes remain their policy.
    pub fn media_assets<'a>(
        &self,
        theme: &Theme,
        scene: impl Fn(&str) -> Option<&'a [Node]>,
        text: impl Fn(&str) -> Option<&'a TextContract>,
    ) -> BTreeSet<String> {
        let mut assets = BTreeSet::new();
        let mut text_ids = BTreeSet::new();
        for definition in &self.effects {
            let effect = &definition.effect;
            effect.collect_text_ids(&mut text_ids);
            effect.collect_dialogue_style_assets(theme, &mut assets);
            match effect {
                Effect::StagePresent {
                    scene: id,
                    transition,
                    ..
                } => {
                    assets.extend(transition.asset().map(str::to_owned));
                    if let Some(nodes) = scene(id) {
                        assets.extend(effect.stage_image_assets(nodes).cloned());
                    }
                }
                Effect::Audio { asset, .. } => {
                    assets.insert(asset.clone());
                }
                Effect::Sequence { .. } | Effect::ParallelAll { .. } => {
                    effect.collect_audio_assets(&mut assets);
                }
                _ => {}
            }
        }
        for id in text_ids {
            if let Some(contract) = text(&id) {
                assets.extend(contract.images.iter().map(|image| image.asset.clone()));
            }
        }
        assets
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDef {
    pub id: String,
    pub scope: Scope,
    pub effect: Effect,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Frame,
    Scene,
    Interaction,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    /// Suspend authored execution while the player owns a modal page.
    StoryModal {
        target: StoryModalTarget,
    },
    /// Prepared replacement of the dialogue window and its text metrics.
    DialogueStyle {
        style: String,
    },
    /// Prepared portrait/name image replacement without replacing live text.
    DialogueDecoration {
        slot: DialogueDecorationSlot,
        image: Option<DialogueDecorationImage>,
    },
    /// Immutable local keyframes, clocked by this task and bound to a scene root.
    SpriteTimeline {
        timeline: String,
        root: String,
        duration_us: Micros,
        /// Delete this bound root and its descendants on natural completion.
        #[serde(default, skip_serializing_if = "is_false")]
        delete_on_finish: bool,
    },
    /// A finite, captured text-content shake; the dialogue window stays still.
    DialogueShake {
        spec: DialogueShake,
    },
    /// A finite shared wave in local sprite translation, separate from X/Y
    /// movement. Sampling preserves authored geometry and the parent transform.
    SpriteWave {
        nodes: Vec<String>,
        spec: DialogueShake,
    },
    /// Independent finite paint offsets; each node/axis owns a frozen path.
    SpriteShake {
        nodes: Vec<String>,
        mode: SpriteShakeMode,
        spec: DialogueShake,
    },
    StagePresent {
        scene: String,
        /// Keep the concrete image already committed for these ordinary
        /// leaf nodes. Resolved once at commit, before transition capture.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inherit_images: Vec<String>,
        /// Keep the committed local rectangle together with its image. These
        /// IDs must also occur in inherit_images; explicit changes omit them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inherit_image_geometry: Vec<String>,
        /// Commit a reserved message-window flip at the same prepared instant
        /// as the scene; its reveal shares this transition and duration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dialogue_visible: Option<bool>,
        #[serde(default, skip_serializing_if = "StageTransition::is_default")]
        transition: StageTransition,
        #[serde(default)]
        duration_us: Micros,
    },
    /// Stock integer-millisecond X/Y motion. It shares scalar writer identity
    /// and replacement with Clip, while preserving the source curve sampling.
    SourceMotion {
        node: String,
        property: Property,
        to: i32,
        duration_us: Micros,
        curve: SourceMotionCurve,
    },
    Clip {
        node: String,
        property: Property,
        to: f32,
        duration_us: Micros,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        easing: Easing,
        #[serde(default)]
        finish: FinishPolicy,
        #[serde(default)]
        cancel: CancelPolicy,
    },
    Tween {
        target: TweenTarget,
        to: f32,
        duration_us: Micros,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        easing: Easing,
        #[serde(default)]
        finish: FinishPolicy,
        #[serde(default)]
        cancel: CancelPolicy,
    },
    Dialogue {
        text: String,
        #[serde(default)]
        speaker: String,
        reveal_us: Micros,
    },
    Audio {
        asset: String,
        bus: AudioBus,
        #[serde(default = "unit_gain", skip_serializing_if = "is_unit_gain")]
        gain: f32,
        #[serde(default)]
        looped: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loop_region: Option<AudioLoopRegion>,
    },
    /// A finite stop operation; completion is separate from natural playback end.
    AudioStop {
        target: String,
        duration_us: Micros,
    },
    Delay {
        duration_us: Micros,
    },
    /// Children run one after another: the next child starts only after the
    /// previous one completed, capturing the current property values at its
    /// own start. The composition is itself a task; children are tasks with
    /// the composition's scope.
    Sequence {
        children: Vec<EffectDef>,
    },
    /// Children all start together; the composition finishes when every child
    /// finished, and fails or cancels as soon as any child does.
    ParallelAll {
        children: Vec<EffectDef>,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpriteShakeMode {
    Wave,
    Bound,
    Quake,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoryModalTarget {
    ImageMenu { menu: String },
    LoadSaves,
    SaveSaves,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DialogueDecorationSlot {
    Portrait,
    Name,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueDecorationImage {
    pub asset: String,
    pub size: [u32; 2],
    pub placement: DialogueDecorationPlacement,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DialogueDecorationPlacement {
    Absolute {
        point: [f32; 2],
    },
    /// Capture the active text rectangle's top-left at commit, not every frame.
    TextOrigin {
        offset: [f32; 2],
    },
}
impl DialogueDecorationImage {
    pub fn valid(&self) -> bool {
        let point = match &self.placement {
            DialogueDecorationPlacement::Absolute { point } => point,
            DialogueDecorationPlacement::TextOrigin { offset } => offset,
        };
        !self.asset.is_empty()
            && self.asset.len() <= 256
            && self.size.iter().all(|n| (1..=8192).contains(n))
            && point.iter().all(|n| n.is_finite() && n.abs() <= 8192.)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueDecoration {
    pub asset: String,
    pub rect: [f32; 4],
}
impl DialogueDecoration {
    pub fn valid(&self) -> bool {
        !self.asset.is_empty()
            && self.asset.len() <= 256
            && self.rect.iter().all(|n| n.is_finite() && n.abs() <= 8192.)
            && self.rect[2] > 0.
            && self.rect[3] > 0.
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueShake {
    pub amplitude: [u32; 2],
    pub step_us: Micros,
    pub duration_us: Micros,
    #[serde(default)]
    pub randomize: bool,
}
impl DialogueShake {
    pub fn valid(&self) -> bool {
        self.amplitude.iter().all(|n| *n <= 8192)
            && self.amplitude.iter().any(|n| *n > 0)
            && (1000..=1_000_000).contains(&self.step_us.0)
            && self.step_us.0.is_multiple_of(1000)
            && (1000..=60_000_000).contains(&self.duration_us.0)
            && self.duration_us.0.is_multiple_of(1000)
            && self.steps() <= 4096
    }
    pub fn steps(&self) -> u64 {
        self.duration_us.0.div_ceil(self.step_us.0.max(1))
    }
}
impl Effect {
    pub fn collect_dialogue_style_assets(&self, theme: &Theme, assets: &mut BTreeSet<String>) {
        if let Self::DialogueDecoration {
            image: Some(image), ..
        } = self
        {
            assets.insert(image.asset.clone());
        }
        if let Self::DialogueStyle { style } = self {
            if let Some(asset) = theme
                .dialogue_styles
                .get(style)
                .and_then(|style| style.dialogue.background.as_ref())
            {
                assets.insert(asset.clone());
            }
        }
        if let Some(children) = self.compose_children() {
            for child in children {
                child.effect.collect_dialogue_style_assets(theme, assets);
            }
        }
    }
    /// Preparation fetches new image bindings; inherited images remain
    /// owned by the current concrete scene rather than the template.
    pub fn stage_image_assets<'a>(&'a self, nodes: &'a [Node]) -> impl Iterator<Item = &'a String> {
        let inherited: &[String] = match self {
            Self::StagePresent { inherit_images, .. } => inherit_images,
            _ => &[],
        };
        nodes
            .iter()
            .filter(move |node| !inherited.contains(&node.id))
            .filter_map(|node| node.asset.as_ref())
    }
    /// Children of a composition, for tree walks shared by validation,
    /// compilation and the runtime.
    pub fn compose_children(&self) -> Option<&[EffectDef]> {
        match self {
            Self::Sequence { children } | Self::ParallelAll { children } => Some(children),
            _ => None,
        }
    }
    /// Whether this subtree contains any composition node. Children exist
    /// only inside compositions, so the root being one is the whole answer.
    pub fn uses_compose(&self) -> bool {
        self.compose_children().is_some()
    }
    /// Total effect definitions in this subtree, including composites.
    pub fn compose_leaves(&self) -> usize {
        1 + self
            .compose_children()
            .map(|children| children.iter().map(|def| def.effect.compose_leaves()).sum())
            .unwrap_or(0)
    }
    /// Composition nesting depth; a leaf has depth 0.
    pub fn compose_depth(&self) -> usize {
        self.compose_children()
            .map(|children| {
                1 + children
                    .iter()
                    .map(|def| def.effect.compose_depth())
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }
    /// Depth-first test over this effect and every composition child; cue
    /// walkers that gate capabilities or media on effect kind must see the
    /// whole tree, not only top-level definitions.
    pub fn effect_tree_any(&self, predicate: &impl Fn(&Self) -> bool) -> bool {
        predicate(self)
            || self.compose_children().is_some_and(|children| {
                children
                    .iter()
                    .any(|def| def.effect.effect_tree_any(predicate))
            })
    }
    /// Every audio asset this effect subtree starts, compositions included.
    /// Stage present transitions are handled by the caller (scene nodes).
    pub fn collect_audio_assets(&self, out: &mut BTreeSet<String>) {
        if let Self::Audio { asset, .. } = self {
            out.insert(asset.clone());
        }
        for def in self.compose_children().unwrap_or(&[]) {
            def.effect.collect_audio_assets(out);
        }
    }
    /// Inline image bindings are semantic text dependencies, including
    /// dialogues nested in a composition.
    pub fn collect_text_image_assets(
        &self,
        contracts: &BTreeMap<String, TextContract>,
        out: &mut BTreeSet<String>,
    ) {
        if let Self::Dialogue { text, .. } = self {
            if let Some(contract) = contracts.get(text) {
                out.extend(contract.images.iter().map(|i| i.asset.clone()));
            }
        }
        for def in self.compose_children().unwrap_or(&[]) {
            def.effect.collect_text_image_assets(contracts, out);
        }
    }
    pub fn collect_text_ids(&self, out: &mut BTreeSet<String>) {
        if let Self::Dialogue { text, .. } = self {
            out.insert(text.clone());
        }
        for def in self.compose_children().unwrap_or(&[]) {
            def.effect.collect_text_ids(out);
        }
    }
}
/// Typed property addresses. UI-owned objects are deliberately not addressable by Story.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TweenTarget {
    SceneNode {
        node: String,
        property: Property,
    },
    DialogueRoot {
        property: DialogueProperty,
    },
    /// A live audio instance's envelope, by task handle. The envelope is the
    /// 0..1 multiplier on top of the authored event gain; one envelope owner
    /// (a gain tween or a timed stop) may target an instance at a time.
    AudioInstance {
        task: String,
        property: AudioProperty,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DialogueProperty {
    Opacity,
    BackgroundOpacity,
    TextOpacity,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AudioProperty {
    Gain,
}
impl TweenTarget {
    pub fn accepts(&self, value: f32) -> bool {
        value.is_finite()
            && match self {
                Self::SceneNode {
                    property: Property::X | Property::Y,
                    ..
                } => true,
                Self::SceneNode {
                    property: Property::Scale,
                    ..
                } => value >= 0.,
                _ => (0.0..=1.0).contains(&value),
            }
    }
}
impl Effect {
    /// Legacy Clip and typed Tween use one evaluator and one writer identity.
    pub fn scalar_track(&self, from: f32, base: f32) -> Option<(TweenTarget, ScalarTween, bool)> {
        if let Self::SourceMotion {
            node,
            property,
            to,
            duration_us,
            curve,
        } = self
        {
            return Some((
                TweenTarget::SceneNode {
                    node: node.clone(),
                    property: *property,
                },
                ScalarTween {
                    from,
                    base,
                    to: if *property == Property::Opacity {
                        *to as f32 / 255.
                    } else {
                        *to as f32
                    },
                    duration_us: *duration_us,
                    easing: Easing::Linear,
                    finish: FinishPolicy::CommitEnd,
                    cancel: CancelPolicy::CommitCurrent,
                    source_curve: Some(*curve),
                },
                true,
            ));
        }
        let (target, to, duration_us, replace, easing, finish, cancel) = match self {
            Self::Clip {
                node,
                property,
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            } => (
                TweenTarget::SceneNode {
                    node: node.clone(),
                    property: *property,
                },
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            ),
            Self::Tween {
                target,
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            } => (
                target.clone(),
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            ),
            _ => return None,
        };
        Some((
            target,
            ScalarTween {
                from,
                base,
                to: *to,
                duration_us: *duration_us,
                easing: *easing,
                finish: *finish,
                cancel: *cancel,
                source_curve: None,
            },
            *replace,
        ))
    }
}
pub(super) fn unit_gain() -> f32 {
    1.0
}
pub(super) fn is_unit_gain(value: &f32) -> bool {
    *value == 1.0
}
/// Play from the beginning to `end_us` once, then repeat [start_us, end_us).
/// Each backend resolves boundaries at its decoded buffer's sample rate,
/// choosing the nearest sample frame without trimming unrelated samples.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioLoopRegion {
    pub start_us: Micros,
    pub end_us: Micros,
}
impl AudioLoopRegion {
    pub fn valid(&self, duration_us: u64) -> bool {
        self.start_us.0 < self.end_us.0 && self.end_us.0 <= duration_us
    }
    pub fn frame_bounds(&self, rate: u32, frames: u64) -> Option<(u64, u64)> {
        if rate == 0 || self.start_us.0 >= self.end_us.0 {
            return None;
        }
        let frame = |us: u64| (us as u128 * rate as u128 + 500_000) / 1_000_000;
        let start = frame(self.start_us.0);
        let end = frame(self.end_us.0);
        (start < end && end <= frames as u128).then_some((start as u64, end as u64))
    }
    /// A cumulative device position includes the one-time intro. Wrap only
    /// after the first end boundary; restoration must not replay that intro.
    pub fn playback_frame(&self, position_us: u64, rate: u32, frames: u64) -> Option<u64> {
        let (start, end) = self.frame_bounds(rate, frames)?;
        let position = (position_us as u128 * rate as u128 + 500_000) / 1_000_000;
        Some(if position < end as u128 {
            position as u64
        } else {
            start + ((position - end as u128) % (end - start) as u128) as u64
        })
    }
}

/// Event gain is separate from the player's mixer preferences.
pub fn valid_audio_gain(value: f32) -> bool {
    value.is_finite() && (0.0..=4.0).contains(&value)
}

#[cfg(test)]
mod audio_loop_tests {
    use super::*;

    #[test]
    fn audio_loop_region_validates_bounds_and_wraps_without_replaying_intro() {
        let region = AudioLoopRegion {
            start_us: Micros(1_000_000),
            end_us: Micros(3_000_000),
        };
        assert!(region.valid(4_000_000));
        assert!(!region.valid(2_999_999));
        let invalid = AudioLoopRegion {
            start_us: Micros(2_000_000),
            end_us: Micros(2_000_000),
        };
        assert!(!invalid.valid(4_000_000));
        assert_eq!(region.frame_bounds(0, 192_000), None);
        assert_eq!(
            region.frame_bounds(48_000, 192_000),
            Some((48_000, 144_000))
        );
        // Before end_us (0.5s in intro) -> exact frame 24_000
        assert_eq!(
            region.playback_frame(500_000, 48_000, 192_000),
            Some(24_000)
        );
        // Past end_us (3.5s) -> wraps into [1s, 3s) => 1.5s => frame 72_000 (intro not replayed)
        assert_eq!(
            region.playback_frame(3_500_000, 48_000, 192_000),
            Some(72_000)
        );
        assert!(valid_audio_gain(0.0));
        assert!(valid_audio_gain(4.0));
        assert!(!valid_audio_gain(-0.1));
        assert!(!valid_audio_gain(f32::NAN));
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AudioBus {
    #[default]
    Bgm,
    Voice,
    Sfx,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    Smooth,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishPolicy {
    #[default]
    CommitEnd,
    RemoveEffect,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelPolicy {
    #[default]
    CommitCurrent,
    SettleEnd,
    RestoreBase,
}
