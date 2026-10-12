//! Tests of the worker: what it says, in what order, and when it stays quiet.
//!
//! These run against a stand-in for the model and the audio output, so that
//! they are fast and exact. **A stand-in proves scheduling and policy only.**
//! That speech is really Kokoro's, and really comes out of the headphones, is
//! shown by the `kokoro_real_*` and `voice_real_*` tests, which use the real
//! model and the real endpoint.

use super::policy::TEST_PHRASE;
use super::*;
use crate::device::state::{ConnectionState, NoiseControlMode, NoiseControlState, StateSource};
use std::collections::HashMap;

fn headphones() -> Output {
    Output {
        id: "aurora".into(),
        name: "Headphones (Aurora)".into(),
    }
}
fn speakers() -> Output {
    Output {
        id: "speakers".into(),
        name: "Speakers".into(),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Play {
    text: String,
    output: String,
    gain: f32,
    completed: bool,
}

/// What the stand-in is set up to do, and what it was asked to do.
struct Fake {
    assets: Result<(), String>,
    load: Result<(), String>,
    synth_error: Option<String>,
    play_error: Option<String>,
    headphones: Option<Output>,
    /// How many times the headphones' output must be asked for before it
    /// appears, as it does a moment after they connect.
    headphones_after: usize,
    default: Option<Output>,
    hold_play: bool,
    hold_synth: bool,

    loaded: bool,
    loads: usize,
    unloads: usize,
    synths: Vec<(String, String, f32)>,
    plays: Vec<Play>,
    routes: Vec<(Via, bool)>,
    /// Which sentence each synthesized clip is, by its (unique) length.
    clips: HashMap<usize, String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            assets: Ok(()),
            load: Ok(()),
            synth_error: None,
            play_error: None,
            headphones: Some(headphones()),
            headphones_after: 0,
            default: Some(speakers()),
            hold_play: false,
            hold_synth: false,
            loaded: false,
            loads: 0,
            unloads: 0,
            synths: Vec::new(),
            plays: Vec::new(),
            routes: Vec::new(),
            clips: HashMap::new(),
        }
    }
}

struct FakeBackend(Arc<Mutex<Fake>>);

