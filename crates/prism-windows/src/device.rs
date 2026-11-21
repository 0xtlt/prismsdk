//! The device APIs' pure parts (`location.rs`, `camera.rs`, `haptics.rs`,
//! `sensors.rs`): accuracy settings, Windows' times and missing values as
//! Prism's positions, the privacy settings' values as permission states,
//! haptic patterns as waveforms, sensor readings as motion samples, a
//! share's text, the file names logs give, async answers given once,
//! what the startup checks found. No WinRT: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{Accuracy, BiometryKind, Capabilities, Haptic, Location, Motion, PermissionState};

/// How Windows' location service is asked for a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// `DesiredAccuracyInMeters`; `None`: `PositionAccuracy::High` (GPS).
    pub meters: Option<u32>,
    /// A position this recent is good enough (`maximumAge`, seconds).
    pub max_age_s: u64,
    /// One position takes at most this long (seconds).
    pub timeout_s: u64,
    /// A watch reports moves of at least this many meters.
    pub movement_m: f64,
}

/// What the web asks the browser for (`client.js`): a kilometer and ten
/// minutes, a hundred meters and 30 s, the best and 30 s; 20 s at most.
pub fn settings(a: Accuracy) -> Settings {
    match a {
        Accuracy::Coarse => Settings {
            meters: Some(1000),
            max_age_s: 600,
            timeout_s: 20,
            movement_m: 100.,
        },
        Accuracy::Balanced => Settings {
            meters: Some(100),
            max_age_s: 30,
            timeout_s: 20,
            movement_m: 10.,
        },
        Accuracy::Best => Settings {
            meters: None,
            max_age_s: 30,
            timeout_s: 20,
            movement_m: 0.,
        },
    }
}

/// 100 ns ticks between 1601-01-01 (Windows' epoch) and 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// A Windows time (`DateTime::UniversalTime`: 100 ns since 1601) in Unix seconds.
pub fn unix_seconds(ticks: i64) -> f64 {
    (ticks - UNIX_EPOCH_TICKS) as f64 / 1e7
}

/// A seconds count as a WinRT `TimeSpan` (100 ns ticks).
pub fn ticks(seconds: u64) -> i64 {
    seconds as i64 * 10_000_000
}

/// What `Geocoordinate` says, its optional values unwrapped.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: f64,
    /// `AltitudeAccuracy`: `None` when the position has no altitude
    /// (Wi-Fi positions say 0 m then).
    pub altitude_accuracy: Option<f64>,
    pub accuracy: f64,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
    pub timestamp_ticks: i64,
}

fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|v| v.is_finite())
}

/// A position as Prism's: no altitude without its accuracy; Windows says
/// NaN for the speed and heading of a position that is not moving.
pub fn location(r: &Reading) -> Location {
    Location {
        latitude: r.latitude,
        longitude: r.longitude,
        altitude: finite(r.altitude_accuracy)
            .and(Some(r.altitude))
            .filter(|a| a.is_finite()),
        accuracy: r.accuracy,
        speed: finite(r.speed).filter(|s| *s >= 0.),
        heading: finite(r.heading).filter(|h| (0. ..=360.).contains(h)),
        timestamp: unix_seconds(r.timestamp_ticks),
    }
}

/// Privacy settings' values (the consent store's `Value`: `Allow`, `Deny`,
/// `Prompt`), the broadest first (the device's switch, the user's, then
/// the app's): one `Deny` is enough, `Prompt` means Windows still asks.
/// Missing values allow.
pub fn consent(values: &[Option<&str>]) -> PermissionState {
    if values.contains(&Some("Deny")) {
        PermissionState::Denied
    } else if values.contains(&Some("Prompt")) {
        PermissionState::NotDetermined
    } else {
        PermissionState::Granted
    }
}

/// A waveform of `SimpleHapticsController` (`KnownSimpleHapticsControllerWaveforms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Click,
    Press,
    Success,
    Error,
}

/// How a haptic pattern plays: the waveforms to try (the first the device
/// has), its intensity (0 to 1), how many times, the pause between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pattern {
    pub waves: &'static [Wave],
    pub intensity: f64,
    pub count: i32,
    pub pause_ms: i64,
}

