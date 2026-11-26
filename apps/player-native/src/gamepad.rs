//! Zero-dependency pure-Rust Linux `evdev` Gamepad host (Xbox / PlayStation / Switch / Steam Deck / 8BitDo):
//! - Directly reads `/dev/input/event*` via `libc::poll` + `ioctl` (zero `libudev` dynamic dependency!)
//! - Background event-driven thread wakes `winit` immediately via `EventLoopProxy` (0% main-thread polling!)
//! - Matches Web player button layout + supports `FF_RUMBLE` haptic feedback via `EVIOCSFF`

use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamepadEvent {
    Connected(String),
    Disconnected,
    Advance,
    CancelOrMenu,
    OpenHistory,
    ToggleHideUi,
    ScrollUp,
    ScrollDown,
    ToggleAuto,
    QuickSave,
    HoldSkip(bool),
}

pub struct GamepadHost {
    rx: mpsc::Receiver<GamepadEvent>,
    rumble_tx: mpsc::Sender<u16>,
}

impl GamepadHost {
    pub fn spawn<F>(wake: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        let (ev_tx, ev_rx) = mpsc::channel::<GamepadEvent>();
        let (rumble_tx, rumble_rx) = mpsc::channel::<u16>();

        #[cfg(target_os = "linux")]
        {
            let _ = std::thread::Builder::new()
                .name("nir-gamepad".into())
                .spawn(move || linux_evdev::run_gamepad_loop(ev_tx, rumble_rx, wake));
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (ev_tx, rumble_rx, wake);
        }

        Self {
            rx: ev_rx,
            rumble_tx,
        }
    }

    pub fn try_recv(&self) -> Option<GamepadEvent> {
        self.rx.try_recv().ok()
    }