impl Backend for FakeBackend {
    fn assets_dir(&self) -> String {
        "C:\\voice\\kokoro".into()
    }
    fn check_assets(&mut self) -> Result<(), String> {
        self.0.lock().assets.clone()
    }
    fn load(&mut self) -> Result<(), String> {
        let mut f = self.0.lock();
        f.loads += 1;
        f.loaded = f.load.is_ok();
        f.load.clone()
    }
    fn is_loaded(&self) -> bool {
        self.0.lock().loaded
    }
    fn unload(&mut self) {
        let mut f = self.0.lock();
        if f.loaded {
            f.unloads += 1;
        }
        f.loaded = false;
    }
    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError> {
        self.0
            .lock()
            .synths
            .push((text.to_string(), voice.id.to_string(), speed));
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(SynthError::Cancelled);
            }
            if !self.0.lock().hold_synth {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut f = self.0.lock();
        if let Some(why) = &f.synth_error {
            return Err(SynthError::Failed(why.clone()));
        }
        let length = 1000 + f.clips.len();
        f.clips.insert(length, text.to_string());
        Ok(Speech {
            samples: vec![0.1; length],
            sample_rate: 24_000,
        })
    }
    fn route(&mut self, via: Via, allow_default: bool) -> Route {
        let mut f = self.0.lock();
        f.routes.push((via, allow_default));
        let asked = f.routes.iter().filter(|(v, _)| *v == Via::HeadphonesFirst).count();
        let there = f.headphones.clone().filter(|_| asked > f.headphones_after);
        choose_route(via, there, f.default.clone(), allow_default)
    }
    fn play(
        &mut self,
        output: &Output,
        speech: &Speech,
        gain: f32,
        stop: &AtomicBool,
    ) -> Result<Played, String> {
        let text = self.0.lock().clips[&speech.samples.len()].clone();
        let mut completed = true;
        loop {
            if stop.load(Ordering::SeqCst) {
                completed = false;
                break;
            }
            if !self.0.lock().hold_play {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut f = self.0.lock();
        if let Some(why) = &f.play_error {
            return Err(why.clone());
        }
        f.plays.push(Play {
            text,
            output: output.name.clone(),
            gain,
            completed,
        });
        Ok(Played {
            completed,
            endpoint_peak: 0.0,
        })
    }
}

struct Rig {
    voice: VoiceFeedback,
    fake: Arc<Mutex<Fake>>,
    store: Arc<SettingsStore>,
    statuses: Arc<Mutex<Vec<VoiceStatus>>>,
}

/// Voice feedback on, headphones named, no ahead-of-time synthesis.
fn settings_on() -> StoredSettings {
    StoredSettings {
        voice_enabled: true,
        preferred_device_name: Some("Aurora".into()),
        ..StoredSettings::default()
    }
}

fn fast() -> Timing {
    Timing {
        idle_unload: Duration::from_secs(3600),
        route_wait: Duration::from_millis(400),
        route_retry: Duration::from_millis(5),
        prepare_ahead: false,
    }
}

fn rig_with(settings: StoredSettings, timing: Timing, set_up: impl FnOnce(&mut Fake)) -> Rig {
    rig_keeping(settings, timing, None, set_up)
}

/// As [`rig_with`], keeping what is synthesized in `spoken` if one is given.
fn rig_keeping(
    settings: StoredSettings,
    timing: Timing,
    spoken: Option<&std::path::Path>,
    set_up: impl FnOnce(&mut Fake),
) -> Rig {
    let mut fake = Fake::default();
    set_up(&mut fake);
    let fake = Arc::new(Mutex::new(fake));
    let store = Arc::new(SettingsStore::in_memory(settings));
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&statuses);
    let voice = VoiceFeedback::start_with_store(
        Arc::clone(&store),
        Box::new(FakeBackend(Arc::clone(&fake))),
        timing,
        spoken.map(|dir| store::SpeechStore::new(dir.to_path_buf())),
        move |status| sink.lock().push(status.clone()),
    );
    let rig = Rig {
        voice,
        fake,
        store,
        statuses,
    };
    // The worker checks for the model as it starts.
    rig.until("the model has been looked for", |r| {
        r.voice.status().state != VoiceState::Initializing
            || r.statuses.lock().iter().any(|s| s.state != VoiceState::Initializing)
    });
    rig
}

fn rig(set_up: impl FnOnce(&mut Fake)) -> Rig {
    rig_with(settings_on(), fast(), set_up)
}

impl Rig {
    /// Waits for something the worker does on its own thread.
    fn until(&self, what: &str, done: impl Fn(&Rig) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Waits until the worker has nothing left to do.
    fn settle(&self) {
        self.until("the worker is idle", |r| {
            let shared = r.voice.inner.shared.lock();
            shared.queue.is_empty() && shared.prepare.is_empty() && shared.current.is_none()
        });
        // One more beat, so a job that was just taken has registered itself.
        std::thread::sleep(Duration::from_millis(15));
        self.until("the worker is idle", |r| {
            let shared = r.voice.inner.shared.lock();
            shared.queue.is_empty() && shared.prepare.is_empty() && shared.current.is_none()
        });
    }

    fn spoken(&self) -> Vec<String> {
        self.fake
            .lock()
            .plays
            .iter()
            .filter(|p| p.completed)
            .map(|p| p.text.clone())
            .collect()
    }

    fn plays(&self) -> Vec<Play> {
        self.fake.lock().plays.clone()
    }

    fn states(&self) -> Vec<VoiceState> {
        self.statuses.lock().iter().map(|s| s.state).collect()
    }

    fn set(&self, change: impl FnOnce(&mut StoredSettings)) {
        let before = self.store.get();
        let after = self.store.update(change).unwrap();
        self.voice.settings_changed(&before, &after);
    }

    /// A snapshot of real, connected headphones in the given mode.
    fn snapshot(&self, mode: NoiseControlMode) {
        self.voice.observe_snapshot(&real_snapshot(Some(mode)));
    }

    fn step(&self, change: impl FnOnce(&mut Step)) {
        let mut step = Step {
            name: "Aurora".into(),
            ..Step::default()
        };
        change(&mut step);
        self.voice.observe_presence(&step);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        {
            let mut f = self.fake.lock();
            f.hold_play = false;
            f.hold_synth = false;
        }
        self.voice.shutdown();
    }
}

fn real_snapshot(mode: Option<NoiseControlMode>) -> DeviceSnapshot {
    DeviceSnapshot {
        connection: ConnectionState::Connected,
        noise_control: mode.map(|mode| NoiseControlState {
            mode,
            level: None,
            label: None,
            source: StateSource::VendorProtocol,
            read_at: "2026-10-10T09:00:00Z".into(),
        }),
        ..DeviceSnapshot::empty(DeviceSource::Real)
    }
}

/// A snapshot of real, connected headphones in the custom mode they call
/// `name`.
fn custom_snapshot(name: &str) -> DeviceSnapshot {
    let mut snapshot = real_snapshot(Some(NoiseControlMode::Custom));
    if let Some(state) = &mut snapshot.noise_control {
        state.label = Some(name.to_string());
        state.level = Some(4);
    }
    snapshot
}

const QUIET: &str = "Quiet mode activated.";
const AWARE: &str = "Aware mode activated.";
const CONNECTED: &str = "Aurora connected.";
const DISCONNECTED: &str = "Aurora disconnected.";
const FOCUS: &str = "Focus mode activated.";

// --- the model on disk -------------------------------------------------------

#[test]
fn an_installed_model_is_ready_and_offers_its_voices() {
    let r = rig(|_| {});
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Ready);
    assert_eq!(status.voices.len(), assets::VOICES.len());
    assert!(status.voices.iter().any(|v| v.id == "af_bella"));
    assert_eq!(status.model, assets::MODEL_NAME);
    assert!(!status.model_loaded, "nothing is loaded until something is said");
    assert_eq!(r.fake.lock().loads, 0);
}

#[test]
fn a_missing_model_says_so_and_offers_no_voices() {
    let r = rig(|f| f.assets = Err("The voice model is not installed.".into()));
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::NotInstalled);
    assert!(status.detail.contains("not installed"));
    assert!(status.voices.is_empty(), "no voices for a model that is not there");
    assert_eq!(status.assets_dir, "C:\\voice\\kokoro");
}

#[test]
fn without_the_model_test_voice_fails_clearly_and_nothing_is_spoken() {
    let r = rig(|f| f.assets = Err("The voice model is not installed.".into()));
    let err = r.voice.test().unwrap_err();
    assert!(err.contains("not installed"), "{err}");

    // Nor do automatic announcements queue up waiting for a model.
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.step(|s| s.returned = true);
    r.settle();
    assert!(r.spoken().is_empty());
    assert!(r.fake.lock().synths.is_empty());
}

#[test]
fn checking_again_after_installing_makes_it_ready() {
    let r = rig(|f| f.assets = Err("The voice model is not installed.".into()));
    assert_eq!(r.voice.status().state, VoiceState::NotInstalled);

    r.fake.lock().assets = Ok(());
    let status = r.voice.recheck();
    assert_eq!(status.state, VoiceState::Ready);
    assert_eq!(status.voices.len(), assets::VOICES.len());
    assert!(r.voice.test().is_ok());
}

#[test]
fn a_model_that_goes_missing_is_noticed_on_the_next_check() {
    let r = rig(|_| {});
    r.fake.lock().assets = Err("model.int8.onnx is damaged".into());
    let status = r.voice.recheck();
    assert_eq!(status.state, VoiceState::NotInstalled);
    assert!(status.voices.is_empty());
}

// --- test voice --------------------------------------------------------------

#[test]
fn test_voice_speaks_the_sample_sentence_through_the_headphones() {
    let r = rig(|_| {});
    r.voice.test().unwrap();
    r.settle();

    assert_eq!(
        r.plays(),
        vec![Play {
            text: TEST_PHRASE.into(),
            output: "Headphones (Aurora)".into(),
            gain: gain_for(80),
            completed: true,
        }]
    );
    assert_eq!(
        r.states(),
        vec![
            VoiceState::Ready,
            VoiceState::Initializing,
            // (the model finishing loading changes `modelLoaded`, not the state)
            VoiceState::Initializing,
            VoiceState::Synthesizing,
            VoiceState::Playing,
            VoiceState::Ready,
        ]
    );
    let statuses = r.statuses.lock();
    let playing = statuses.iter().find(|s| s.state == VoiceState::Playing).unwrap();
    assert!(playing.is_test, "a test is marked as a test");
    assert_eq!(playing.speaking.as_deref(), Some(TEST_PHRASE));
    assert_eq!(playing.output.as_deref(), Some("Headphones (Aurora)"));
    let last = statuses.last().unwrap();
    assert!(!last.is_test && last.speaking.is_none());
}

