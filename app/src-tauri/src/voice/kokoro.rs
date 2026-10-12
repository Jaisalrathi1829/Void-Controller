//! Kokoro inference, through sherpa-onnx.
//!
//! This is the only place speech is actually synthesized. It loads the model
//! from local files and runs it on this machine's CPU; there is no network
//! code in it or beneath it in this application.

use super::assets::{ModelFiles, Voice, SAMPLE_RATE, VOICES};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// The folder the speech runtime took its pronunciation data from, the first
/// time a model was loaded in this process.
///
/// The runtime turns text into sounds with eSpeak NG, and eSpeak NG is set up
/// once per process: the first model's data folder is the one it reads from
/// then on, whatever folder a later model is loaded from. Found on
/// 2026-10-12, when a test that loaded the model from a temporary folder ran
/// first and deleted the folder: every model loaded afterwards in that
/// process — from the proper place, with every checksum passing — spoke a
/// third of a second of noise, because the dictionary it was still looking
/// for had gone.
///
/// Nothing can repoint it, so this is remembered instead, and a load that
/// would run on a folder that is no longer intact is refused with the reason.
static PRONUNCIATION_DATA: OnceLock<PathBuf> = OnceLock::new();

/// See [`PRONUNCIATION_DATA`]. `None` until a model has been loaded.
pub fn pronunciation_data_dir() -> Option<PathBuf> {
    PRONUNCIATION_DATA.get().cloned()
}

/// The slowest and fastest speech offered. Outside this range Kokoro's output
/// stops sounding like speech worth listening to.
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

/// Inference threads. Announcements are a sentence long; more threads would
/// buy little and take CPU from whatever the user is actually doing.
const THREADS: i32 = 2;

/// Synthesized audio: mono, 32-bit float, at `sample_rate`.
#[derive(Debug, Clone, PartialEq)]
pub struct Speech {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Speech {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }
}

/// Why synthesis produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthError {
    /// Cancelled before it finished. Not a failure.
    Cancelled,
    Failed(String),
}

/// Something that turns text into speech.
///
/// The worker talks to this rather than to [`Kokoro`] directly so that its
/// scheduling can be tested without a 128 MB model. A test double standing in
/// here proves things about scheduling only — never about real inference.
pub trait Synthesizer {
    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError>;
}

/// A loaded Kokoro model.
pub struct Kokoro {
    tts: OfflineTts,
}

impl Kokoro {
    /// Loads the model and checks it is the one this build expects.
    ///
    /// The voice list offered to the user is a property of the pinned model,
    /// written down in `assets.rs`. Here the loaded model is asked how many
    /// speakers it has and what rate it produces; if either differs, these
    /// are not the files the list describes and the model is refused, rather
    /// than offering voices that might map to the wrong speakers.
    ///
    /// The files must already have passed [`assets::verify`]. That is not a
    /// formality: given a file that is not a model, the runtime does not fail
    /// — it ends the process. `load_verified` is the entry point that checks
    /// first, and the one the application uses.
    ///
    /// [`assets::verify`]: super::assets::verify
    pub fn load_verified(files: &ModelFiles) -> Result<Self, String> {
        super::assets::verify(files).map_err(|problem| problem.describe())?;
        Self::load(files)
    }

    fn load(files: &ModelFiles) -> Result<Self, String> {
        // The runtime will go on reading pronunciation data from wherever it
        // first found it. If that is somewhere else, and no longer what it
        // was, this model would load and then mispronounce everything.
        if let Some(first) = PRONUNCIATION_DATA.get() {
            if first != &files.espeak_data && !super::assets::pronunciation_data_intact(first) {
                return Err(format!(
                    "The speech runtime read its pronunciation data from {} when it first \
                     loaded a model, and reads it from there for as long as the application \
                     runs. That folder is no longer intact. Restart the application to use the \
                     model in {}.",
                    first.display(),
                    files.dir.display()
                ));
            }
        }

        let path = |p: &std::path::Path| Some(p.to_string_lossy().into_owned());
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: path(&files.model),
                    voices: path(&files.voices),
                    tokens: path(&files.tokens),
                    data_dir: path(&files.espeak_data),
                    length_scale: 1.0,
                    ..Default::default()
                },
                num_threads: THREADS,
                debug: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let tts = OfflineTts::create(&config).ok_or_else(|| {
            format!(
                "The speech runtime could not load the model in {}.",
                files.dir.display()
            )
        })?;