    pub fn rumble(&self, duration_ms: u16) {
        if duration_ms > 0 {
            let _ = self.rumble_tx.send(duration_ms);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_evdev {
    use super::GamepadEvent;
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    // Linux input event types (`linux/input-event-codes.h`)
    const EV_KEY: u16 = 0x01;
    const EV_ABS: u16 = 0x03;
    const EV_FF: u16 = 0x15;

    // Standard Linux gamepad button codes
    const BTN_SOUTH: u16 = 0x130; // A / Cross
    const BTN_EAST: u16 = 0x131; // B / Circle
    const BTN_NORTH: u16 = 0x133; // Y / Triangle
    const BTN_WEST: u16 = 0x134; // X / Square
    const BTN_TL: u16 = 0x136; // L1 / LB
    const BTN_TR: u16 = 0x137; // R1 / RB
    const BTN_TR2: u16 = 0x139; // R2 / RT (digital)
    const BTN_SELECT: u16 = 0x13a;
    const BTN_START: u16 = 0x13b;
    const BTN_DPAD_UP: u16 = 0x220;
    const BTN_DPAD_DOWN: u16 = 0x221;

    // Standard Linux gamepad axes
    const ABS_Y: u16 = 0x01;
    const ABS_RZ: u16 = 0x05; // R2 / RT (analog)
    const ABS_HAT0Y: u16 = 0x11; // D-Pad vertical

    const FF_RUMBLE: u16 = 0x50;

    // Linux ioctl request numbers (`linux/input.h`)
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_READ: libc::c_ulong = 2;
    const fn ioc(dir: libc::c_ulong, ty: u8, nr: u8, size: usize) -> libc::c_ulong {
        (dir << 30)
            | ((size as libc::c_ulong) << 16)
            | ((ty as libc::c_ulong) << 8)
            | (nr as libc::c_ulong)
    }
    const fn eviocgname(len: usize) -> libc::c_ulong {
        ioc(IOC_READ, b'E', 0x06, len)
    }
    const fn eviocgbit(ev: u8, len: usize) -> libc::c_ulong {
        ioc(IOC_READ, b'E', 0x20 + ev, len)
    }
    const fn eviocgabs(abs: u8) -> libc::c_ulong {
        ioc(
            IOC_READ,
            b'E',
            0x40 + abs,
            std::mem::size_of::<InputAbsInfo>(),
        )
    }
    const EVIOCSFF: libc::c_ulong = ioc(IOC_WRITE, b'E', 0x80, std::mem::size_of::<FfEffect>());
    const EVIOCRMFF: libc::c_ulong = ioc(IOC_WRITE, b'E', 0x81, std::mem::size_of::<libc::c_int>());

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct InputEvent {
        tv_sec: libc::time_t,
        tv_usec: libc::suseconds_t,
        ev_type: u16,
        code: u16,
        value: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct InputAbsInfo {
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FfReplay {
        length: u16,
        delay: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FfTrigger {
        button: u16,
        interval: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FfRumbleEffect {
        strong_magnitude: u16,
        weak_magnitude: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FfEffect {
        effect_type: u16,
        id: i16,
        direction: u16,
        trigger: FfTrigger,
        replay: FfReplay,
        rumble: FfRumbleEffect,
        _pad: [u8; 28],
    }

    impl Default for FfEffect {
        fn default() -> Self {
            Self {
                effect_type: FF_RUMBLE,
                id: -1,
                direction: 0,
                trigger: FfTrigger::default(),
                replay: FfReplay::default(),
                rumble: FfRumbleEffect::default(),
                _pad: [0; 28],
            }
        }
    }

    fn test_bit(bits: &[u8], bit: usize) -> bool {
        bits.get(bit / 8)
            .is_some_and(|&byte| (byte & (1u8 << (bit % 8))) != 0)
    }

    pub(super) struct GamepadMapper {
        y_min: i32,
        y_max: i32,
        rz_min: i32,
        rz_max: i32,
        hat_y: i32,
        hold_skip_btn: bool,
        hold_skip_trig: bool,
        last_stick_scroll: Instant,
    }

    impl GamepadMapper {
        pub(super) fn new(y_min: i32, y_max: i32, rz_min: i32, rz_max: i32) -> Self {
            Self {
                y_min,
                y_max,
                rz_min,
                rz_max,
                hat_y: 0,
                hold_skip_btn: false,
                hold_skip_trig: false,
                last_stick_scroll: Instant::now() - Duration::from_secs(1),
            }
        }

        pub(super) fn map_event(
            &mut self,
            ev_type: u16,
            code: u16,
            value: i32,
            now: Instant,
        ) -> Option<GamepadEvent> {
            match ev_type {
                EV_KEY => {
                    let pressed = value == 1;
                    let released = value == 0;
                    match code {
                        BTN_SOUTH | BTN_TR if pressed => Some(GamepadEvent::Advance),
                        BTN_EAST if pressed => Some(GamepadEvent::CancelOrMenu),
                        BTN_WEST if pressed => Some(GamepadEvent::OpenHistory),
                        BTN_NORTH if pressed => Some(GamepadEvent::ToggleHideUi),
                        BTN_TL | BTN_DPAD_UP if pressed => Some(GamepadEvent::ScrollUp),
                        BTN_DPAD_DOWN if pressed => Some(GamepadEvent::ScrollDown),
                        BTN_START if pressed => Some(GamepadEvent::ToggleAuto),
                        BTN_SELECT if pressed => Some(GamepadEvent::QuickSave),
                        BTN_TR2 if pressed || released => {
                            let prev = self.hold_skip_btn || self.hold_skip_trig;
                            self.hold_skip_btn = pressed;
                            let next = self.hold_skip_btn || self.hold_skip_trig;
                            (next != prev).then_some(GamepadEvent::HoldSkip(next))
                        }
                        _ => None,
                    }
                }
                EV_ABS => match code {
                    ABS_HAT0Y => {
                        let prev = self.hat_y;
                        self.hat_y = value;
                        if value < 0 && prev >= 0 {
                            Some(GamepadEvent::ScrollUp)
                        } else if value > 0 && prev <= 0 {
                            Some(GamepadEvent::ScrollDown)
                        } else {
                            None
                        }
                    }
                    ABS_Y => {
                        let span = (self.y_max - self.y_min).max(1) as f32;
                        let center = (self.y_min + self.y_max) as f32 * 0.5;
                        let norm = (value as f32 - center) / (span * 0.5);
                        if norm.abs() > 0.65
                            && now.saturating_duration_since(self.last_stick_scroll)
                                >= Duration::from_millis(240)
                        {
                            self.last_stick_scroll = now;
                            Some(if norm < 0.0 {
                                GamepadEvent::ScrollUp
                            } else {
                                GamepadEvent::ScrollDown
                            })
                        } else {
                            None
                        }
                    }
                    ABS_RZ => {
                        let span = (self.rz_max - self.rz_min).max(1) as f32;
                        let norm = (value - self.rz_min) as f32 / span;
                        let active = norm > 0.45;
                        let prev = self.hold_skip_btn || self.hold_skip_trig;
                        self.hold_skip_trig = active;
                        let next = self.hold_skip_btn || self.hold_skip_trig;
                        (next != prev).then_some(GamepadEvent::HoldSkip(next))
                    }
                    _ => None,
                },
                _ => None,
            }
        }

        pub(super) fn is_hold_skip(&self) -> bool {
            self.hold_skip_btn || self.hold_skip_trig
        }
    }

    struct OpenedGamepad {
        file: File,
        name: String,
        mapper: GamepadMapper,
        has_rumble: bool,
        writable: bool,
        rumble_id: i16,
    }

    impl OpenedGamepad {
        fn play_rumble(&mut self, duration_ms: u16) {
            if !self.has_rumble || !self.writable {
                return;
            }
            let mut eff = FfEffect {
                effect_type: FF_RUMBLE,
                id: self.rumble_id,
                replay: FfReplay {
                    length: duration_ms.clamp(30, 500),
                    delay: 0,
                },
                rumble: FfRumbleEffect {
                    strong_magnitude: 0x3800,
                    weak_magnitude: 0x5800,
                },
                ..Default::default()
            };
            let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCSFF as _, &mut eff) };
            if rc < 0 {
                return;
            }
            self.rumble_id = eff.id;
            let play = InputEvent {
                tv_sec: 0,
                tv_usec: 0,
                ev_type: EV_FF,
                code: eff.id as u16,
                value: 1,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (&play as *const InputEvent).cast::<u8>(),
                    std::mem::size_of::<InputEvent>(),
                )
            };
            let _ =
                unsafe { libc::write(self.file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        }
    }

    impl Drop for OpenedGamepad {
        fn drop(&mut self) {
            if self.rumble_id >= 0 && self.writable {
                let id = self.rumble_id as libc::c_int;
                let _ = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCRMFF as _, id) };
            }
        }
    }

    fn probe_device(path: &PathBuf) -> Option<OpenedGamepad> {
        let (file, writable) = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(f) => (f, true),
            Err(_) => {
                let f = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                    .ok()?;
                (f, false)
            }
        };
        let fd = file.as_raw_fd();

        let mut ev_bits = [0u8; 8];
        if unsafe { libc::ioctl(fd, eviocgbit(0, ev_bits.len()) as _, ev_bits.as_mut_ptr()) } < 0 {
            return None;
        }
        if !test_bit(&ev_bits, EV_KEY as usize) {
            return None;
        }

        let mut key_bits = [0u8; 96];
        if unsafe {
            libc::ioctl(
                fd,
                eviocgbit(EV_KEY as u8, key_bits.len()) as _,
                key_bits.as_mut_ptr(),
            )
        } < 0
        {
            return None;
        }
        // Real gamepads expose standard `BTN_SOUTH` (A/Cross) AND `BTN_EAST` (B/Circle).
        if !test_bit(&key_bits, BTN_SOUTH as usize) || !test_bit(&key_bits, BTN_EAST as usize) {
            return None;
        }

        let mut name_buf = [0u8; 128];
        let name_len =
            unsafe { libc::ioctl(fd, eviocgname(name_buf.len()) as _, name_buf.as_mut_ptr()) };
        let name = if name_len > 0 {
            let slice = &name_buf[..(name_len as usize).min(name_buf.len())];
            let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
            String::from_utf8_lossy(&slice[..end]).trim().to_owned()
        } else {
            "Gamepad".to_owned()
        };

        let mut abs_y = InputAbsInfo {
            minimum: -32768,
            maximum: 32767,
            ..Default::default()
        };
        let mut abs_rz = InputAbsInfo {
            minimum: 0,
            maximum: 255,
            ..Default::default()
        };
        if test_bit(&ev_bits, EV_ABS as usize) {
            let _ = unsafe { libc::ioctl(fd, eviocgabs(ABS_Y as u8) as _, &mut abs_y) };
            let _ = unsafe { libc::ioctl(fd, eviocgabs(ABS_RZ as u8) as _, &mut abs_rz) };
        }

        let mut has_rumble = false;
        if test_bit(&ev_bits, EV_FF as usize) {
            let mut ff_bits = [0u8; 16];
            if unsafe {
                libc::ioctl(
                    fd,
                    eviocgbit(EV_FF as u8, ff_bits.len()) as _,
                    ff_bits.as_mut_ptr(),
                )
            } >= 0
            {
                has_rumble = test_bit(&ff_bits, FF_RUMBLE as usize);
            }
        }

        Some(OpenedGamepad {
            file,
            name: if name.is_empty() {
                "Gamepad".into()
            } else {
                name
            },
            mapper: GamepadMapper::new(
                abs_y.minimum,
                abs_y.maximum,
                abs_rz.minimum,
                abs_rz.maximum,
            ),
            has_rumble,
            writable,
            rumble_id: -1,
        })
    }

    fn scan_first_gamepad() -> Option<OpenedGamepad> {
        let rd = std::fs::read_dir("/dev/input").ok()?;
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.starts_with("event"))
            })
            .collect();
        paths.sort();
        for p in paths {
            if let Some(gp) = probe_device(&p) {
                return Some(gp);
            }
        }
        None
    }

    pub(super) fn run_gamepad_loop<F>(
        ev_tx: mpsc::Sender<GamepadEvent>,
        rumble_rx: mpsc::Receiver<u16>,
        wake: F,
    ) where
        F: Fn() + Send + 'static,
    {
        let mut active: Option<OpenedGamepad> = None;
        let ev_size = std::mem::size_of::<InputEvent>();
        let mut raw_buf = vec![0u8; ev_size * 16];

        loop {
            if let Some(gp) = active.as_mut() {
                while let Ok(ms) = rumble_rx.try_recv() {
                    gp.play_rumble(ms);
                }

                let mut pfd = libc::pollfd {
                    fd: gp.file.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut pfd, 1, 120) };
                if ready < 0 {
                    continue;
                }
                if (pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                    if gp.mapper.is_hold_skip() {
                        let _ = ev_tx.send(GamepadEvent::HoldSkip(false));
                    }
                    active = None;
                    if ev_tx.send(GamepadEvent::Disconnected).is_err() {
                        return;
                    }
                    wake();
                    continue;
                }
                if ready == 0 {
                    // Check if receiver closed while idle.
                    if matches!(rumble_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                        return;
                    }
                    continue;
                }

                match gp.file.read(&mut raw_buf) {
                    Ok(n) if n >= ev_size => {
                        let now = Instant::now();
                        let count = n / ev_size;
                        let mut sent = false;
                        for i in 0..count {
                            let mut ev = InputEvent::default();
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    raw_buf.as_ptr().add(i * ev_size),
                                    (&mut ev as *mut InputEvent).cast::<u8>(),
                                    ev_size,
                                );
                            }
                            if let Some(mapped) =
                                gp.mapper.map_event(ev.ev_type, ev.code, ev.value, now)
                            {
                                if ev_tx.send(mapped).is_err() {
                                    return;
                                }
                                sent = true;
                            }
                        }
                        if sent {
                            wake();
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => {
                        if gp.mapper.is_hold_skip() {
                            let _ = ev_tx.send(GamepadEvent::HoldSkip(false));
                        }
                        active = None;
                        if ev_tx.send(GamepadEvent::Disconnected).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            } else {
                // Drain any pending rumble requests while disconnected and exit if channel closed.
                match rumble_rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(_) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if let Some(gp) = scan_first_gamepad() {
                    let name = gp.name.clone();
                    active = Some(gp);
                    if ev_tx.send(GamepadEvent::Connected(name)).is_err() {
                        return;
                    }
                    wake();
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn gamepad_mapper_maps_standard_buttons_dpad_and_r2_hold_skip() {
            let mut m = GamepadMapper::new(-32768, 32767, 0, 255);
            let now = Instant::now();

            assert_eq!(
                m.map_event(EV_KEY, BTN_SOUTH, 1, now),
                Some(GamepadEvent::Advance)
            );
            assert_eq!(m.map_event(EV_KEY, BTN_SOUTH, 0, now), None);
            assert_eq!(
                m.map_event(EV_KEY, BTN_EAST, 1, now),
                Some(GamepadEvent::CancelOrMenu)
            );
            assert_eq!(
                m.map_event(EV_KEY, BTN_WEST, 1, now),
                Some(GamepadEvent::OpenHistory)
            );
            assert_eq!(
                m.map_event(EV_KEY, BTN_NORTH, 1, now),
                Some(GamepadEvent::ToggleHideUi)
            );
            assert_eq!(
                m.map_event(EV_KEY, BTN_START, 1, now),
                Some(GamepadEvent::ToggleAuto)
            );
            assert_eq!(
                m.map_event(EV_KEY, BTN_SELECT, 1, now),
                Some(GamepadEvent::QuickSave)
            );

            // D-pad hat edge detection
            assert_eq!(
                m.map_event(EV_ABS, ABS_HAT0Y, -1, now),
                Some(GamepadEvent::ScrollUp)
            );
            assert_eq!(m.map_event(EV_ABS, ABS_HAT0Y, -1, now), None);
            assert_eq!(m.map_event(EV_ABS, ABS_HAT0Y, 0, now), None);
            assert_eq!(
                m.map_event(EV_ABS, ABS_HAT0Y, 1, now),
                Some(GamepadEvent::ScrollDown)
            );

            // Analog R2 trigger hold-skip threshold (>45%)
            assert_eq!(m.map_event(EV_ABS, ABS_RZ, 50, now), None);
            assert_eq!(
                m.map_event(EV_ABS, ABS_RZ, 180, now),
                Some(GamepadEvent::HoldSkip(true))
            );
            assert_eq!(m.map_event(EV_ABS, ABS_RZ, 200, now), None);
            assert_eq!(
                m.map_event(EV_ABS, ABS_RZ, 20, now),
                Some(GamepadEvent::HoldSkip(false))
            );
        }
    }
}
