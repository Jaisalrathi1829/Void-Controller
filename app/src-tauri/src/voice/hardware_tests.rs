//! The voice service against the real model, the real Windows audio output
//! and — where a test says so — the real headphones.
//!
//! Nothing here is substituted. The backend is the one the application uses,
//! wrapped only so that what it was asked to do, and what Windows reported
//! back, can be read afterwards.
//!
//! Ignored by default: they need the voice model installed and the headphones
//! connected, and they make sound. Run them one at a time:
//!
//! ```text
//! cargo test --lib -- --ignored --nocapture --test-threads=1 voice_hw
//! ```
//!
//! The names deliberately avoid `hw_a`, `hw_b` and `hw_c`: those are the
//! filters the control link's own hardware tests are run by, and some of
//! those runs are meant to be read-only.
//!
//! What these can show is that speech was synthesized by Kokoro and rendered
//! to the headphones' Windows output, whose own level meter moved. What they
//! cannot show is that a person heard it.

use super::backend::KokoroBackend;
use super::*;
use crate::bose::real::{sight_device, RealBoseDevice};
use crate::device::state::{ConnectionState, NoiseControlMode};
use crate::device::traits::BoseDevice;
use std::path::PathBuf;

/// One synthesis the real model performed.
#[derive(Debug, Clone)]
struct Synthesis {
    text: String,
    voice: &'static str,
    speed: f32,
    seconds_of_audio: f32,
    took: Duration,
}

/// One playback Windows performed.
#[derive(Debug, Clone)]
struct Playback {
    output: String,
    gain: f32,
    played: Played,
    took: Duration,
}

#[derive(Default)]
struct Witness {
    loads: Vec<Duration>,
    unloads: usize,
    syntheses: Vec<Synthesis>,
    playbacks: Vec<Playback>,
}

/// The real backend, with a note taken of everything it did.
struct Witnessed {
    real: KokoroBackend,
    seen: Arc<Mutex<Witness>>,
}

impl Backend for Witnessed {
    fn assets_dir(&self) -> String {
        self.real.assets_dir()
    }
    fn check_assets(&mut self) -> Result<(), String> {
        self.real.check_assets()
    }
    fn load(&mut self) -> Result<(), String> {
        let was_loaded = self.real.is_loaded();
        let started = Instant::now();
        let result = self.real.load();
        if !was_loaded && result.is_ok() {
            self.seen.lock().loads.push(started.elapsed());
        }
        result
    }
    fn is_loaded(&self) -> bool {
        self.real.is_loaded()
    }
    fn unload(&mut self) {
        if self.real.is_loaded() {
            self.seen.lock().unloads += 1;
        }
        self.real.unload();
    }
    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError> {
        let started = Instant::now();
        let speech = self.real.synthesize(text, voice, speed, cancel)?;
        self.seen.lock().syntheses.push(Synthesis {
            text: text.to_string(),
            voice: voice.id,
            speed,
            seconds_of_audio: speech.samples.len() as f32 / speech.sample_rate as f32,
            took: started.elapsed(),
        });
        Ok(speech)
    }
    fn route(&mut self, via: Via, allow_default: bool) -> Route {
        self.real.route(via, allow_default)
    }
    fn play(
        &mut self,
        output: &Output,
        speech: &Speech,
        gain: f32,
        stop: &AtomicBool,
    ) -> Result<Played, String> {
        let started = Instant::now();
        let played = self.real.play(output, speech, gain, stop)?;
        self.seen.lock().playbacks.push(Playback {
            output: output.name.clone(),
            gain,
            played: played.clone(),
            took: started.elapsed(),
        });
        Ok(played)
    }
}

struct Rig {
    voice: VoiceFeedback,
    store: Arc<SettingsStore>,
    seen: Arc<Mutex<Witness>>,
    statuses: Arc<Mutex<Vec<(Instant, VoiceStatus)>>>,
    /// The name of the headphones' own Windows output.
    headphones_output: String,
}

fn voice_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .expect("LOCALAPPDATA")
        .join("local.bosecontrolcenter.app")
        .join("voice")
}

