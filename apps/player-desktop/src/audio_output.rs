//! Device opening/teardown stays off the owner thread. The stream lives here;
//! its mixer handle is the only platform resource delivered to the owner.
use rodio::{cpal::traits::HostTrait, mixer::Mixer, OutputStream, OutputStreamBuilder};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

pub(crate) struct OutputHandle {
    pub mixer: Mixer,
    pub rate: u32,
}
pub(crate) type OutputReply = crate::audio_output_worker::WorkerReply<OutputHandle>;
pub(crate) type OutputWorker = crate::audio_output_worker::Worker<OutputHandle>;
impl OutputWorker {
    pub fn new() -> Self {
        Self::with_factory(|| {
            // Platform stream types need not be Send: create and drop them
            // entirely inside this worker, rather than capturing one here.
            let mut stream: Option<OutputStream> = None;
            move |generation, fault| {
                // Stop the old callback before attaching its voice queues to a
                // replacement mixer. CPAL teardown can wait, so it runs here too.
                drop(stream.take());
                let opened = open(generation, fault)?;
                let handle = OutputHandle {
                    mixer: opened.mixer().clone(),
                    rate: opened.config().sample_rate(),
                };
                stream = Some(opened);
                // Keep the stream captured and owned by this worker closure.
                debug_assert!(stream.is_some());
                Ok(handle)
            }
        })
    }
}
// Each attempted configuration has its own callback identity. Failed fallback
// candidates must not report a fault against the successfully selected stream.
fn candidate(
    builder: OutputStreamBuilder,
    generation: u64,
    fault: Arc<AtomicU64>,
) -> Result<OutputStream, String> {
    let live = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let publish = live.clone();
    let observed = failed.clone();
    let signal = fault.clone();
    let mut stream = builder
        .with_error_callback(move |_| {
            observed.store(true, Ordering::SeqCst);
            if publish.load(Ordering::SeqCst) {
                signal.fetch_max(generation, Ordering::Release);
            }
        })
        .open_stream()
        .map_err(|error| format!("E_AUDIO_OUTPUT: {error}"))?;
    if failed.load(Ordering::SeqCst) {
        stream.log_on_drop(false);
        return Err("E_AUDIO_OUTPUT: stream failed while opening".into());
    }
    live.store(true, Ordering::SeqCst);
    if failed.load(Ordering::SeqCst) {
        fault.fetch_max(generation, Ordering::Release);
    }
    stream.log_on_drop(false);
    Ok(stream)
}
fn open(generation: u64, fault: Arc<AtomicU64>) -> Result<OutputStream, String> {
    let original = OutputStreamBuilder::from_default_device()
        .map_err(|error| format!("E_AUDIO_OUTPUT: {error}"))
        .and_then(|builder| candidate(builder, generation, fault.clone()));
    let initial = match original {
        Ok(stream) => return Ok(stream),
        Err(error) => error,
    };
    let devices = rodio::cpal::default_host()
        .output_devices()
        .map_err(|_| initial.clone())?;
    for device in devices {
        if let Ok(builder) = OutputStreamBuilder::from_device(device.clone()) {
            if let Ok(stream) = candidate(builder, generation, fault.clone()) {
                return Ok(stream);
            }
        }
        // Preserve Rodio's supported-format fallback order, giving every
        // configuration its own callback rather than cloning a stale signal.
        if let Ok(configs) = rodio::stream::supported_output_configs(&device) {
            for config in configs {
                let builder = OutputStreamBuilder::default()
                    .with_device(device.clone())
                    .with_supported_config(&config);
                if let Ok(stream) = candidate(builder, generation, fault.clone()) {
                    return Ok(stream);
                }
            }
        }
    }
    Err(initial)
}