/// The Test Voice button is how a voice is chosen before announcements are
/// switched on, so it works with them off.
#[test]
fn test_voice_works_with_voice_feedback_switched_off() {
    let r = rig_with(
        StoredSettings {
            voice_enabled: false,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

#[test]
fn pressing_test_repeatedly_does_not_queue_a_backlog() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the first test is playing", |r| r.voice.status().state == VoiceState::Playing);
    for _ in 0..10 {
        r.voice.test().unwrap();
    }
    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(r.spoken().len(), 2, "the one playing, and one waiting");
}

// --- voice, speed, volume ----------------------------------------------------

#[test]
fn the_chosen_voice_and_speed_are_what_the_model_is_asked_for() {
    let r = rig_with(
        StoredSettings {
            voice_id: "bm_george".into(),
            voice_speed_percent: 130,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(
        r.fake.lock().synths,
        vec![(TEST_PHRASE.to_string(), "bm_george".to_string(), 1.3)]
    );
}

#[test]
fn the_volume_is_applied_to_the_speech_and_follows_the_setting() {
    let r = rig(|_| {});
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_volume_percent = 30);
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_volume_percent = 0);
    r.voice.test().unwrap();
    r.settle();

    let gains: Vec<f32> = r.plays().iter().map(|p| p.gain).collect();
    assert_eq!(gains, vec![gain_for(80), gain_for(30), 0.0]);
    assert!(gain_for(100) == 1.0 && gain_for(50) == 0.25, "an even-sounding curve");
    assert_eq!(gain_for(250), 1.0, "never above what the model produced");
}

#[test]
fn changing_voice_or_speed_changes_the_very_next_thing_said() {
    let r = rig(|_| {});
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_id = "am_adam".into());
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_speed_percent = 150);
    r.voice.test().unwrap();
    r.settle();

    let asked: Vec<(String, f32)> = r
        .fake
        .lock()
        .synths
        .iter()
        .map(|(_, voice, speed)| (voice.clone(), *speed))
        .collect();
    assert_eq!(
        asked,
        vec![
            ("af_bella".to_string(), 1.0),
            ("am_adam".to_string(), 1.0),
            ("am_adam".to_string(), 1.5),
        ]
    );
}

// --- one model, reused -------------------------------------------------------

#[test]
fn the_model_is_loaded_once_and_reused() {
    let r = rig(|_| {});
    r.voice.test().unwrap();
    r.settle();
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.settle();
    r.step(|s| s.low_battery = Some(15));
    r.settle();

    assert_eq!(r.spoken().len(), 3);
    assert_eq!(r.fake.lock().loads, 1);
    assert!(r.voice.status().model_loaded);
}

#[test]
fn a_sentence_said_before_is_not_synthesized_again() {
    let r = rig(|_| {});
    for _ in 0..3 {
        r.voice.test().unwrap();
        r.settle();
    }
    assert_eq!(r.spoken().len(), 3);
    assert_eq!(r.fake.lock().synths.len(), 1, "synthesized once, played three times");
}

#[test]
fn a_different_voice_is_a_different_recording() {
    let r = rig(|_| {});
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_id = "bf_emma".into());
    r.voice.test().unwrap();
    r.settle();
    r.set(|s| s.voice_id = "af_bella".into());
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 2, "the first voice was still cached");
}

#[test]
fn the_cache_is_bounded() {
    let r = rig(|_| {});
    // Far more distinct sentences than the cache holds: one per battery level.
    for percent in 0..=20u8 {
        r.step(|s| s.low_battery = Some(percent));
        r.settle();
    }
    for speed in [60u16, 70, 80, 90, 110, 120] {
        r.set(|s| s.voice_speed_percent = speed);
        r.voice.test().unwrap();
        r.settle();
    }
    assert!(r.voice.inner.shared.lock().cache.len() <= CACHE_ENTRIES);
}

/// Left idle, the model's memory is given back. The sentences it already
/// produced stay, so the usual announcements still work without reloading.
#[test]
fn an_idle_model_is_released_and_cached_sentences_survive_it() {
    let r = rig_with(
        settings_on(),
        Timing {
            idle_unload: Duration::from_millis(40),
            ..fast()
        },
        |_| {},
    );
    r.voice.test().unwrap();
    r.settle();
    r.until("the idle model is released", |r| r.fake.lock().unloads == 1);
    assert!(!r.voice.status().model_loaded);

    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.spoken().len(), 2);
    assert_eq!(r.fake.lock().loads, 1, "the cached sentence needed no model");

    // Something new does need it, and it comes back.
    r.step(|s| s.low_battery = Some(9));
    r.settle();
    assert_eq!(r.fake.lock().loads, 2);
}

// --- failures ----------------------------------------------------------------

#[test]
fn a_model_that_will_not_load_is_an_error_and_nothing_is_played() {
    let r = rig(|f| f.load = Err("The speech runtime could not load the model.".into()));
    r.voice.test().unwrap();
    r.settle();
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Error);
    assert!(status.detail.contains("could not load"));
    assert!(r.plays().is_empty());
    assert!(!status.model_loaded);
}

#[test]
fn a_synthesis_failure_is_an_error_and_the_next_success_clears_it() {
    let r = rig(|f| f.synth_error = Some("the model produced no audio".into()));
    r.voice.test().unwrap();
    r.settle();
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Error);
    assert!(status.detail.contains("Speech synthesis failed"), "{}", status.detail);
    assert!(r.plays().is_empty());

    r.fake.lock().synth_error = None;
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.voice.status().state, VoiceState::Ready);
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