/// Apple's feedback generators on Windows' waveforms: impacts are clicks
/// of rising strength (a heavy one presses), results their own waveform
/// where the device has it, else clicks (two for a warning, three for an
/// error, as iOS taps them).
pub fn pattern(h: Haptic) -> Pattern {
    let one = |waves, intensity| Pattern {
        waves,
        intensity,
        count: 1,
        pause_ms: 0,
    };
    match h {
        Haptic::Selection => one(&[Wave::Click], 0.3),
        Haptic::Light => one(&[Wave::Click], 0.5),
        Haptic::Medium => one(&[Wave::Click], 0.8),
        Haptic::Heavy => one(&[Wave::Press, Wave::Click], 1.0),
        Haptic::Success => one(&[Wave::Success, Wave::Click], 1.0),
        Haptic::Warning => Pattern {
            waves: &[Wave::Click],
            intensity: 1.0,
            count: 2,
            pause_ms: 100,
        },
        Haptic::Error => Pattern {
            waves: &[Wave::Error, Wave::Click],
            intensity: 1.0,
            count: 1,
            pause_ms: 100,
        },
    }
}

/// How often `wave` plays for `p`: an error without its own waveform
/// clicks three times.
pub fn count(p: &Pattern, wave: Wave) -> i32 {
    if p.waves.first() == Some(&Wave::Error) && wave == Wave::Click {
        3
    } else {
        p.count
    }
}

/// One reading of each motion sensor the PC has (`None`: no such sensor,
/// or no reading yet). Windows' axes are Apple's: x to the right of the
/// screen, y up it, z out of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sensors {
    /// `Accelerometer`, gravity included, in g (lying face up: z is -1).
    pub acceleration: Option<[f64; 3]>,
    /// The fused gravity sensor (`AccelerometerReadingType::Gravity`), in g.
    pub gravity: Option<[f64; 3]>,
    /// The fused linear acceleration (`AccelerometerReadingType::Linear`).
    pub linear: Option<[f64; 3]>,
    /// `Gyrometer`, around x, y, z, in degrees per second.
    pub rotation_deg: Option<[f64; 3]>,
    /// `Inclinometer`: pitch (around x, ±180°), roll (around y, ±90°) and
    /// yaw (around z, 0-360°), the web's `deviceorientation` beta, gamma
    /// and alpha.
    pub inclination_deg: Option<[f64; 3]>,
}

/// How long gravity takes to follow the accelerometer without a gravity
/// sensor (a low-pass filter's time constant, seconds).
const GRAVITY_LAG_S: f64 = 0.25;

/// Readings as Prism's `Motion`: Apple's units (g, radians per second,
/// radians). Without a fused gravity sensor, gravity is the
/// accelerometer's low-pass filtered value and the user's acceleration
/// what is left, as Android does.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fusion {
    gravity: Option<[f64; 3]>,
}

impl Fusion {
    /// The sample for readings `dt_s` seconds after the last ones; `None`
    /// before any sensor has a reading.
    pub fn sample(&mut self, s: &Sensors, dt_s: f64) -> Option<Motion> {
        if s.acceleration.is_none()
            && s.gravity.is_none()
            && s.linear.is_none()
            && s.rotation_deg.is_none()
            && s.inclination_deg.is_none()
        {
            return None;
        }
        let gravity = match (s.gravity, s.acceleration) {
            (Some(g), _) => Some(g),
            (None, Some(a)) => Some(match self.gravity {
                None => a,
                Some(g) => {
                    let k = GRAVITY_LAG_S / (GRAVITY_LAG_S + dt_s.max(0.));
                    std::array::from_fn(|i| g[i] * k + a[i] * (1. - k))
                }
            }),
            (None, None) => None,
        };
        self.gravity = gravity;
        let g = gravity.unwrap_or_default();
        let acceleration = match (s.linear, s.acceleration) {
            (Some(l), _) => l,
            (None, Some(a)) if gravity.is_some() => std::array::from_fn(|i| a[i] - g[i]),
            _ => [0.; 3],
        };
        let rad = |d: [f64; 3]| d.map(f64::to_radians);
        let [pitch, roll, yaw] = s.inclination_deg.map(rad).unwrap_or_default();
        Some(Motion {
            acceleration,
            gravity: g,
            rotation_rate: s.rotation_deg.map(rad).unwrap_or_default(),
            attitude: [roll, pitch, yaw],
        })
    }
}