/// Starts the real service, pointed at the paired headphones.
fn rig(timing: Timing, adjust: impl FnOnce(&mut StoredSettings)) -> Rig {
    let headphones = sight_device(None).expect("a paired Bose device");
    assert!(headphones.connected, "connect the headphones first");

    let container = crate::bluetooth::pnp::container_id(&headphones.instance_id)
        .expect("the headphones have a container id");
    let output = crate::audio::playback::output_for_device(&container)
        .expect("outputs can be listed")
        .expect("the headphones have an active output");
    println!("\n  headphones        : {}", headphones.name);
    println!("  their output      : {}", output.name);
    println!(
        "  Windows default   : {:?}",
        crate::audio::playback::default_output()
            .ok()
            .flatten()
            .map(|o| o.name)
    );

    let mut settings = StoredSettings {
        preferred_device_id: Some(headphones.instance_id.clone()),
        preferred_device_name: Some(headphones.name.clone()),
        voice_enabled: true,
        ..StoredSettings::default()
    };
    adjust(&mut settings);
    let store = Arc::new(SettingsStore::in_memory(settings));

    let seen = Arc::new(Mutex::new(Witness::default()));
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&statuses);
    let voice = VoiceFeedback::start(
        Arc::clone(&store),
        Box::new(Witnessed {
            real: KokoroBackend::new(voice_root(), Arc::clone(&store)),
            seen: Arc::clone(&seen),
        }),
        timing,
        move |status| sink.lock().push((Instant::now(), status.clone())),
    );

    let rig = Rig {
        voice,
        store,
        seen,
        statuses,
        headphones_output: output.name,
    };
    // Checked means either the model's voices are on offer or it was found
    // wanting. (The state alone will not do: with sentences being made ready
    // ahead, the service is busy the moment the check passes.)
    rig.until("the installed model has been checked", 30, |r| {
        let status = r.voice.status();
        !status.voices.is_empty() || status.state == VoiceState::NotInstalled
    });
    let status = rig.voice.status();
    assert!(
        !status.voices.is_empty(),
        "{} Run tools/install-voice-model.ps1 first.",
        status.detail
    );
    rig
}

/// The application's timing, without synthesizing ahead — so that what is
/// measured is what was asked for.
fn on_demand() -> Timing {
    Timing {
        prepare_ahead: false,
        ..Timing::standard()
    }
}

impl Rig {
    fn until(&self, what: &str, seconds: u64, done: impl Fn(&Rig) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn playbacks(&self) -> Vec<Playback> {
        self.seen.lock().playbacks.clone()
    }

    fn syntheses(&self) -> Vec<Synthesis> {
        self.seen.lock().syntheses.clone()
    }

    /// Waits for the `n`th playback to finish and the service to say Ready.
    fn spoken(&self, n: usize) -> Playback {
        self.until("it has been spoken", 60, |r| {
            r.playbacks().len() >= n && r.voice.status().state == VoiceState::Ready
        });
        self.playbacks()[n - 1].clone()
    }

    fn set(&self, change: impl FnOnce(&mut StoredSettings)) {
        let before = self.store.get();
        let after = self.store.update(change).unwrap();
        self.voice.settings_changed(&before, &after);
    }

    /// When the service first reported `state` at or after `since`.
    fn first(&self, state: VoiceState, since: Instant) -> Option<Instant> {
        self.statuses
            .lock()
            .iter()
            .find(|(at, status)| *at >= since && status.state == state)
            .map(|(at, _)| *at)
    }

    fn states_since(&self, since: Instant) -> Vec<VoiceState> {
        let mut states: Vec<VoiceState> = self
            .statuses
            .lock()
            .iter()
            .filter(|(at, _)| *at >= since)
            .map(|(_, status)| status.state)
            .collect();
        states.dedup();
        states
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.voice.shutdown();
    }
}

/// This process's working set, in megabytes, as Windows reports it.
fn working_set_mb() -> Option<f64> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", std::process::id()), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "name.exe","1234","Console","1","123,456 K"
    let field = text.trim().rsplit("\",\"").next()?;
    let kilobytes: String = field.chars().filter(char::is_ascii_digit).collect();
    kilobytes.parse::<f64>().ok().map(|kb| kb / 1024.0)
}