#[test]
fn a_playback_failure_names_the_output_it_failed_on() {
    let r = rig(|f| f.play_error = Some("the audio output stopped during playback".into()));
    r.voice.test().unwrap();
    r.settle();
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Error);
    assert!(status.detail.contains("Headphones (Aurora)"), "{}", status.detail);
    assert!(status.detail.contains("stopped during playback"));
}

/// A failed synthesis is not cached as if it were speech.
#[test]
fn a_failure_is_retried_from_scratch_next_time() {
    let r = rig(|f| f.synth_error = Some("failed".into()));
    r.voice.test().unwrap();
    r.settle();
    r.fake.lock().synth_error = None;
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 2);
}

// --- where it is played ------------------------------------------------------

#[test]
fn with_the_headphones_away_and_no_fallback_nothing_is_played_anywhere() {
    let r = rig(|f| f.headphones = None);
    r.voice.test().unwrap();
    r.settle();

    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Unavailable);
    assert!(status.detail.contains("default output is switched off"), "{}", status.detail);
    assert!(r.plays().is_empty(), "not out of the laptop's speakers");
    assert!(r.fake.lock().synths.is_empty(), "and nothing was synthesized for nowhere");
}

#[test]
fn the_fallback_is_used_only_when_it_has_been_switched_on() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |f| f.headphones = None,
    );
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.plays()[0].output, "Speakers");
}

#[test]
fn the_headphones_are_used_even_when_the_fallback_is_allowed() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.plays()[0].output, "Headphones (Aurora)");
}

#[test]
fn no_output_at_all_is_unavailable_not_an_error() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |f| {
            f.headphones = None;
            f.default = None;
        },
    );
    r.voice.test().unwrap();
    r.settle();
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Unavailable);
    assert!(status.detail.contains("no audio output"), "{}", status.detail);
}

#[test]
fn routing_prefers_the_headphones_and_never_falls_back_unasked() {
    let (h, d) = (Some(headphones()), Some(speakers()));
    use Via::*;
    assert_eq!(choose_route(HeadphonesFirst, h.clone(), d.clone(), false), Route::To(headphones()));
    assert_eq!(choose_route(HeadphonesFirst, h.clone(), d.clone(), true), Route::To(headphones()));
    assert_eq!(choose_route(HeadphonesFirst, None, d.clone(), true), Route::To(speakers()));
    assert!(matches!(choose_route(HeadphonesFirst, None, d.clone(), false), Route::Nowhere(_)));
    // Told to avoid the headphones, their output is not used even if listed.
    assert_eq!(choose_route(NotTheHeadphones, h.clone(), d.clone(), true), Route::To(speakers()));
    assert!(matches!(choose_route(NotTheHeadphones, h, d, false), Route::Nowhere(_)));
    assert!(matches!(choose_route(NotTheHeadphones, None, None, true), Route::Nowhere(_)));
}

// --- mode announcements ------------------------------------------------------

#[test]
fn a_confirmed_change_to_quiet_is_spoken() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Aware);
    r.snapshot(NoiseControlMode::Quiet);
    r.settle();
    assert_eq!(r.spoken(), vec![QUIET]);
}

#[test]
fn a_confirmed_change_to_aware_is_spoken() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.settle();
    assert_eq!(r.spoken(), vec![AWARE]);
}

/// The headphones are in *some* mode when the application first looks. That
/// is not a change.
#[test]
fn the_mode_found_at_startup_is_not_announced() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Quiet);
    r.settle();
    assert!(r.spoken().is_empty());
}

/// A change that was asked for but never confirmed: every snapshot still
/// shows the old mode, because that is what the headphones report. Nothing
/// has happened, so nothing is said.
#[test]
fn a_change_the_headphones_did_not_confirm_is_not_announced() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Quiet);
    // …the user asks for Aware; the request fails or is never confirmed…
    for _ in 0..5 {
        r.snapshot(NoiseControlMode::Quiet);
    }
    // …or the read after it fails, and the snapshot has no mode at all.
    r.voice.observe_snapshot(&real_snapshot(None));
    r.settle();
    assert!(r.spoken().is_empty());
}

#[test]
fn asking_for_the_mode_already_active_announces_nothing() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Aware);
    r.snapshot(NoiseControlMode::Aware);
    r.settle();
    assert!(r.spoken().is_empty());
}

#[test]
fn the_same_change_seen_many_times_is_spoken_once() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Quiet);
    for _ in 0..10 {
        r.snapshot(NoiseControlMode::Aware);
    }
    r.settle();
    assert_eq!(r.spoken(), vec![AWARE]);
}

/// The simulated device changes mode too. That is not a hardware state, and
/// it is never spoken as one.
#[test]
fn a_simulated_mode_change_is_never_announced() {
    let r = rig(|_| {});
    let simulated = |mode| DeviceSnapshot {
        source: DeviceSource::Mock,
        ..real_snapshot(Some(mode))
    };
    r.voice.observe_snapshot(&simulated(NoiseControlMode::Quiet));
    r.voice.observe_snapshot(&simulated(NoiseControlMode::Aware));
    r.settle();
    assert!(r.spoken().is_empty());

    // And switching from the simulation to real headphones is not a change.
    r.snapshot(NoiseControlMode::Quiet);
    r.settle();
    assert!(r.spoken().is_empty());
}

/// Quiet was confirmed and queued, but before it could be spoken the
/// headphones reported Aware again. "Quiet mode activated" is no longer true.
#[test]
fn a_mode_announcement_overtaken_by_events_is_dropped() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);

    r.snapshot(NoiseControlMode::Aware);
    r.snapshot(NoiseControlMode::Quiet); // queued behind the test
    r.snapshot(NoiseControlMode::Aware); // …and overtaken

    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE, AWARE]);
}

/// The same, but the mode moves on while the announcement is being
/// synthesized rather than while it waits.
#[test]
fn a_mode_that_moves_on_during_synthesis_is_not_spoken() {
    let r = rig(|f| f.hold_synth = true);
    r.snapshot(NoiseControlMode::Aware);
    r.snapshot(NoiseControlMode::Quiet);
    r.until("Quiet is being synthesized", |r| {
        r.voice.status().state == VoiceState::Synthesizing
    });
    // The headphones go away mid-synthesis.
    r.voice.observe_snapshot(&DeviceSnapshot::empty(DeviceSource::Real));
    r.fake.lock().hold_synth = false;
    r.settle();
    assert!(r.spoken().is_empty());
}

