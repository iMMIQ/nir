//! Pure-Rust Linux D-Bus desktop integrations via `zbus` (zero `libdbus` dynamic dependency!):
//! - `org.freedesktop.ScreenSaver.Inhibit` during Auto/Skip reading (prevents display sleep)
//! - MPRIS2 (`org.mpris.MediaPlayer2.nir.instance<pid>`) exposing game title, speaker & current
//!   dialogue excerpt, and handling Bluetooth/desktop media keys (Play/Pause -> Auto, Next -> Advance, Previous -> Rollback)

use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKeyAction {
    ToggleAuto,
    Advance,
    Rollback,
}

#[allow(dead_code)]
enum DbusCmd {
    SetInhibit(bool),
    UpdateMetadata {
        title: String,
        speaker: String,
        text: String,
        auto_active: bool,
    },
    Shutdown,
}

pub struct LinuxDbusHost {
    tx: Option<mpsc::Sender<DbusCmd>>,
    rx: mpsc::Receiver<MediaKeyAction>,
    last_inhibit: bool,
}

impl LinuxDbusHost {
    pub fn spawn<F>(initial_title: String, wake: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        let (key_tx, key_rx) = mpsc::channel::<MediaKeyAction>();

        #[cfg(target_os = "linux")]
        let tx = {
            let (cmd_tx, cmd_rx) = mpsc::channel::<DbusCmd>();
            let _ = std::thread::Builder::new()
                .name("nir-dbus".into())
                .spawn(move || {
                    linux_impl::run_dbus_worker(initial_title, cmd_rx, key_tx, wake);
                });
            Some(cmd_tx)
        };

        #[cfg(not(target_os = "linux"))]
        let tx = {
            let _ = (initial_title, key_tx, wake);
            None
        };

        Self {
            tx,
            rx: key_rx,
            last_inhibit: false,
        }
    }

    pub fn set_inhibit(&mut self, active: bool) {
        if active == self.last_inhibit {
            return;
        }
        self.last_inhibit = active;
        if let Some(tx) = &self.tx {
            let _ = tx.send(DbusCmd::SetInhibit(active));
        }
    }

    pub fn update_metadata(&self, title: &str, speaker: &str, text: &str, auto_active: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(DbusCmd::UpdateMetadata {
                title: title.to_owned(),
                speaker: speaker.to_owned(),
                text: text.chars().take(80).collect(),
                auto_active,
            });
        }
    }

    pub fn try_recv_media_key(&self) -> Option<MediaKeyAction> {
        self.rx.try_recv().ok()
    }
}