/// Test Voice, end to end: the button's request, the real model, the
/// headphones' own Windows output.
#[test]
#[ignore = "needs the voice model and the connected headphones; makes sound"]
fn voice_hw_test_voice_reaches_the_headphones() {
    // Voice feedback itself is off: Test Voice has to work before it is on.
    let r = rig(on_demand(), |s| s.voice_enabled = false);

    // --- cold: the model is not loaded -------------------------------------
    let asked = Instant::now();
    r.voice.test().expect("the test is accepted");
    let first = r.spoken(1);
    let cold_to_sound = r.first(VoiceState::Playing, asked).expect("it played") - asked;

    let states = r.states_since(asked);
    println!("\n  states, cold      : {states:?}");
    assert_eq!(
        states,
        vec![
            VoiceState::Initializing,
            VoiceState::Synthesizing,
            VoiceState::Playing,
            VoiceState::Ready
        ]
    );
    // While it spoke, the status said this was a test and named the output.
    let playing = r
        .statuses
        .lock()
        .iter()
        .find(|(_, s)| s.state == VoiceState::Playing)
        .map(|(_, s)| s.clone())
        .unwrap();
    assert!(playing.is_test);
    assert_eq!(playing.speaking.as_deref(), Some(policy::TEST_PHRASE));
    assert_eq!(playing.output.as_deref(), Some(r.headphones_output.as_str()));

    let synth = r.syntheses();
    assert_eq!(synth.len(), 1);
    assert_eq!(synth[0].text, policy::TEST_PHRASE);
    assert_eq!(synth[0].voice, assets::DEFAULT_VOICE);
    assert!(synth[0].seconds_of_audio > 0.8, "{:?}", synth[0]);

    assert_eq!(first.output, r.headphones_output, "spoken through the headphones");
    assert!(first.played.completed);
    assert!(
        first.played.endpoint_peak > 0.01,
        "the headphones' output meter did not move: {:?}",
        first.played
    );

    let load = r.seen.lock().loads[0];
    println!("  model load        : {:.0} ms", load.as_secs_f64() * 1000.0);
    println!(
        "  synthesis         : {:.0} ms for {:.2} s of audio",
        synth[0].took.as_secs_f64() * 1000.0,
        synth[0].seconds_of_audio
    );
    println!("  request to sound  : {:.0} ms (cold)", cold_to_sound.as_secs_f64() * 1000.0);
    println!("  endpoint meter    : {:.4} at gain {:.2}", first.played.endpoint_peak, first.gain);

    // --- again: nothing is synthesized twice --------------------------------
    let asked = Instant::now();
    r.voice.test().unwrap();
    let second = r.spoken(2);
    let warm_to_sound = r.first(VoiceState::Playing, asked).expect("it played") - asked;
    assert_eq!(r.syntheses().len(), 1, "the same sentence was reused, not made again");
    assert_eq!(r.seen.lock().loads.len(), 1, "the model was loaded once");
    assert!(second.played.completed);
    println!("  request to sound  : {:.0} ms (repeat)", warm_to_sound.as_secs_f64() * 1000.0);

    // --- a different voice ----------------------------------------------------
    r.set(|s| s.voice_id = "am_adam".into());
    r.voice.test().unwrap();
    r.spoken(3);
    let synth = r.syntheses();
    assert_eq!(synth.len(), 2);
    assert_eq!(synth[1].voice, "am_adam");

    // --- speed ------------------------------------------------------------------
    r.set(|s| s.voice_speed_percent = 150);
    r.voice.test().unwrap();
    r.spoken(4);
    let synth = r.syntheses();
    assert_eq!(synth.len(), 3);
    assert!((synth[2].speed - 1.5).abs() < 1e-6);
    assert!(
        synth[2].seconds_of_audio < synth[1].seconds_of_audio * 0.85,
        "faster speech is shorter: {:.2} s vs {:.2} s",
        synth[2].seconds_of_audio,
        synth[1].seconds_of_audio
    );
    println!(
        "  am_adam at 1.0x / 1.5x: {:.2} s / {:.2} s",
        synth[1].seconds_of_audio, synth[2].seconds_of_audio
    );

    // --- volume: speech only, applied to the next thing said --------------------
    r.set(|s| s.voice_volume_percent = 30);
    r.voice.test().unwrap();
    let quiet = r.spoken(5);
    r.set(|s| s.voice_volume_percent = 100);
    r.voice.test().unwrap();
    let loud = r.spoken(6);
    assert_eq!(r.syntheses().len(), 3, "a volume change needs no new synthesis");
    assert!((quiet.gain - gain_for(30)).abs() < 1e-6);
    assert!((loud.gain - 1.0).abs() < 1e-6);
    assert!(
        loud.played.endpoint_peak > quiet.played.endpoint_peak * 3.0,
        "volume 100 against 30: meter {:.4} against {:.4}",
        loud.played.endpoint_peak,
        quiet.played.endpoint_peak
    );
    println!(
        "  meter at volume 30 / 100: {:.4} / {:.4}",
        quiet.played.endpoint_peak, loud.played.endpoint_peak
    );
    assert!(r.playbacks().iter().all(|p| p.output == r.headphones_output));
}