#[test]
fn the_mode_switch_silences_mode_announcements_only() {
    let r = rig_with(
        StoredSettings {
            announce_mode_changes: false,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.step(|s| s.low_battery = Some(12));
    r.settle();
    assert_eq!(r.spoken(), vec!["Battery low. 12 percent."]);
}

/// The headphones are asked for their mode in the background only when the
/// answer could be spoken: voice feedback on, mode announcements on, and a
/// model to speak with.
#[test]
fn background_mode_readings_are_wanted_only_when_they_would_be_spoken() {
    let r = rig(|_| {});
    assert!(r.voice.wants_mode_readings());

    r.set(|s| s.announce_mode_changes = false);
    assert!(!r.voice.wants_mode_readings());
    r.set(|s| s.announce_mode_changes = true);
    assert!(r.voice.wants_mode_readings());

    r.set(|s| s.voice_enabled = false);
    assert!(!r.voice.wants_mode_readings());

    let no_model = rig(|f| f.assets = Err("The voice model is not installed.".into()));
    assert!(!no_model.voice.wants_mode_readings());
}

// --- connection announcements ------------------------------------------------

#[test]
fn the_headphones_connecting_is_spoken_through_them() {
    let r = rig(|_| {});
    r.step(|s| {
        s.changed = true;
        s.returned = true;
    });
    r.settle();
    assert_eq!(
        r.plays(),
        vec![Play {
            text: CONNECTED.into(),
            output: "Headphones (Aurora)".into(),
            gain: gain_for(80),
            completed: true,
        }]
    );
}

/// Windows takes a moment to bring the headphones' audio output up after
/// they connect. The announcement waits for it rather than going elsewhere —
/// even with the fallback allowed, which is a last resort, not a shortcut.
#[test]
fn a_connection_announcement_waits_for_the_headphones_output() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |f| f.headphones_after = 3,
    );
    r.step(|s| s.returned = true);
    r.settle();
    assert_eq!(r.plays()[0].output, "Headphones (Aurora)");
    assert!(r.fake.lock().routes.len() >= 4, "it asked until the output was there");
}

#[test]
fn if_the_output_never_appears_the_wait_ends() {
    let r = rig(|f| f.headphones = None);
    let started = Instant::now();
    r.step(|s| s.returned = true);
    r.settle();
    assert!(r.plays().is_empty());
    assert_eq!(r.voice.status().state, VoiceState::Unavailable);
    assert!(started.elapsed() < Duration::from_secs(4), "bounded");
}

/// "Aurora disconnected" cannot be spoken through Aurora. Without the
/// fallback it is not spoken at all, and the status says why.
#[test]
fn a_disconnection_is_never_spoken_into_the_headphones_that_left() {
    let r = rig(|_| {}); // their output is still listed, as it can be briefly
    r.step(|s| {
        s.changed = true;
        s.lost = true;
    });
    r.settle();
    assert!(r.plays().is_empty());
    let status = r.voice.status();
    assert_eq!(status.state, VoiceState::Unavailable);
    assert!(status.detail.contains("headphones are gone"), "{}", status.detail);
    assert_eq!(r.fake.lock().routes, vec![(Via::NotTheHeadphones, false)]);
}

#[test]
fn with_the_fallback_on_a_disconnection_is_spoken_through_the_default_output() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.step(|s| s.lost = true);
    r.settle();
    assert_eq!(
        r.plays(),
        vec![Play {
            text: DISCONNECTED.into(),
            output: "Speakers".into(),
            gain: gain_for(80),
            completed: true,
        }]
    );
}

/// The watcher reports each real change once; the worker says each once.
#[test]
fn reconnecting_after_a_disconnection_is_announced_once() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.step(|s| s.lost = true);
    r.settle();
    r.step(|s| s.returned = true);
    r.settle();
    // Looks that find nothing changed say nothing.
    for _ in 0..5 {
        r.step(|_| {});
    }
    r.settle();
    assert_eq!(r.spoken(), vec![DISCONNECTED, CONNECTED]);
}

/// Dropped and back before anything could be said — a machine waking up,
/// say. Only where things ended up is announced.
#[test]
fn a_drop_and_return_in_quick_succession_is_one_announcement() {
    let r = rig_with(
        StoredSettings {
            voice_use_default_output: true,
            ..settings_on()
        },
        fast(),
        |f| f.hold_play = true,
    );
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.lost = true);
    r.step(|s| s.returned = true);
    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE, CONNECTED]);
}

#[test]
fn a_disconnection_stops_an_announcement_being_spoken_into_the_headphones() {
    let r = rig(|f| f.hold_play = true);
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.until("Aware is playing", |r| r.voice.status().state == VoiceState::Playing);

    r.step(|s| s.lost = true);
    r.settle();
    let plays = r.plays();
    assert_eq!(plays.len(), 1);
    assert!(!plays[0].completed, "cut off, not played out to nobody");
}

#[test]
fn a_disconnection_forgets_the_mode_so_reconnecting_announces_none() {
    let r = rig(|_| {});
    r.snapshot(NoiseControlMode::Quiet);
    r.step(|s| s.lost = true);
    r.settle();
    // Back, and in a different mode than before: found, not changed.
    r.snapshot(NoiseControlMode::Aware);
    r.settle();
    assert!(r.spoken().iter().all(|s| s != AWARE));
}