/// What the startup checks have found so far (`probe.rs`); `None`: that
/// check has not answered yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Windows Hello: its sensor, and whether it can verify the user.
    pub hello: Option<(BiometryKind, bool)>,
    pub location: Option<bool>,
    pub motion: Option<bool>,
    pub camera: Option<bool>,
    pub haptics: Option<bool>,
    pub passkeys: Option<bool>,
    pub share: Option<bool>,
}

impl Found {
    /// No answer yet.
    pub const NONE: Found = Found {
        hello: None,
        location: None,
        motion: None,
        camera: None,
        haptics: None,
        passkeys: None,
        share: None,
    };

    /// `caps` with what the checks found. One that has not answered says
    /// no until it does; then the app is given the new capabilities.
    pub fn apply(&self, mut caps: Capabilities) -> Capabilities {
        let (kind, usable) = self.hello.unwrap_or((BiometryKind::None, false));
        caps.biometry = kind;
        caps.device_auth = usable;
        let yes = |a: Option<bool>| a == Some(true);
        caps.location = yes(self.location);
        caps.motion = yes(self.motion);
        caps.camera = yes(self.camera);
        caps.haptics = yes(self.haptics);
        caps.passkeys = yes(self.passkeys);
        caps.share = yes(self.share);
        caps
    }

    /// The checks that have not answered.
    pub fn pending(&self) -> Vec<&'static str> {
        [
            ("Windows Hello", self.hello.is_none()),
            ("location", self.location.is_none()),
            ("motion", self.motion.is_none()),
            ("camera", self.camera.is_none()),
            ("haptics", self.haptics.is_none()),
            ("passkeys", self.passkeys.is_none()),
            ("share", self.share.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, pending)| pending.then_some(name))
        .collect()
    }
}

/// Whether a device call may go ahead, from its check's answer: unless
/// the check found nothing. Before it answers, the call tries the API
/// itself, which fails on its own where there is nothing: a slow check
/// never stops a call.
pub fn may(answer: Option<bool>) -> bool {
    answer != Some(false)
}

/// Answers recorded on other threads, before or after the UI thread takes
/// them. [`open`](Self::open) gives what came so far, and from then on
/// [`record`](Self::record) says the UI is to be told: none is lost.
#[derive(Debug)]
pub struct Mailbox<T> {
    inner: std::sync::Mutex<(T, bool)>,
}