        let speakers = tts.num_speakers();
        if speakers != VOICES.len() as i32 {
            return Err(format!(
                "The installed model has {speakers} voices where {} are expected. It is not the \
                 model this build was made for.",
                VOICES.len()
            ));
        }
        let rate = tts.sample_rate();
        if rate != SAMPLE_RATE as i32 {
            return Err(format!(
                "The installed model produces {rate} Hz audio where {SAMPLE_RATE} Hz is expected."
            ));
        }
        // Kept only if nothing is there yet: the first is the one that counts.
        let _ = PRONUNCIATION_DATA.set(files.espeak_data.clone());
        Ok(Self { tts })
    }
}

impl Synthesizer for Kokoro {
    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError> {
        // The runtime takes a C string; an interior NUL would end it early.
        let text: String = text.chars().filter(|c| *c != '\0').collect();
        if text.trim().is_empty() {
            return Err(SynthError::Failed("there is nothing to say".into()));
        }

        let config = GenerationConfig {
            sid: voice.speaker_id,
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
            ..Default::default()
        };

        // The runtime asks, between chunks, whether to carry on.
        let stop = Arc::clone(cancel);
        let keep_going = move |_so_far: &[f32], _progress: f32| !stop.load(Ordering::SeqCst);

        let audio = self
            .tts
            .generate_with_config(&text, &config, Some(keep_going));

        if cancel.load(Ordering::SeqCst) {
            return Err(SynthError::Cancelled);
        }
        let audio = audio.ok_or_else(|| SynthError::Failed("the model produced no audio".into()))?;
        let samples = audio.samples().to_vec();
        if samples.is_empty() {
            return Err(SynthError::Failed("the model produced no audio".into()));
        }
        Ok(Speech {
            samples,
            sample_rate: audio.sample_rate().max(1) as u32,
        })
    }
}

/// Tests that load the real model and run real inference.
///
/// `#[ignore]`d because they need the model installed
/// (`tools/install-voice-model.ps1`) and take seconds, not milliseconds. They
/// are the evidence that speech here is genuinely Kokoro's and not a stand-in:
///
/// ```text
/// cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 kokoro_real
/// ```
#[cfg(test)]
pub(crate) mod real_tests {
    use super::super::assets;
    use super::*;
    use std::time::Instant;

    /// The installed model, or a panic saying how to install it.
    pub fn installed_model() -> ModelFiles {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .expect("LOCALAPPDATA")
            .join("local.bosecontrolcenter.app")
            .join("voice");
        assets::locate(&root).unwrap_or_else(|problem| {
            panic!(
                "{} Run tools/install-voice-model.ps1 first.",
                problem.describe()
            )
        })
    }