#[test]
fn the_connection_switch_silences_connection_announcements_only() {
    let r = rig_with(
        StoredSettings {
            announce_connection: false,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.step(|s| s.returned = true);
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.settle();
    assert_eq!(r.spoken(), vec![AWARE]);
}

// --- battery -----------------------------------------------------------------

#[test]
fn a_low_battery_is_spoken_with_the_level_windows_reported() {
    let r = rig(|_| {});
    r.step(|s| s.low_battery = Some(18));
    r.settle();
    assert_eq!(r.spoken(), vec!["Battery low. 18 percent."]);
}

/// The watcher reports a dip once; a look with nothing new in it adds nothing.
#[test]
fn looks_with_no_battery_news_say_nothing() {
    let r = rig(|_| {});
    r.step(|s| s.low_battery = Some(18));
    for _ in 0..10 {
        r.step(|_| {});
    }
    r.settle();
    assert_eq!(r.spoken().len(), 1);
}

#[test]
fn the_battery_switch_silences_battery_announcements_only() {
    let r = rig_with(
        StoredSettings {
            announce_battery: false,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.step(|s| s.low_battery = Some(10));
    r.step(|s| s.returned = true);
    r.settle();
    assert_eq!(r.spoken(), vec![CONNECTED]);
}

// --- the master switch -------------------------------------------------------

#[test]
fn with_voice_feedback_off_nothing_automatic_is_spoken() {
    let r = rig_with(
        StoredSettings {
            voice_enabled: false,
            ..settings_on()
        },
        fast(),
        |_| {},
    );
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.step(|s| {
        s.returned = true;
        s.low_battery = Some(5);
    });
    r.settle();
    assert!(r.spoken().is_empty());
    assert!(r.fake.lock().synths.is_empty(), "nor synthesized in the background");
}

#[test]
fn switching_voice_feedback_off_stops_speech_and_drops_what_was_waiting() {
    let r = rig(|f| f.hold_play = true);
    r.snapshot(NoiseControlMode::Quiet);
    r.snapshot(NoiseControlMode::Aware);
    r.until("Aware is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.low_battery = Some(8)); // waiting behind it

    r.set(|s| s.voice_enabled = false);
    r.settle();

    let plays = r.plays();
    assert_eq!(plays.len(), 1, "the battery warning was dropped");
    assert!(!plays[0].completed, "and the one being spoken was stopped");

    // New events are ignored from that moment.
    r.fake.lock().hold_play = false;
    r.snapshot(NoiseControlMode::Quiet);
    r.settle();
    assert_eq!(r.plays().len(), 1);
}

/// A test the user asked for is not an automatic announcement, and is not
/// cut off by switching those off.
#[test]
fn switching_voice_feedback_off_does_not_cut_off_a_test() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.set(|s| s.voice_enabled = false);
    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

#[test]
fn switching_a_category_off_drops_its_waiting_announcements() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.low_battery = Some(8));
    r.set(|s| s.announce_battery = false);
    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

// --- order, and one at a time ------------------------------------------------

#[test]
fn what_waits_is_spoken_most_important_first() {
    let r = rig(|f| f.hold_play = true);
    r.snapshot(NoiseControlMode::Quiet);
    r.voice.test().unwrap();
    r.until("the first test is playing", |r| r.voice.status().state == VoiceState::Playing);

    // Arriving least important first.
    r.voice.test().unwrap();
    r.step(|s| s.low_battery = Some(14));
    r.snapshot(NoiseControlMode::Aware);
    r.step(|s| s.returned = true);

    r.fake.lock().hold_play = false;
    r.settle();
    assert_eq!(
        r.spoken(),
        vec![TEST_PHRASE, CONNECTED, AWARE, "Battery low. 14 percent.", TEST_PHRASE]
    );
}

/// Speech already under way is not cut off by something more important
/// arriving; that waits its turn at the front of the line. Only the
/// headphones leaving, an explicit stop, or switching voice feedback off
/// interrupts.
#[test]
fn a_more_important_announcement_does_not_interrupt_one_being_spoken() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.returned = true);
    std::thread::sleep(Duration::from_millis(30));
    assert!(r.plays().is_empty(), "the test is still going");

    r.fake.lock().hold_play = false;
    r.settle();
    let plays = r.plays();
    assert!(plays[0].completed && plays[0].text == TEST_PHRASE);
    assert_eq!(plays[1].text, CONNECTED);
}

#[test]
fn only_one_thing_is_ever_being_said() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.returned = true);
    r.step(|s| s.low_battery = Some(3));
    std::thread::sleep(Duration::from_millis(30));
    // Two more are waiting, and neither has started.
    assert_eq!(r.fake.lock().synths.len(), 1);
    assert_eq!(r.voice.inner.shared.lock().queue.len(), 2);
}

// --- stopping, and shutting down ---------------------------------------------

#[test]
fn stop_speaking_cuts_off_speech_and_empties_the_line() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);
    r.step(|s| s.returned = true);
    r.step(|s| s.low_battery = Some(3));

    r.voice.stop();
    r.settle();
    let plays = r.plays();
    assert_eq!(plays.len(), 1);
    assert!(!plays[0].completed);
    assert_eq!(r.voice.status().state, VoiceState::Ready, "stopping is not an error");

    // And it still works afterwards.
    r.fake.lock().hold_play = false;
    r.voice.test().unwrap();
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

#[test]
fn stop_during_synthesis_means_nothing_is_played() {
    let r = rig(|f| f.hold_synth = true);
    r.voice.test().unwrap();
    r.until("it is synthesizing", |r| r.voice.status().state == VoiceState::Synthesizing);
    r.voice.stop();
    r.settle();
    assert!(r.plays().is_empty());
    assert_eq!(r.voice.status().state, VoiceState::Ready);
}

#[test]
fn shutting_down_mid_sentence_ends_promptly_and_releases_the_model() {
    let r = rig(|f| f.hold_play = true);
    r.voice.test().unwrap();
    r.until("the test is playing", |r| r.voice.status().state == VoiceState::Playing);

    let started = Instant::now();
    r.voice.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(r.voice.inner.shared.lock().finished);
    let f = r.fake.lock();
    assert!(!f.loaded, "the model was released");
    assert_eq!(f.unloads, 1);
}

#[test]
fn shutting_down_during_synthesis_does_not_hang() {
    let r = rig(|f| f.hold_synth = true);
    r.voice.test().unwrap();
    r.until("it is synthesizing", |r| r.voice.status().state == VoiceState::Synthesizing);
    let started = Instant::now();
    r.voice.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(r.plays().is_empty());
}

#[test]
fn nothing_is_accepted_after_shutdown() {
    let r = rig(|_| {});
    r.voice.shutdown();
    let _ = r.voice.test();
    r.step(|s| s.returned = true);
    std::thread::sleep(Duration::from_millis(30));
    assert!(r.plays().is_empty());
}

// --- synthesizing ahead ------------------------------------------------------

fn preparing() -> Timing {
    Timing {
        prepare_ahead: true,
        ..fast()
    }
}