/// Stop Speaking, on real playback.
#[test]
#[ignore = "needs the voice model and the connected headphones; makes sound"]
fn voice_hw_stop_speaking_cuts_real_playback_short() {
    let r = rig(on_demand(), |s| s.voice_enabled = false);
    r.voice.test().unwrap();
    let whole = r.spoken(1);

    r.voice.test().unwrap();
    r.until("it is playing", 30, |r| r.voice.status().state == VoiceState::Playing);
    std::thread::sleep(Duration::from_millis(250));
    let stopped_at = Instant::now();
    r.voice.stop();
    r.until("it has stopped", 10, |r| r.voice.status().state == VoiceState::Ready);
    let took = stopped_at.elapsed();

    let cut = r.playbacks()[1].clone();
    println!(
        "\n  whole sentence    : {:.0} ms\n  cut short after   : {:.0} ms\n  stop took         : {:.0} ms",
        whole.took.as_secs_f64() * 1000.0,
        cut.took.as_secs_f64() * 1000.0,
        took.as_secs_f64() * 1000.0
    );
    assert!(!cut.played.completed, "it was cut off");
    assert!(cut.took < whole.took);
    assert!(took < Duration::from_millis(800), "stopping took {took:?}");
}

fn mode_in(snapshot: &DeviceSnapshot) -> NoiseControlMode {
    snapshot
        .noise_control
        .as_ref()
        .expect("the headphones reported a mode")
        .mode
}

async fn read(dev: &RealBoseDevice) -> DeviceSnapshot {
    let snapshot = dev.snapshot().await.expect("the headphones can be read");
    assert_eq!(
        snapshot.connection,
        ConnectionState::Connected,
        "connect the headphones first"
    );
    assert!(
        snapshot.noise_control.is_some(),
        "the mode could not be read ({:?}). Close the application first: only one program \
         can hold the headphones' control channel.",
        snapshot.last_error
    );
    snapshot
}

fn sentence_for(mode: NoiseControlMode) -> &'static str {
    match mode {
        NoiseControlMode::Quiet => "Quiet mode activated.",
        NoiseControlMode::Aware => "Aware mode activated.",
        other => panic!("not a mode this test switches to: {other:?}"),
    }
}