    fn not_cancelled() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// Root-mean-square level: zero for silence.
    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    fn voice(id: &str) -> &'static Voice {
        assets::voice(id).unwrap()
    }

    /// Which `onnxruntime.dll` this process actually loaded. Windows ships an
    /// older one in System32, and loading that by accident is the classic way
    /// for this to go wrong.
    #[cfg(windows)]
    fn loaded_onnxruntime() -> String {
        use windows::core::w;
        use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
        unsafe {
            let module = GetModuleHandleW(w!("onnxruntime.dll")).expect("onnxruntime.dll loaded");
            let mut buf = [0u16; 1024];
            let len = GetModuleFileNameW(module, &mut buf) as usize;
            String::from_utf16_lossy(&buf[..len])
        }
    }

    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_loads_and_speaks() {
        let files = installed_model();

        let started = Instant::now();
        let mut kokoro = Kokoro::load_verified(&files).expect("model loads");
        let load = started.elapsed();

        let started = Instant::now();
        let speech = kokoro
            .synthesize("Voice feedback is ready.", voice("af_bella"), 1.0, &not_cancelled())
            .expect("synthesis succeeds");
        let first = started.elapsed();

        let started = Instant::now();
        let again = kokoro
            .synthesize("Quiet mode activated.", voice("af_bella"), 1.0, &not_cancelled())
            .expect("synthesis succeeds");
        let second = started.elapsed();

        println!("\n  model load          : {load:?}");
        println!(
            "  \"Voice feedback is ready.\" : {:.2} s of audio in {first:?} (first run)",
            speech.seconds()
        );
        println!(
            "  \"Quiet mode activated.\"    : {:.2} s of audio in {second:?}",
            again.seconds()
        );
        println!("  level (rms)         : {:.4}", rms(&speech.samples));
        #[cfg(windows)]
        println!("  onnxruntime.dll     : {}", loaded_onnxruntime());

        assert_eq!(speech.sample_rate, SAMPLE_RATE);
        // A short sentence: more than half a second, well under ten.
        assert!((0.5..10.0).contains(&speech.seconds()), "{} s", speech.seconds());
        assert!(rms(&speech.samples) > 0.01, "the audio is not silence");
        assert!(speech.samples.iter().all(|s| s.is_finite()));
        assert!(speech.samples.iter().any(|s| s.abs() > 0.05));

        #[cfg(windows)]
        assert!(
            !loaded_onnxruntime().to_lowercase().contains("system32"),
            "the runtime shipped with the application must be the one loaded"
        );
    }

    /// The typical pitch of the voiced parts of some speech, in Hz: for each
    /// short frame loud enough to be speech, the lag at which the frame best
    /// matches itself, and then the median over frames.
    fn median_pitch_hz(speech: &Speech) -> f32 {
        let rate = speech.sample_rate as usize;
        let frame = rate / 25; // 40 ms
        let (shortest, longest) = (rate / 400, rate / 70); // 400 Hz .. 70 Hz
        let loud = rms(&speech.samples) * 0.5;

        let mut pitches = Vec::new();
        let last = speech.samples.len().saturating_sub(frame + longest);
        for start in (0..last).step_by(frame / 2) {
            let a = &speech.samples[start..start + frame];
            if rms(a) < loud {
                continue;
            }
            let energy: f32 = a.iter().map(|s| s * s).sum();
            let mut best = (0usize, 0.0f32);
            for lag in shortest..=longest {
                let b = &speech.samples[start + lag..start + lag + frame];
                let corr: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                let other: f32 = b.iter().map(|s| s * s).sum();
                let score = corr / (energy * other).sqrt().max(1e-9);
                if score > best.1 {
                    best = (lag, score);
                }
            }
            if best.1 > 0.6 {
                pitches.push(rate as f32 / best.0 as f32);
            }
        }
        assert!(pitches.len() >= 5, "too few voiced frames to judge pitch");
        pitches.sort_by(|a, b| a.total_cmp(b));
        pitches[pitches.len() / 2]
    }

    /// Different voices are different speakers, not one voice under eleven
    /// names.
    ///
    /// Comparing samples would prove nothing: Kokoro is not deterministic, so
    /// even the same voice gives different samples each time. Pitch is a
    /// property of the speaker, so that is what is compared — a female voice
    /// against two male ones on the same sentence, and against itself.
    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_voices_differ() {
        let mut kokoro = Kokoro::load(&installed_model()).expect("model loads");
        let pitch = |k: &mut Kokoro, id: &str| {
            let speech = k
                .synthesize("Aware mode activated.", voice(id), 1.0, &not_cancelled())
                .unwrap();
            median_pitch_hz(&speech)
        };
        let bella = pitch(&mut kokoro, "af_bella");
        let adam = pitch(&mut kokoro, "am_adam");
        let george = pitch(&mut kokoro, "bm_george");
        let bella_again = pitch(&mut kokoro, "af_bella");

        println!("  typical pitch: af_bella {bella:.0} Hz (and {bella_again:.0} Hz on a second run)");
        println!("                 am_adam {adam:.0} Hz, bm_george {george:.0} Hz");
        assert!(bella > adam * 1.3, "af_bella {bella} Hz vs am_adam {adam} Hz");
        assert!(bella > george * 1.3, "af_bella {bella} Hz vs bm_george {george} Hz");
        // The same voice sounds like itself from one run to the next.
        assert!(
            (bella - bella_again).abs() < bella * 0.15,
            "{bella} Hz vs {bella_again} Hz"
        );
    }

    /// Every voice in the list really is there and really speaks.
    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_every_listed_voice_speaks() {
        let mut kokoro = Kokoro::load(&installed_model()).expect("model loads");
        println!();
        for v in &VOICES {
            let speech = kokoro
                .synthesize("Battery at thirty percent.", v, 1.0, &not_cancelled())
                .unwrap_or_else(|e| panic!("{} failed: {e:?}", v.id));
            println!("  {:<12} {:.2} s  rms {:.4}", v.id, speech.seconds(), rms(&speech.samples));
            assert!(rms(&speech.samples) > 0.01, "{} is silent", v.id);
        }
    }

    /// Speed changes how long the same sentence takes to say.
    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_speed_changes_duration() {
        let mut kokoro = Kokoro::load(&installed_model()).expect("model loads");
        let say = |k: &mut Kokoro, speed: f32| {
            k.synthesize("Aurora connected.", voice("af_bella"), speed, &not_cancelled())
                .unwrap()
                .seconds()
        };
        let slow = say(&mut kokoro, 0.7);
        let normal = say(&mut kokoro, 1.0);
        let fast = say(&mut kokoro, 1.5);
        println!("\n  0.7x: {slow:.2} s   1.0x: {normal:.2} s   1.5x: {fast:.2} s");
        assert!(slow > normal && normal > fast, "{slow} > {normal} > {fast}");

        // Out-of-range requests are clamped, not passed through.
        let absurd = say(&mut kokoro, 50.0);
        let fastest = say(&mut kokoro, MAX_SPEED);
        assert!((absurd - fastest).abs() < 0.05, "{absurd} vs {fastest}");
    }

    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_cancelled_synthesis_reports_cancelled() {
        let mut kokoro = Kokoro::load(&installed_model()).expect("model loads");
        let cancel = Arc::new(AtomicBool::new(true));
        let result = kokoro.synthesize("Voice feedback is ready.", voice("af"), 1.0, &cancel);
        assert_eq!(result, Err(SynthError::Cancelled));
    }

    #[test]
    #[ignore = "needs the Kokoro model installed; runs real inference"]
    fn kokoro_real_empty_text_is_an_error_not_silence() {
        let mut kokoro = Kokoro::load(&installed_model()).expect("model loads");
        for text in ["", "   ", "\0"] {
            let result = kokoro.synthesize(text, voice("af"), 1.0, &not_cancelled());
            assert!(matches!(result, Err(SynthError::Failed(_))), "{text:?}");
        }
    }

    /// A model file that is not a model never reaches the runtime.
    ///
    /// This is the guard for a measured hazard. Calling the runtime's loader
    /// directly on a garbage model file — with everything else the real
    /// installation — ended the test process: "Rust cannot catch foreign
    /// exceptions, aborting" (0xc0000409). That experiment cannot be kept as
    /// a test, because it cannot pass. What is kept is the check that the
    /// application's own entry point refuses such a file first.
    #[test]
    #[ignore = "needs the Kokoro model installed"]
    fn kokoro_real_garbage_model_is_refused_before_the_runtime_sees_it() {
        let real = installed_model();
        let dir = tempfile::tempdir().unwrap();
        // A copy of the real installation's layout whose model is garbage of
        // exactly the right size, so only the checksum can tell.
        let fake_dir = assets::model_dir(dir.path());
        std::fs::create_dir_all(fake_dir.join("espeak-ng-data")).unwrap();
        for name in ["voices.bin", "tokens.txt"] {
            std::fs::copy(real.dir.join(name), fake_dir.join(name)).unwrap();
        }
        for name in ["phontab", "phonindex", "phondata", "intonations", "en_dict"] {
            std::fs::copy(
                real.espeak_data.join(name),
                fake_dir.join("espeak-ng-data").join(name),
            )
            .unwrap();
        }
        let size = real.model.metadata().unwrap().len() as usize;
        std::fs::write(fake_dir.join("model.int8.onnx"), vec![0x42u8; size]).unwrap();

        let files = assets::locate(dir.path()).expect("the layout is complete");
        let err = Kokoro::load_verified(&files).err().expect("refused");
        println!("  refused with: {err}");
        assert!(err.contains("model.int8.onnx"), "{err}");
    }

    /// Real speech through the real headphones' Windows output.
    ///
    /// Needs the headphones connected. This is the first point at which sound
    /// actually comes out, so it measures what can be measured from here: that
    /// Windows accepted the audio for that specific endpoint, and that the
    /// endpoint's own level meter moved while it played. Whether it was heard
    /// is for whoever is wearing the headphones to say.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs the Kokoro model and the headphones connected; plays sound"]
    fn voice_real_plays_through_the_headphones() {
        use crate::audio::playback;
        use crate::bluetooth::pnp;

        let seen = crate::bose::real::sight_device(None).expect("a paired Bose device");
        assert!(seen.connected, "connect the headphones first");
        let container = pnp::container_id(&seen.instance_id).expect("a container id");
        let output = playback::output_for_device(&container)
            .expect("endpoints can be listed")
            .expect("the headphones have an active stereo output");
        let default = playback::default_output().unwrap();
        println!("  headphones : {}", seen.name);
        println!("  their output  : {}", output.name);
        println!("  Windows default: {:?}", default.as_ref().map(|o| &o.name));

        let mut kokoro = Kokoro::load_verified(&installed_model()).expect("model loads");
        let speech = kokoro
            .synthesize("Voice feedback is ready.", voice("af_bella"), 1.0, &not_cancelled())
            .unwrap();

        let stop = AtomicBool::new(false);
        let silent = playback::play(&output.id, &vec![0.0; 12_000], SAMPLE_RATE, 1.0, &stop).unwrap();
        let quiet = playback::play(&output.id, &speech.samples, speech.sample_rate, 0.2, &stop).unwrap();
        let loud = playback::play(&output.id, &speech.samples, speech.sample_rate, 0.8, &stop).unwrap();
        println!("  endpoint meter — silence: {:.4}", silent.endpoint_peak);
        println!("                   speech at gain 0.2: {:.4}", quiet.endpoint_peak);
        println!("                   speech at gain 0.8: {:.4}", loud.endpoint_peak);

        assert!(quiet.completed && loud.completed);
        assert!(loud.endpoint_peak > 0.01, "the endpoint's meter moved");
        assert!(
            loud.endpoint_peak > quiet.endpoint_peak * 1.5,
            "speech volume changes what reaches the endpoint"
        );
    }

    /// Stopping ends playback promptly rather than at the end of the sentence.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs an audio output; plays (near-silent) sound"]
    fn voice_real_playback_stops_when_told() {
        use crate::audio::playback;
        let output = playback::default_output().unwrap().expect("a default output");
        // Five seconds of very quiet tone, stopped after a third of a second.
        let tone: Vec<f32> = (0..SAMPLE_RATE * 5)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.02)
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let played = playback::play(&output.id, &tone, SAMPLE_RATE, 1.0, &stop).unwrap();
        println!("  stopped after {:?}", started.elapsed());
        assert!(!played.completed);
        assert!(started.elapsed() < std::time::Duration::from_millis(1500));
    }
}