/// With voice feedback on, the fixed sentences are synthesized while idle,
/// so that the first real announcement does not arrive seconds late.
#[test]
fn the_fixed_sentences_are_made_ready_ahead_of_need() {
    let r = rig_with(settings_on(), preparing(), |_| {});
    r.settle();
    let mut ready: Vec<String> = r.fake.lock().synths.iter().map(|s| s.0.clone()).collect();
    ready.sort();
    assert_eq!(ready, vec![CONNECTED, DISCONNECTED, AWARE, QUIET]);
    assert!(r.plays().is_empty(), "prepared, not spoken");

    // The real thing then needs no synthesis at all.
    r.snapshot(NoiseControlMode::Aware);
    r.snapshot(NoiseControlMode::Quiet);
    r.settle();
    assert_eq!(r.spoken(), vec![QUIET]);
    assert_eq!(r.fake.lock().synths.len(), 4);
}

/// Making a sentence ready is not saying it. While that goes on the status
/// names no sentence: "Quiet mode activated." on screen would read as an
/// announcement of something that has not happened.
#[test]
fn preparing_ahead_is_never_reported_as_an_announcement() {
    let r = rig_with(settings_on(), preparing(), |_| {});
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 4, "the sentences were prepared");

    let statuses = r.statuses.lock();
    let preparing: Vec<_> = statuses
        .iter()
        .filter(|s| s.state == VoiceState::Synthesizing)
        .collect();
    assert!(!preparing.is_empty(), "preparation was visible in the status");
    for status in statuses.iter() {
        assert_eq!(status.speaking, None, "{status:?}");
        assert!(!status.is_test);
        assert_ne!(status.state, VoiceState::Playing);
    }
    assert!(preparing.iter().all(|s| s.detail.contains("ready")));
    assert_eq!(statuses.last().unwrap().state, VoiceState::Ready);
}

#[test]
fn nothing_is_prepared_while_voice_feedback_is_off() {
    let r = rig_with(
        StoredSettings {
            voice_enabled: false,
            ..settings_on()
        },
        preparing(),
        |_| {},
    );
    r.settle();
    assert!(r.fake.lock().synths.is_empty());
    assert_eq!(r.fake.lock().loads, 0, "the model is not even loaded");

    // Switching it on is what starts the preparation.
    r.set(|s| s.voice_enabled = true);
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 4);
}

#[test]
fn a_new_voice_is_prepared_and_switched_off_categories_are_not() {
    let r = rig_with(
        StoredSettings {
            announce_connection: false,
            ..settings_on()
        },
        preparing(),
        |_| {},
    );
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 2, "only the two mode sentences");

    r.set(|s| s.voice_id = "bm_lewis".into());
    r.settle();
    let f = r.fake.lock();
    assert_eq!(f.synths.len(), 4);
    assert!(f.synths[2..].iter().all(|s| s.1 == "bm_lewis"));
}

/// Preparing is never worth delaying a real announcement for.
#[test]
fn a_real_announcement_is_not_kept_waiting_behind_preparation() {
    let r = rig_with(settings_on(), preparing(), |f| f.hold_synth = true);
    r.until("preparation has started", |r| !r.fake.lock().synths.is_empty());

    r.voice.test().unwrap();
    r.until("the test has been taken up", |r| {
        r.fake.lock().synths.iter().any(|s| s.0 == TEST_PHRASE)
    });
    r.fake.lock().hold_synth = false;
    r.settle();
    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
}

/// The headphones' custom mode is announced as often as Quiet and Aware are,
/// and is made ready the same way — once the headphones have said it exists.
#[test]
fn a_custom_mode_the_headphones_listed_is_made_ready_ahead_of_need() {
    let r = rig_with(settings_on(), preparing(), |_| {});
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 4);

    r.voice.expect_modes(&["Focus".to_string()]);
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 5);
    assert_eq!(r.fake.lock().synths[4].0, FOCUS);
    assert!(r.plays().is_empty(), "prepared, not spoken: naming a mode is not being in it");

    // Being told again changes nothing.
    r.voice.expect_modes(&["Focus".to_string(), " Focus ".to_string(), String::new()]);
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 5);

    // And the switch into it then needs no synthesis.
    r.snapshot(NoiseControlMode::Quiet);
    r.voice.observe_snapshot(&custom_snapshot("Focus"));
    r.settle();
    assert_eq!(r.spoken(), vec![FOCUS]);
    assert_eq!(r.fake.lock().synths.len(), 5);
}

#[test]
fn a_custom_mode_is_not_made_ready_when_it_would_not_be_announced() {
    let r = rig_with(
        StoredSettings {
            announce_mode_changes: false,
            ..settings_on()
        },
        preparing(),
        |_| {},
    );
    r.settle();
    let before = r.fake.lock().synths.len();
    r.voice.expect_modes(&["Focus".to_string()]);
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), before);

    // It is remembered, though, and made ready when announcements go on.
    r.set(|s| s.announce_mode_changes = true);
    r.settle();
    assert!(r.fake.lock().synths.iter().any(|s| s.0 == FOCUS));
}

/// A voice change makes the custom mode's sentence again along with the rest.
#[test]
fn a_new_voice_makes_the_custom_mode_ready_too() {
    let r = rig_with(settings_on(), preparing(), |_| {});
    r.voice.expect_modes(&["Focus".to_string()]);
    r.settle();
    r.set(|s| s.voice_id = "bm_lewis".into());
    r.settle();
    let f = r.fake.lock();
    assert!(f.synths.iter().any(|s| s.0 == FOCUS && s.1 == "bm_lewis"));
}

/// A sentence set aside so that an announcement could go first is not lost:
/// it is made ready once the announcement is out. It used to be dropped,
/// and then the first real use of it was late.
#[test]
fn a_preparation_set_aside_for_an_announcement_is_taken_up_again() {
    let r = rig_with(settings_on(), preparing(), |f| f.hold_synth = true);
    r.until("preparation has started", |r| !r.fake.lock().synths.is_empty());
    let interrupted = r.fake.lock().synths[0].0.clone();

    r.voice.test().unwrap();
    r.until("the test has been taken up", |r| {
        r.fake.lock().synths.iter().any(|s| s.0 == TEST_PHRASE)
    });
    r.fake.lock().hold_synth = false;
    r.settle();

    assert_eq!(r.spoken(), vec![TEST_PHRASE]);
    let f = r.fake.lock();
    let attempts = f.synths.iter().filter(|s| s.0 == interrupted).count();
    assert_eq!(attempts, 2, "started, set aside, then finished");
    for sentence in [QUIET, AWARE, CONNECTED, DISCONNECTED] {
        assert!(f.synths.iter().any(|s| s.0 == sentence), "{sentence} was made ready");
    }
}