impl Drop for LinuxDbusHost {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(DbusCmd::Shutdown);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{DbusCmd, MediaKeyAction};
    use std::collections::HashMap;
    use std::sync::{mpsc, Arc, Mutex};
    use zbus::interface;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};

    struct SharedMprisState {
        title: String,
        speaker: String,
        text: String,
        auto_active: bool,
    }

    struct MprisRoot;

    #[interface(name = "org.mpris.MediaPlayer2")]
    impl MprisRoot {
        fn raise(&self) {}
        fn quit(&self) {}

        #[zbus(property)]
        fn can_quit(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn can_raise(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn has_track_list(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn identity(&self) -> &'static str {
            "NIR Player"
        }

        #[zbus(property)]
        fn desktop_entry(&self) -> &'static str {
            "nir-player"
        }

        #[zbus(property)]
        fn supported_uri_schemes(&self) -> Vec<String> {
            Vec::new()
        }

        #[zbus(property)]
        fn supported_mime_types(&self) -> Vec<String> {
            Vec::new()
        }
    }

    struct MprisPlayer {
        state: Arc<Mutex<SharedMprisState>>,
        key_tx: mpsc::Sender<MediaKeyAction>,
        wake: Arc<dyn Fn() + Send + Sync>,
    }

    impl MprisPlayer {
        fn send_action(&self, action: MediaKeyAction) {
            if self.key_tx.send(action).is_ok() {
                (self.wake)();
            }
        }
    }

    #[interface(name = "org.mpris.MediaPlayer2.Player")]
    impl MprisPlayer {
        fn next(&self) {
            self.send_action(MediaKeyAction::Advance);
        }

        fn previous(&self) {
            self.send_action(MediaKeyAction::Rollback);
        }

        fn pause(&self) {
            self.send_action(MediaKeyAction::ToggleAuto);
        }

        fn play_pause(&self) {
            self.send_action(MediaKeyAction::ToggleAuto);
        }

        fn play(&self) {
            self.send_action(MediaKeyAction::ToggleAuto);
        }

        fn stop(&self) {}

        #[zbus(property)]
        fn playback_status(&self) -> &'static str {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if guard.auto_active {
                "Playing"
            } else {
                "Paused"
            }
        }

        #[zbus(property)]
        fn rate(&self) -> f64 {
            1.0
        }

        #[zbus(property)]
        fn metadata(&self) -> HashMap<String, OwnedValue> {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut map = HashMap::new();
            if let Ok(path) = ObjectPath::try_from("/org/nir/player/story") {
                if let Ok(v) = OwnedValue::try_from(Value::ObjectPath(path)) {
                    map.insert("mpris:trackid".to_owned(), v);
                }
            }
            if let Ok(v) = OwnedValue::try_from(Value::from(guard.title.clone())) {
                map.insert("xesam:title".to_owned(), v);
            }
            let artist = if guard.speaker.trim().is_empty() {
                "NIR Visual Novel".to_owned()
            } else {
                guard.speaker.trim().to_owned()
            };
            if let Ok(v) = OwnedValue::try_from(Value::from(vec![artist])) {
                map.insert("xesam:artist".to_owned(), v);
            }
            if !guard.text.is_empty() {
                if let Ok(v) = OwnedValue::try_from(Value::from(guard.text.clone())) {
                    map.insert("xesam:album".to_owned(), v);
                }
            }
            map
        }

        #[zbus(property)]
        fn can_go_next(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn can_go_previous(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn can_play(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn can_pause(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn can_seek(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn can_control(&self) -> bool {
            true
        }
    }

    fn inhibit_screensaver(conn: &zbus::blocking::Connection) -> Option<u32> {
        let reply = conn
            .call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "Inhibit",
                &("NIR Player", "Visual novel auto-reading"),
            )
            .ok()?;
        reply.body().deserialize::<u32>().ok()
    }

    fn uninhibit_screensaver(conn: &zbus::blocking::Connection, cookie: u32) {
        let _ = conn.call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/org/freedesktop/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "UnInhibit",
            &(cookie,),
        );
    }

    pub(super) fn run_dbus_worker<F>(
        initial_title: String,
        cmd_rx: mpsc::Receiver<DbusCmd>,
        key_tx: mpsc::Sender<MediaKeyAction>,
        wake: F,
    ) where
        F: Fn() + Send + 'static,
    {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return;
        }
        let state = Arc::new(Mutex::new(SharedMprisState {
            title: initial_title,
            speaker: String::new(),
            text: String::new(),
            auto_active: false,
        }));
        let wake_sync: Arc<dyn Fn() + Send + Sync> = {
            let mu = Mutex::new(wake);
            Arc::new(move || {
                if let Ok(f) = mu.lock() {
                    f();
                }
            })
        };
        let bus_name = format!("org.mpris.MediaPlayer2.nir.instance{}", std::process::id());
        let player = MprisPlayer {
            state: Arc::clone(&state),
            key_tx,
            wake: wake_sync,
        };
        let Ok(builder) = zbus::blocking::connection::Builder::session() else {
            return;
        };
        let conn = match builder
            .name(bus_name)
            .and_then(|b| b.serve_at("/org/mpris/MediaPlayer2", MprisRoot))
            .and_then(|b| b.serve_at("/org/mpris/MediaPlayer2", player))
            .and_then(|b| b.build())
        {
            Ok(c) => c,
            Err(_) => return,
        };

        let mut inhibit_cookie: Option<u32> = None;
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                DbusCmd::SetInhibit(want) => {
                    if want && inhibit_cookie.is_none() {
                        inhibit_cookie = inhibit_screensaver(&conn);
                    } else if !want {
                        if let Some(cookie) = inhibit_cookie.take() {
                            uninhibit_screensaver(&conn, cookie);
                        }
                    }
                }
                DbusCmd::UpdateMetadata {
                    title,
                    speaker,
                    text,
                    auto_active,
                } => {
                    {
                        let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
                        g.title = title;
                        g.speaker = speaker;
                        g.text = text;
                        g.auto_active = auto_active;
                    }
                    if let Ok(iface_ref) = conn
                        .object_server()
                        .interface::<_, MprisPlayer>("/org/mpris/MediaPlayer2")
                    {
                        pollster::block_on(async {
                            let emitter = iface_ref.signal_emitter();
                            let p = iface_ref.get();
                            let _ = p.metadata_changed(emitter).await;
                            let _ = p.playback_status_changed(emitter).await;
                        });
                    }
                }
                DbusCmd::Shutdown => break,
            }
        }

        if let Some(cookie) = inhibit_cookie.take() {
            uninhibit_screensaver(&conn, cookie);
        }
    }
}