/// A real mode change, confirmed by the headphones, is what gets spoken —
/// and only that.
#[tokio::test]
#[ignore = "needs the model and the headphones; CHANGES the noise-control mode; makes sound"]
async fn voice_hw_mode_change_confirmed_is_spoken() {
    let r = rig(on_demand(), |_| {});
    let dev = RealBoseDevice::default();

    // Where the headphones stand to begin with is not news.
    let before = read(&dev).await;
    let initial = mode_in(&before);
    println!("\n  initial mode      : {initial:?}");
    r.voice.observe_snapshot(&before);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(r.playbacks().is_empty(), "the mode found at the start was announced");

    let (away, back) = match initial {
        NoiseControlMode::Aware => (NoiseControlMode::Quiet, NoiseControlMode::Aware),
        NoiseControlMode::Quiet => (NoiseControlMode::Aware, NoiseControlMode::Quiet),
        // Starting in a custom mode: visit both presets, ending in Quiet.
        _ => (NoiseControlMode::Aware, NoiseControlMode::Quiet),
    };

    for (n, target) in [away, back].into_iter().enumerate() {
        let asked = Instant::now();
        let outcome = dev.set_noise_control(target).await.expect("the request went out");
        println!("  requested {target:?}: {outcome:?}");

        // The request alone says nothing, whatever its outcome.
        assert_eq!(r.playbacks().len(), n, "something was spoken before any reading");

        assert!(outcome.is_confirmed(), "the headphones did not confirm {target:?}");
        let confirmed = read(&dev).await;
        assert_eq!(mode_in(&confirmed), target, "the headphones report the new mode");
        let reported = Instant::now();

        // The reading is what the service is given.
        r.voice.observe_snapshot(&confirmed);
        let spoken = r.spoken(n + 1);
        let sound = r.first(VoiceState::Playing, reported).expect("it played");

        let synth = r.syntheses();
        assert_eq!(synth[n].text, sentence_for(target));
        assert_eq!(spoken.output, r.headphones_output);
        assert!(spoken.played.completed);
        assert!(spoken.played.endpoint_peak > 0.01, "{:?}", spoken.played);
        println!(
            "    spoke {:?} through {} — request to confirmed {:.0} ms, confirmed to sound {:.0} ms, meter {:.4}",
            synth[n].text,
            spoken.output,
            (reported - asked).as_secs_f64() * 1000.0,
            (sound - reported).as_secs_f64() * 1000.0,
            spoken.played.endpoint_peak
        );

        // The same reading again is not a change.
        r.voice.observe_snapshot(&read(&dev).await);
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(r.playbacks().len(), n + 1, "an unchanged mode was announced again");
    }

    // Where the time went: this run started with no model loaded and nothing
    // made ready, which the application avoids for its fixed sentences.
    for load in &r.seen.lock().loads {
        println!("  model load        : {:.0} ms", load.as_secs_f64() * 1000.0);
    }
    for synth in r.syntheses() {
        println!(
            "  synthesis         : {:.0} ms for {:?}",
            synth.took.as_secs_f64() * 1000.0,
            synth.text
        );
    }

    // Asking for the mode the headphones are already in: nothing is sent, and
    // nothing is said.
    let outcome = dev.set_noise_control(back).await.expect("answered");
    println!("  requested {back:?} again: {outcome:?}");
    r.voice.observe_snapshot(&read(&dev).await);
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(r.playbacks().len(), 2, "re-selecting the active mode was announced");

    // With mode announcements switched off, a real change is silent.
    r.set(|s| s.announce_mode_changes = false);
    assert!(dev.set_noise_control(away).await.unwrap().is_confirmed());
    r.voice.observe_snapshot(&read(&dev).await);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(r.playbacks().len(), 2, "spoken with mode announcements off");

    // Put the headphones back the way they were found.
    if matches!(initial, NoiseControlMode::Quiet | NoiseControlMode::Aware) {
        assert!(dev.set_noise_control(initial).await.unwrap().is_confirmed());
        assert_eq!(mode_in(&read(&dev).await), initial);
        println!("  restored          : {initial:?}");
    }
}

/// As the application runs it. With voice feedback on, the fixed sentences
/// are made ready ahead of need, so a confirmed change is spoken without any
/// synthesis standing between the headphones' report and the sound.
#[tokio::test]
#[ignore = "needs the model and the headphones; CHANGES the noise-control mode; makes sound"]
async fn voice_hw_mode_change_prepared_is_spoken_at_once() {
    let r = rig(Timing::standard(), |_| {});
    // Quiet, Aware, connected, disconnected.
    r.until("the fixed sentences are ready", 90, |r| {
        r.syntheses().len() >= 4 && r.voice.status().state == VoiceState::Ready
    });
    let prepared = r.syntheses();
    println!(
        "\n  made ready ahead   : {:?}",
        prepared.iter().map(|s| s.text.as_str()).collect::<Vec<_>>()
    );
    assert!(r.playbacks().is_empty(), "prepared, not spoken");

    let dev = RealBoseDevice::default();
    let before = read(&dev).await;
    let initial = mode_in(&before);
    r.voice.observe_snapshot(&before);

    let (away, back) = match initial {
        NoiseControlMode::Aware => (NoiseControlMode::Quiet, NoiseControlMode::Aware),
        _ => (NoiseControlMode::Aware, NoiseControlMode::Quiet),
    };
    for (n, target) in [away, back].into_iter().enumerate() {
        assert!(dev.set_noise_control(target).await.unwrap().is_confirmed());
        let confirmed = read(&dev).await;
        assert_eq!(mode_in(&confirmed), target);
        let reported = Instant::now();

        r.voice.observe_snapshot(&confirmed);
        let spoken = r.spoken(n + 1);
        let sound = r.first(VoiceState::Playing, reported).expect("it played");

        assert_eq!(spoken.output, r.headphones_output);
        assert!(spoken.played.completed && spoken.played.endpoint_peak > 0.01);
        assert_eq!(
            r.syntheses().len(),
            prepared.len(),
            "nothing had to be synthesized for {target:?}"
        );
        println!(
            "  {target:?}: confirmed to sound {:.0} ms, meter {:.4}",
            (sound - reported).as_secs_f64() * 1000.0,
            spoken.played.endpoint_peak
        );
    }

    if matches!(initial, NoiseControlMode::Quiet | NoiseControlMode::Aware) {
        if mode_in(&read(&dev).await) != initial {
            assert!(dev.set_noise_control(initial).await.unwrap().is_confirmed());
        }
        assert_eq!(mode_in(&read(&dev).await), initial);
        println!("  restored          : {initial:?}");
    }
}