impl<T: Copy> Mailbox<T> {
    pub const fn new(t: T) -> Self {
        Mailbox {
            inner: std::sync::Mutex::new((t, false)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (T, bool)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records an answer; true when the UI took the earlier ones and is to
    /// be told of this one.
    pub fn record(&self, f: impl FnOnce(&mut T)) -> bool {
        let mut inner = self.lock();
        f(&mut inner.0);
        inner.1
    }

    /// What came so far; the answers after it are told.
    pub fn open(&self) -> T {
        let mut inner = self.lock();
        inner.1 = true;
        inner.0
    }

    /// What came so far.
    pub fn get(&self) -> T {
        self.lock().0
    }
}

/// Gives `then` the result of an async call started by `start`, exactly
/// once. `start` gets the callback to hand the call (windows-rs `when`);
/// when it fails, it has dropped that callback without calling it, so
/// `then` gets `start`'s error instead: a device call always answers.
pub fn answered<R, E, F, S>(then: F, start: S)
where
    F: FnOnce(Result<R, E>) + Send + 'static,
    S: FnOnce(Box<dyn FnOnce(Result<R, E>) + Send>) -> Result<(), E>,
{
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(then)));
    let take = |s: &std::sync::Mutex<Option<F>>| s.lock().unwrap_or_else(|e| e.into_inner()).take();
    let ours = slot.clone();
    let r = start(Box::new(move |r| {
        if let Some(then) = take(&ours) {
            then(r);
        }
    }));
    if let Err(e) = r
        && let Some(then) = take(&slot)
    {
        then(Err(e));
    }
}

/// The text a share gives targets that read only text: the text, then
/// the link on its own line, as on Android; the link alone when there is
/// no text.
pub fn share_text(text: Option<&str>, url: Option<&str>) -> Option<String> {
    match (
        text.filter(|t| !t.is_empty()),
        url.filter(|u| !u.is_empty()),
    ) {
        (Some(t), Some(u)) => Some(format!("{t}\n{u}")),
        (t, u) => t.or(u).map(str::to_string),
    }
}

/// The last part of a Windows path (`C:\Users\<account>\…\report.json`
/// is `report.json`): what the logs name, never the folders, which say
/// whose account it is.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_says_no_only_once_it_has_answered() {
        let base = Capabilities {
            tray: true,
            camera: true,
            ..Default::default()
        };
        // Nothing yet: no device API, and no Hello.
        let caps = Found::NONE.apply(base);
        assert!(caps.tray && !caps.camera && !caps.location && !caps.device_auth);
        assert_eq!(caps.biometry, BiometryKind::None);
        assert_eq!(Found::NONE.pending().len(), 7);
        let found = Found {
            hello: Some((BiometryKind::Face, true)),
            camera: Some(true),
            location: Some(false),
            ..Found::NONE
        };
        let caps = found.apply(base);
        assert!(caps.camera && !caps.location && caps.device_auth);
        assert_eq!(caps.biometry, BiometryKind::Face);
        assert_eq!(found.pending(), ["motion", "haptics", "passkeys", "share"]);
        // A call waits for no check: only a « no » stops it.
        assert!(may(None));
        assert!(may(Some(true)));
        assert!(!may(Some(false)));
    }

    #[test]
    fn answers_before_and_after_the_ui_opens_are_all_had() {
        let mailbox = Mailbox::new(Found::NONE);
        // Before the UI takes them: in what it takes, nothing to tell.
        assert!(!mailbox.record(|f| f.camera = Some(true)));
        let opened = mailbox.open();
        assert_eq!(opened.camera, Some(true));
        // After: to tell, and there when the UI looks.
        assert!(mailbox.record(|f| f.share = Some(false)));
        assert_eq!(mailbox.get().share, Some(false));
        assert_eq!(mailbox.get().camera, Some(true));
        // From threads, racing the UI: each answer is in what it opened,
        // or it is told.
        let mailbox = std::sync::Arc::new(Mailbox::new(0u32));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let m = mailbox.clone();
                std::thread::spawn(move || m.record(|n| *n |= 1 << i).then_some(i))
            })
            .collect();
        let opened = mailbox.open();
        let told: Vec<u32> = threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .collect();
        for i in 0..8 {
            assert!(
                opened & (1 << i) != 0 || told.contains(&i),
                "answer {i} lost"
            );
        }
        assert_eq!(mailbox.get(), 0xff);
    }

    #[test]
    fn an_async_call_always_answers_once() {
        use std::sync::mpsc::channel;
        type Done = Box<dyn FnOnce(Result<i32, &'static str>) + Send>;
        // Answered by the call, now or later.
        let (tx, rx) = channel();
        answered(
            move |r| tx.send(r).unwrap(),
            |cb: Done| {
                cb(Ok(1));
                Ok(())
            },
        );
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Ok(1)]);
        let (tx, rx) = channel();
        let mut later = None;
        answered(
            move |r| tx.send(r).unwrap(),
            |cb: Done| {
                later = Some(cb);
                Ok(())
            },
        );
        assert!(rx.try_recv().is_err());
        later.unwrap()(Ok(2));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Ok(2)]);
        // The call could not be waited on: windows-rs dropped the callback.
        let (tx, rx) = channel();
        answered(
            move |r| tx.send(r).unwrap(),
            |cb: Done| {
                drop(cb);
                Err("SetCompleted failed")
            },
        );
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [Err("SetCompleted failed")]
        );
        // Never twice.
        let (tx, rx) = channel();
        answered(
            move |r| tx.send(r).unwrap(),
            |cb: Done| {
                cb(Ok(3));
                Err("late")
            },
        );
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Ok(3)]);
    }

    #[test]
    fn text_only_targets_get_the_link_too() {
        let t = |text, url| share_text(text, url);
        assert_eq!(
            t(Some("Look"), Some("https://e.com")).as_deref(),
            Some("Look\nhttps://e.com")
        );
        assert_eq!(
            t(None, Some("https://e.com")).as_deref(),
            Some("https://e.com")
        );
        assert_eq!(t(Some("Look"), None).as_deref(), Some("Look"));
        assert_eq!(
            t(Some(""), Some("https://e.com")).as_deref(),
            Some("https://e.com")
        );
        assert_eq!(t(None, None), None);
    }

    #[test]
    fn logs_name_the_file_only() {
        assert_eq!(
            file_name(r"C:\Users\someone\agents\x\device-report.json"),
            "device-report.json"
        );
        assert_eq!(file_name("C:/Users/someone/photo.jpg"), "photo.jpg");
        assert_eq!(file_name("report.json"), "report.json");
        assert_eq!(file_name(r"C:\Users\someone\"), "");
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9)
    }

    #[test]
    fn no_reading_no_sample() {
        assert_eq!(Fusion::default().sample(&Sensors::default(), 0.05), None);
    }

    #[test]
    fn fused_sensors_are_used_as_they_are() {
        let s = Sensors {
            acceleration: Some([0.1, 0., -1.]),
            gravity: Some([0., 0., -1.]),
            linear: Some([0.1, 0., 0.]),
            rotation_deg: Some([180., -90., 0.]),
            inclination_deg: Some([10., -20., 90.]),
        };
        let m = Fusion::default().sample(&s, 0.05).unwrap();
        assert!(near(m.gravity, [0., 0., -1.]));
        assert!(near(m.acceleration, [0.1, 0., 0.]));
        let pi = std::f64::consts::PI;
        assert!(near(m.rotation_rate, [pi, -pi / 2., 0.]));
        // Roll, pitch, yaw, as Apple orders them.
        assert!(near(
            m.attitude,
            [(-20f64).to_radians(), 10f64.to_radians(), pi / 2.]
        ));
    }

    #[test]
    fn gravity_follows_the_accelerometer_without_a_gravity_sensor() {
        let mut f = Fusion::default();
        let flat = Sensors {
            acceleration: Some([0., 0., -1.]),
            ..Sensors::default()
        };
        // The first reading is all gravity.
        let m = f.sample(&flat, 0.05).unwrap();
        assert!(near(m.gravity, [0., 0., -1.]) && near(m.acceleration, [0.; 3]));
        // A push along x: most of it is the user's, gravity barely moves.
        let push = Sensors {
            acceleration: Some([0.5, 0., -1.]),
            ..Sensors::default()
        };
        let m = f.sample(&push, 0.05).unwrap();
        let k = 0.25 / 0.3;
        assert!(near(m.gravity, [0.5 * (1. - k), 0., -1.]));
        assert!(near(m.acceleration, [0.5 * k, 0., 0.]));
        // Held still, gravity settles on the new direction.
        for _ in 0..200 {
            f.sample(&push, 0.05);
        }
        let m = f.sample(&push, 0.05).unwrap();
        assert!(near(m.gravity, [0.5, 0., -1.]) && near(m.acceleration, [0.; 3]));
    }

    #[test]
    fn a_gyrometer_alone_gives_rotation_only() {
        let s = Sensors {
            rotation_deg: Some([0., 0., 45.]),
            ..Sensors::default()
        };
        let m = Fusion::default().sample(&s, 0.05).unwrap();
        assert!(near(m.rotation_rate, [0., 0., std::f64::consts::FRAC_PI_4]));
        assert!(near(m.acceleration, [0.; 3]) && near(m.gravity, [0.; 3]));
        assert!(near(m.attitude, [0.; 3]));
    }

    #[test]
    fn times() {
        assert_eq!(unix_seconds(UNIX_EPOCH_TICKS), 0.);
        // 2024-01-01T00:00:00Z.
        assert_eq!(unix_seconds(133_485_408_000_000_000), 1_704_067_200.);
        assert_eq!(unix_seconds(UNIX_EPOCH_TICKS + 5_000_000), 0.5);
        assert_eq!(ticks(20), 200_000_000);
    }

    #[test]
    fn accuracy_settings() {
        assert_eq!(settings(Accuracy::Coarse).meters, Some(1000));
        assert_eq!(settings(Accuracy::Coarse).max_age_s, 600);
        assert_eq!(settings(Accuracy::Balanced).meters, Some(100));
        assert_eq!(settings(Accuracy::Best).meters, None);
        assert!(settings(Accuracy::Best).timeout_s <= 20);
    }

    #[test]
    fn positions() {
        let r = Reading {
            latitude: 48.85,
            longitude: 2.35,
            altitude: 35.,
            altitude_accuracy: Some(5.),
            accuracy: 12.,
            speed: Some(1.5),
            heading: Some(90.),
            timestamp_ticks: UNIX_EPOCH_TICKS + 10_000_000,
        };
        let l = location(&r);
        assert_eq!((l.latitude, l.longitude, l.accuracy), (48.85, 2.35, 12.));
        assert_eq!(l.altitude, Some(35.));
        assert_eq!(l.speed, Some(1.5));
        assert_eq!(l.heading, Some(90.));
        assert_eq!(l.timestamp, 1.);
        // Wi-Fi: no altitude; standing still: NaN speed and heading.
        let l = location(&Reading {
            altitude: 0.,
            altitude_accuracy: None,
            speed: Some(f64::NAN),
            heading: Some(f64::NAN),
            ..r
        });
        assert_eq!((l.altitude, l.speed, l.heading), (None, None, None));
        let l = location(&Reading {
            speed: Some(-1.),
            heading: None,
            ..r
        });
        assert_eq!((l.speed, l.heading), (None, None));
    }

    #[test]
    fn privacy_settings() {
        assert_eq!(consent(&[None, None]), PermissionState::Granted);
        assert_eq!(
            consent(&[Some("Allow"), Some("Allow")]),
            PermissionState::Granted
        );
        assert_eq!(
            consent(&[Some("Deny"), Some("Allow")]),
            PermissionState::Denied
        );
        assert_eq!(
            consent(&[Some("Allow"), Some("Prompt")]),
            PermissionState::NotDetermined
        );
        assert_eq!(
            consent(&[Some("Prompt"), Some("Deny")]),
            PermissionState::Denied
        );
    }

    #[test]
    fn haptic_patterns() {
        assert_eq!(pattern(Haptic::Selection).waves, [Wave::Click]);
        assert!(pattern(Haptic::Selection).intensity < pattern(Haptic::Light).intensity);
        assert!(pattern(Haptic::Light).intensity < pattern(Haptic::Medium).intensity);
        assert_eq!(pattern(Haptic::Heavy).waves[0], Wave::Press);
        assert_eq!(pattern(Haptic::Success).waves, [Wave::Success, Wave::Click]);
        let warning = pattern(Haptic::Warning);
        assert_eq!(count(&warning, Wave::Click), 2);
        let error = pattern(Haptic::Error);
        assert_eq!(count(&error, Wave::Error), 1);
        assert_eq!(count(&error, Wave::Click), 3);
        for h in [
            Haptic::Selection,
            Haptic::Light,
            Haptic::Medium,
            Haptic::Heavy,
            Haptic::Success,
            Haptic::Warning,
            Haptic::Error,
        ] {
            let p = pattern(h);
            assert!(
                p.waves.contains(&Wave::Click),
                "{h:?} falls back to a click"
            );
            assert!((0. ..=1.).contains(&p.intensity));
        }
    }
}