/// Stop means stop. What was being prepared is not quietly started again.
#[test]
fn stop_speaking_does_not_take_preparation_up_again() {
    let r = rig_with(settings_on(), preparing(), |f| f.hold_synth = true);
    r.until("preparation has started", |r| !r.fake.lock().synths.is_empty());
    r.voice.stop();
    r.fake.lock().hold_synth = false;
    r.settle();
    assert_eq!(r.fake.lock().synths.len(), 1, "only the attempt that was stopped");
}

// --- sentences kept on disk ---------------------------------------------------

/// The point of keeping them: the second time the application runs, the
/// usual sentences are said without the model ever being loaded.
#[test]
fn a_sentence_kept_on_disk_is_said_without_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let clips = {
        let first = rig_keeping(settings_on(), fast(), Some(dir.path()), |_| {});
        first.snapshot(NoiseControlMode::Aware);
        first.snapshot(NoiseControlMode::Quiet);
        first.settle();
        assert_eq!(first.spoken(), vec![QUIET]);
        assert_eq!(first.fake.lock().loads, 1);
        let clips = first.fake.lock().clips.clone();
        clips
    };

    // The application starts again: nothing in memory, the files still there.
    let second = rig_keeping(settings_on(), fast(), Some(dir.path()), |f| f.clips = clips);
    second.snapshot(NoiseControlMode::Aware);
    second.snapshot(NoiseControlMode::Quiet);
    second.settle();
    assert_eq!(second.spoken(), vec![QUIET]);
    let f = second.fake.lock();
    assert!(f.synths.is_empty(), "nothing was synthesized");
    assert_eq!(f.loads, 0, "the model was never loaded");
    assert!(!second.voice.status().model_loaded);
}

/// Preparing ahead on a later run is reading a few small files.
#[test]
fn preparing_ahead_from_disk_does_not_load_the_model() {
    let dir = tempfile::tempdir().unwrap();
    {
        let first = rig_keeping(settings_on(), preparing(), Some(dir.path()), |_| {});
        first.settle();
        assert_eq!(first.fake.lock().synths.len(), 4);
    }
    let second = rig_keeping(settings_on(), preparing(), Some(dir.path()), |_| {});
    second.settle();
    assert!(second.fake.lock().synths.is_empty());
    assert_eq!(second.fake.lock().loads, 0);
    // Nothing on screen claimed to be synthesizing, either.
    assert!(!second.states().contains(&VoiceState::Synthesizing));
}

/// Another voice, or another speed, is other audio and is made afresh.
#[test]
fn a_kept_sentence_is_only_used_for_the_voice_and_speed_it_was_made_in() {
    let dir = tempfile::tempdir().unwrap();
    {
        let first = rig_keeping(settings_on(), fast(), Some(dir.path()), |_| {});
        first.voice.test().unwrap();
        first.settle();
    }
    let other_voice = StoredSettings {
        voice_id: "am_adam".into(),
        ..settings_on()
    };
    let second = rig_keeping(other_voice, fast(), Some(dir.path()), |_| {});
    second.voice.test().unwrap();
    second.settle();
    assert_eq!(second.fake.lock().synths.len(), 1);
    assert_eq!(second.fake.lock().synths[0].1, "am_adam");
}

/// A kept file that has been damaged is not played. The sentence is made
/// again, as if it had never been kept.
#[test]
fn a_damaged_kept_sentence_is_synthesized_again() {
    let dir = tempfile::tempdir().unwrap();
    {
        let first = rig_keeping(settings_on(), fast(), Some(dir.path()), |_| {});
        first.voice.test().unwrap();
        first.settle();
    }
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 40;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, bytes).unwrap();
    }
    let second = rig_keeping(settings_on(), fast(), Some(dir.path()), |_| {});
    second.voice.test().unwrap();
    second.settle();
    assert_eq!(second.spoken(), vec![TEST_PHRASE]);
    assert_eq!(second.fake.lock().synths.len(), 1, "made again");
}

// --- which phone or computer ---------------------------------------------------

/// Once this machine's name is known, a connection is announced as a
/// connection to it.
#[test]
fn a_connection_names_the_machine_the_headphones_connected_to() {
    let r = rig(|_| {});
    r.voice.set_host(Some("Galaxy S24 Ultra".into()));
    r.step(|s| {
        s.changed = true;
        s.returned = true;
    });
    r.settle();
    assert_eq!(r.spoken(), vec!["Aurora connected to Galaxy S24 Ultra."]);
}

/// A name the system gave in a form nobody would say is said as words, and
/// one with nothing sayable in it is left out altogether.
#[test]
fn a_host_name_is_spoken_as_words_or_not_at_all() {
    let r = rig(|_| {});
    r.voice.set_host(Some("DESKTOP-8A3F2K1".into()));
    r.step(|s| s.returned = true);
    r.settle();
    r.voice.set_host(Some("---".into()));
    r.step(|s| s.lost = true);
    r.step(|s| s.returned = true);
    r.settle();
    assert_eq!(
        r.spoken(),
        vec!["Aurora connected to DESKTOP 8A3F2K1.", CONNECTED]
    );
}

/// The sentence made ready ahead is the one that will be said.
#[test]
fn the_connection_sentence_made_ready_is_the_one_with_the_machines_name() {
    let r = rig_with(settings_on(), preparing(), |_| {});
    r.settle();
    r.voice.set_host(Some("Nemesis".into()));
    r.settle();
    let made = r.fake.lock().synths.len();
    assert!(r
        .fake
        .lock()
        .synths
        .iter()
        .any(|s| s.0 == "Aurora connected to Nemesis."));

    r.step(|s| s.returned = true);
    r.settle();
    assert_eq!(r.spoken(), vec!["Aurora connected to Nemesis."]);
    assert_eq!(r.fake.lock().synths.len(), made, "nothing more to synthesize");
}