/// Measurements, taken rather than estimated. Asserts little: it exists to
/// print what this machine actually does.
#[test]
#[ignore = "needs the voice model and the connected headphones; makes sound"]
fn voice_hw_measurements() {
    let idle_unload = Duration::from_secs(3);
    let r = rig(
        Timing {
            idle_unload,
            prepare_ahead: false,
            ..Timing::standard()
        },
        |s| s.voice_enabled = false,
    );
    let baseline = working_set_mb();

    r.voice.test().unwrap();
    r.spoken(1);
    let loaded = working_set_mb();
    assert!(r.voice.status().model_loaded);

    // Left alone, the model is released.
    r.until("the idle model is released", 30, |r| !r.voice.status().model_loaded);
    std::thread::sleep(Duration::from_millis(500));
    let released = working_set_mb();
    assert_eq!(r.seen.lock().unloads, 1);

    // The sentence already made still plays, without the model.
    let asked = Instant::now();
    r.voice.test().unwrap();
    r.spoken(2);
    let cached = r.first(VoiceState::Playing, asked).unwrap() - asked;
    assert_eq!(r.seen.lock().loads.len(), 1, "no reload for a sentence already made");
    assert!(!r.voice.status().model_loaded);

    let load = r.seen.lock().loads[0];
    let synth = r.syntheses()[0].clone();
    println!("\n  model load                 : {:.0} ms", load.as_secs_f64() * 1000.0);
    println!(
        "  synthesis                  : {:.0} ms for {:.2} s of audio (x{:.2} real time)",
        synth.took.as_secs_f64() * 1000.0,
        synth.seconds_of_audio,
        synth.took.as_secs_f32() / synth.seconds_of_audio
    );
    println!("  cached sentence to sound   : {:.0} ms", cached.as_secs_f64() * 1000.0);
    let mb = |v: Option<f64>| v.map_or("unknown".to_string(), |v| format!("{v:.0} MB"));
    println!("  working set, model unloaded: {}", mb(baseline));
    println!("  working set, model loaded  : {}", mb(loaded));
    println!(
        "  working set, {} s idle later: {}",
        idle_unload.as_secs(),
        mb(released)
    );
}

/// What asking often costs: the time one question takes on the open link.
#[tokio::test]
#[ignore = "needs the connected headphones and the application closed; read-only"]
async fn voice_hw_peek_is_quick_enough_to_ask_often() {
    let dev = RealBoseDevice::default();
    let full = read(&dev).await;
    let mut took = Vec::new();
    for _ in 0..40 {
        let started = Instant::now();
        let mode = dev.peek_noise_control().await.expect("answered").expect("a mode");
        took.push(started.elapsed());
        assert_eq!(mode.mode, mode_in(&full));
        tokio::time::sleep(crate::watcher::MODE_INTERVAL).await;
    }
    took.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "\n  40 questions, {} ms apart: fastest {:.0} ms, median {:.0} ms, slowest {:.0} ms",
        crate::watcher::MODE_INTERVAL.as_millis(),
        ms(took[0]),
        ms(took[20]),
        ms(took[39])
    );
    assert!(took[20] < crate::watcher::MODE_INTERVAL, "a question outlasts the gap between them");
}
