use super::*;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentPriority {
    Required,
    Prefetch,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreparePriority {
    Required,
    Near,
    Speculative,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveEnvelope {
    pub format: u32,
    pub slot: u32,
    pub revision: u32,
    pub snapshot: Snapshot,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceKind {
    Preferences,
    Profile,
    ProfileValues,
}
impl PersistenceKind {
    pub(super) fn location(self) -> &'static str {
        match self {
            Self::Preferences => "preferences",
            Self::Profile => "profile",
            Self::ProfileValues => "profile_values",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PersistencePhase {
    Read,
    Write,
}
/// Inspect a persisted slot without restoring it or changing the live Player.
/// Deserialize the snapshot before hashing: JSON number spelling and object
/// key order are transport details, while the digest uses typed Rust encoding.
pub fn inspect_save_slot(json: &str, slot: u32, release: &str, game: &str) -> Result<u32> {
    let envelope: SaveEnvelope = nir_content::parse(json.as_bytes(), "save")?;
    if slot >= MAX_SAVE_SLOTS
        || envelope.format != 1
        || envelope.slot != slot
        || envelope.snapshot.release != release
        || envelope.snapshot.game_id != game
    {
        return Err(Diagnostic::new(
            "E_SAVE_IDENTITY",
            "save",
            "stored slot does not match this game and release",
        ));
    }
    if envelope.revision == 0 {
        return Err(Diagnostic::new(
            "E_SAVE_REVISION",
            "save",
            "stored revision is zero",
        ));
    }
    nir_content::verify(
        &serde_json::to_vec(&envelope.snapshot).unwrap(),
        &envelope.digest,
    )?;
    Ok(envelope.revision)
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppCommand {
    GetContent {
        request: u32,
        session: u32,
        objects: Vec<ContentRequest>,
        priority: ContentPriority,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    PromoteContent {
        request: u32,
        session: u32,
    },
    CancelContent {
        request: u32,
    },
    ResourceStage {
        stage: String,
        request: u32,
        asset: String,
        object: Option<String>,
        session: u32,
        device: u32,
        start_us: Micros,
        end_us: Micros,
        bytes: usize,
    },
    Observation {
        origin_session: Option<u32>,
        task: Option<u32>,
        sequence: Option<u32>,
        stage: String,
        session: u32,
        device: u32,
        request: Option<u32>,
        location: String,
        cue: Option<String>,
    },
    Diagnostic {
        diagnostic: Box<Diagnostic>,
    },
    GetAssets {
        request: u32,
        session: u32,
        device: u32,
        assets: Vec<String>,
        descriptors: BTreeMap<String, Asset>,
        priority: PreparePriority,
    },
    PromoteAssets {
        request: u32,
        session: u32,
    },
    CancelAssets {
        request: u32,
    },
    PreparePresentation {
        request: u32,
    },
    PrepareLocale {
        request: u32,
        ui_locale: String,
        text_locale: String,
    },
    AudioStart {
        domain: TimeDomain,
        task: u32,
        asset: String,
        bus: AudioBus,
        looped: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        loop_region: Option<AudioLoopRegion>,
        position_us: Micros,
        gain: f32,
        envelope: f32,
        character: String,
        session: u32,
    },
    AudioCharacter {
        domain: TimeDomain,
        task: u32,
        character: String,
        session: u32,
    },
    AudioEnvelope {
        owner: Option<u32>,
        elapsed_us: Micros,
        session: u32,
        domain: TimeDomain,
        task: u32,
        from: f32,
        to: f32,
        duration_us: Micros,
    },
    AudioStop {
        session: u32,
        domain: TimeDomain,
        task: u32,
    },
    AudioPause {
        domain: TimeDomain,
        paused: bool,
    },
    /// Additional per-bus pause ownership, combined with domain suspension.
    AudioBusPause {
        domain: TimeDomain,
        bus: AudioBus,
        paused: bool,
    },
    AudioReset {
        domain: TimeDomain,
    },
    Save {
        slot: u32,
        expected_revision: u32,
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    Load {
        slot: u32,
        job: u32,
    },
    Export {
        job: u32,
        json: String,
    },
    Import,
    ApplyPreferences {
        preferences: Preferences,
    },
    PersistPreferences {
        preferences: Preferences,
    },
    PersistProfile {
        /// Newly learned keys. Hosts persist a monotonic union, including
        /// progress already stored by another release or browser tab.
        keys: BTreeSet<String>,
    },
    PersistProfileValues {
        values: BTreeMap<String, nir_format::Value>,
    },
    ListSaves,
    Trace {
        event: String,
        at: String,
    },
}
#[derive(Debug, Clone)]
pub enum AppEvent {
    AudioPositions {
        domain: TimeDomain,
        session: u32,
        positions: Vec<AudioPosition>,
    },
    ContentReady {
        request: u32,
        objects: Vec<Vec<u8>>,
    },
    ContentFailed {
        request: u32,
        message: String,
    },
    ContentSkipped {
        request: u32,
        code: String,
        message: String,
    },
    Action {
        action: UiAction,
        interaction: u32,
        sequence: u32,
        session: u32,
    },
    Tick {
        delta_us: u64,
    },
    /// Host elapsed time after independently applying domain boundary policies.
    TickDomains {
        story_us: u64,
        foreground_us: u64,
    },
    /// Internal continuation: this elapsed time was already charged to the UI clock.
    #[doc(hidden)]
    ContinueStoryTime {
        delta_us: u64,
    },
    AssetReady {
        request: u32,
        asset: String,
    },
    AssetFailed {
        request: u32,
        message: String,
    },
    AssetFault {
        request: u32,
        diagnostic: Box<Diagnostic>,
    },
    AssetsCancelled {
        request: u32,
    },
    PresentationReady {
        request: u32,
    },
    LocaleReady {
        request: u32,
    },
    LocaleFailed {
        request: u32,
        message: String,
    },
    AudioEnded {
        domain: TimeDomain,
        task: u32,
        session: u32,
    },
    AudioFailed {
        domain: TimeDomain,
        task: u32,
        session: u32,
        message: String,
    },
    Hidden(bool),
    SlotLoaded {
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    SlotLoadFailed {
        job: u32,
        message: String,
    },
    Loaded {
        envelope: Box<SaveEnvelope>,
    },
    LoadFailed(String),
    Exported {
        job: u32,
    },
    ExportCancelled {
        job: u32,
    },
    ExportFailed {
        job: u32,
        message: String,
    },
    HostFailed(String),
    Saved {
        job: u32,
        slot: u32,
        revision: u32,
    },
    SaveFault {
        job: u32,
        diagnostic: Box<Diagnostic>,
    },
    SaveFailed {
        job: u32,
        message: String,
    },
    /// The original physical write is still awaiting a terminal result.
    SavePending {
        job: u32,
        message: String,
    },
    Slots(Vec<SlotView>, BTreeMap<u32, u32>),
    Preferences(Preferences),
    Profile(BTreeSet<String>),
    ProfileValues(BTreeMap<String, nir_format::Value>),
    PreferencesRecovered(Preferences),
    ProfileRecovered(BTreeSet<String>),
    ProfileValuesRecovered(BTreeMap<String, nir_format::Value>),
    PersistenceReadFailed {
        kind: PersistenceKind,
        message: String,
    },
    PersistenceFailed {
        kind: PersistenceKind,
        message: String,
    },
    PersistenceStored(PersistenceKind),
    DeviceLost,
    DeviceReady,
}
