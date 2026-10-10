//! Voice feedback: spoken announcements of confirmed device events.
//!
//! The pieces, in the order an announcement passes through them:
//!
//! | Module | Does |
//! | --- | --- |
//! | [`policy`] | Decides whether a confirmed change is worth saying, and what the sentence is. |
//! | [`queue`] | Holds what is waiting: priority order, one per subject, bounded. |
//! | this module | One worker thread that takes the next announcement, synthesizes it and plays it. |
//! | [`kokoro`] | Kokoro inference, on this machine. |
//! | [`backend`] | The real model and the real Windows audio output. |
//!
//! Three rules hold throughout.
//!
//! **Nothing unconfirmed is spoken.** Announcements enter only through
//! [`VoiceFeedback::observe_snapshot`] and [`VoiceFeedback::observe_presence`],
//! which are fed states the headphones or Windows have reported. There is no
//! entry point for "the user asked for Quiet".
//!
//! **Nothing here blocks anything else.** Synthesis and playback happen on
//! the worker thread. Every public method only touches a short-lived lock.
//!
//! **One voice at a time.** A single thread does the speaking, so two
//! announcements cannot overlap, and the model is loaded once and reused.

pub mod assets;
pub mod backend;
pub mod kokoro;
pub mod policy;
pub mod queue;

use crate::audio::{Output, Played};
use crate::device::state::{DeviceSnapshot, DeviceSource};
use crate::settings::{SettingsStore, StoredSettings};
use crate::watcher::Step;
use assets::Voice;
use kokoro::{Speech, SynthError};
use parking_lot::{Condvar, Mutex};
use policy::{Announcement, Category, ModeTracker};
use queue::Queue;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Emitted to the window whenever the status changes.
pub const VOICE_STATUS_EVENT: &str = "voice-status";

/// How many synthesized sentences are kept for reuse. The announcements are a
/// handful of fixed sentences, so a few entries cover everything said in
/// normal use; at about 150 KB each the whole cache stays under a few MB.
const CACHE_ENTRIES: usize = 16;

/// Sentences longer than this are not kept. Nothing the application says is.
const CACHEABLE_CHARS: usize = 80;

/// Where an announcement may be played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The headphones' own output, if it is there.
    HeadphonesFirst,
    /// Anything but the headphones: they are what just went away.
    NotTheHeadphones,
}

/// The output an announcement will be played through, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    To(Output),
    Nowhere(String),
}

/// Decides where speech goes, given what outputs exist.
///
/// Speech meant for the headphones goes to the headphones. It goes anywhere
/// else only if the user has said it may — it is never quietly sent out of
/// the laptop's speakers because the headphones happened to be unavailable.
pub fn choose_route(
    via: Via,
    headphones: Option<Output>,
    default: Option<Output>,
    allow_default: bool,
) -> Route {
    if via == Via::HeadphonesFirst {
        if let Some(output) = headphones {
            return Route::To(output);
        }
    }
    match (allow_default, default) {
        (true, Some(output)) => Route::To(output),
        (true, None) => Route::Nowhere("Windows has no audio output to speak through.".into()),
        (false, _) if via == Via::NotTheHeadphones => Route::Nowhere(
            "The headphones are gone, so this could only be spoken through another output, and \
             speaking through the default output is switched off."
                .into(),
        ),
        (false, _) => Route::Nowhere(
            "The headphones' audio output is not available, and speaking through the default \
             output is switched off."
                .into(),
        ),
    }
}

/// What the worker needs from the outside world. The real one loads Kokoro
/// and plays through Windows; tests substitute one that records what it was
/// asked to do. A substitute proves things about scheduling and policy only.
pub trait Backend: Send + 'static {
    /// Where the model is expected to be, for telling the user.
    fn assets_dir(&self) -> String;
    /// Confirms the model is installed and intact.
    fn check_assets(&mut self) -> Result<(), String>;
    /// Loads the model if it is not loaded.
    fn load(&mut self) -> Result<(), String>;
    fn is_loaded(&self) -> bool;
    /// Releases the model's memory.
    fn unload(&mut self);
    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError>;
    fn route(&mut self, via: Via, allow_default: bool) -> Route;
    fn play(
        &mut self,
        output: &Output,
        speech: &Speech,
        gain: f32,
        stop: &AtomicBool,
    ) -> Result<Played, String>;
}

/// The states shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VoiceState {
    /// The model is missing, incomplete or not the expected one.
    NotInstalled,
    /// Able to speak.
    Ready,
    /// Loading the model.
    Initializing,
    Synthesizing,
    Playing,
    /// Able to speak, but there is nowhere to speak to.
    Unavailable,
    /// The last attempt failed.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceInfo {
    pub id: String,
    pub label: String,
}

/// Everything the Settings page shows about voice feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub state: VoiceState,
    /// One sentence saying what `state` means right now.
    pub detail: String,
    pub model: String,
    /// Where the model is, or is expected to be.
    pub assets_dir: String,
    /// The voices of the installed model. Empty until the installed files
    /// have been confirmed to be that model.
    pub voices: Vec<VoiceInfo>,
    /// The sentence being synthesized or played, if any.
    pub speaking: Option<String>,
    /// Whether that sentence is a Test Voice request rather than an
    /// announcement of something that happened.
    pub is_test: bool,
    /// The output last spoken through.
    pub output: Option<String>,
    /// Whether the model is in memory at the moment.
    pub model_loaded: bool,
}

/// The waits the worker uses, injectable so tests need not sit through them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// With nothing synthesized for this long, the model is released. The
    /// sentences already synthesized stay cached, so the usual announcements
    /// keep working without it.
    pub idle_unload: Duration,
    /// How long an announcement waits for the headphones' output to appear.
    /// Windows takes a moment to bring it up after the headphones connect.
    pub route_wait: Duration,
    pub route_retry: Duration,
    /// Synthesize the fixed sentences ahead of need, while idle.
    pub prepare_ahead: bool,
}

impl Timing {
    pub fn standard() -> Self {
        Self {
            idle_unload: Duration::from_secs(90),
            route_wait: Duration::from_secs(8),
            route_retry: Duration::from_millis(500),
            prepare_ahead: true,
        }
    }
}

/// Speech volume as a gain. Squared, so that the slider's steps sound even:
/// loudness is heard roughly logarithmically, and a straight line would put
/// nearly all of the audible change in the bottom fifth of the slider.
pub fn gain_for(volume_percent: u8) -> f32 {
    let linear = f32::from(volume_percent.min(100)) / 100.0;
    linear * linear
}

type CacheKey = (String, &'static str, u16);

struct Current {
    automatic: bool,
    /// Pre-synthesizing a sentence for later, not speaking one.
    preparing: bool,
    cancel: Arc<AtomicBool>,
}

struct Shared {
    queue: Queue,
    mode: ModeTracker,
    /// Sentences to synthesize ahead of need, when there is nothing to say.
    prepare: VecDeque<String>,
    recheck_requested: u64,
    recheck_done: u64,
    shutdown: bool,
    finished: bool,
    assets_ok: bool,
    status: VoiceStatus,
    cache: Vec<(CacheKey, Arc<Speech>)>,
    current: Option<Current>,
}

struct Inner {
    shared: Mutex<Shared>,
    wake: Condvar,
    settings: Arc<SettingsStore>,
    emit: Box<dyn Fn(&VoiceStatus) + Send + Sync>,
    timing: Timing,
}

/// How one announcement ended.
enum Outcome {
    Spoken,
    /// Cancelled, gone stale, or switched off while it waited.
    Dropped,
    Unavailable(String),
    Failed(String),
}

/// The voice-feedback service. Cheap to clone; all clones are one service.
#[derive(Clone)]
pub struct VoiceFeedback {
    inner: Arc<Inner>,
}

fn allowed(announcement: &Announcement, settings: &StoredSettings) -> bool {
    match announcement.category() {
        // The Test Voice button works whether or not announcements are on:
        // it is how a voice is chosen before switching them on.
        Category::Test => true,
        Category::Mode => settings.voice_enabled && settings.announce_mode_changes,
        Category::Connection => settings.voice_enabled && settings.announce_connection,
        Category::Battery => settings.voice_enabled && settings.announce_battery,
    }
}

impl VoiceFeedback {
    /// Starts the service and its worker thread.
    pub fn start(
        settings: Arc<SettingsStore>,
        backend: Box<dyn Backend>,
        timing: Timing,
        emit: impl Fn(&VoiceStatus) + Send + Sync + 'static,
    ) -> Self {
        let status = VoiceStatus {
            state: VoiceState::Initializing,
            detail: "Checking the voice model…".to_string(),
            model: assets::MODEL_NAME.to_string(),
            assets_dir: backend.assets_dir(),
            voices: Vec::new(),
            speaking: None,
            is_test: false,
            output: None,
            model_loaded: false,
        };
        let inner = Arc::new(Inner {
            shared: Mutex::new(Shared {
                queue: Queue::default(),
                mode: ModeTracker::default(),
                prepare: VecDeque::new(),
                recheck_requested: 0,
                recheck_done: 0,
                shutdown: false,
                finished: false,
                assets_ok: false,
                status,
                cache: Vec::new(),
                current: None,
            }),
            wake: Condvar::new(),
            settings,
            emit: Box::new(emit),
            timing,
        });

        let worker = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("voice-feedback".into())
            .spawn(move || run(worker, backend))
            .expect("the voice thread can be started");

        Self { inner }
    }

    pub fn status(&self) -> VoiceStatus {
        self.inner.shared.lock().status.clone()
    }

    /// Queues the Test Voice sentence.
    pub fn test(&self) -> Result<(), String> {
        let mut shared = self.inner.shared.lock();
        if !shared.assets_ok {
            return Err(shared.status.detail.clone());
        }
        shared.queue.push(Announcement::Test);
        cancel_preparing(&shared);
        drop(shared);
        self.inner.wake.notify_all();
        Ok(())
    }

    /// Stops whatever is being said and drops everything waiting.
    pub fn stop(&self) {
        let mut shared = self.inner.shared.lock();
        shared.queue.clear();
        shared.prepare.clear();
        if let Some(current) = &shared.current {
            current.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Looks again for the model — after installing it, say — and waits for
    /// the answer.
    pub fn recheck(&self) -> VoiceStatus {
        let mut shared = self.inner.shared.lock();
        shared.recheck_requested += 1;
        let wanted = shared.recheck_requested;
        self.inner.wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(15);
        while shared.recheck_done < wanted && !shared.finished {
            if self.inner.wake.wait_until(&mut shared, deadline).timed_out() {
                break;
            }
        }
        shared.status.clone()
    }

    /// Tells the service the settings changed, so that what it is doing
    /// matches them from this moment.
    ///
    /// Switching voice feedback or a category off drops every waiting
    /// announcement it covers and stops one being spoken. Changing the voice,
    /// speed or volume affects whatever is said next; nothing already waiting
    /// is dropped for it.
    pub fn settings_changed(&self, before: &StoredSettings, after: &StoredSettings) {
        let mut shared = self.inner.shared.lock();
        shared.queue.retain(|a| allowed(a, after));

        if before.voice_enabled && !after.voice_enabled {
            shared.prepare.clear();
            if let Some(current) = &shared.current {
                if current.automatic {
                    current.cancel.store(true, Ordering::SeqCst);
                }
            }
        }

        let voice_changed = before.voice_id != after.voice_id
            || before.voice_speed_percent != after.voice_speed_percent;
        let switches_changed = before.voice_enabled != after.voice_enabled
            || before.announce_mode_changes != after.announce_mode_changes
            || before.announce_connection != after.announce_connection;
        let prepare = self.inner.timing.prepare_ahead && after.voice_enabled && shared.assets_ok;
        if prepare && (voice_changed || switches_changed) {
            shared.prepare = sentences_to_prepare(after).into();
        }
        drop(shared);
        self.inner.wake.notify_all();
    }

    /// Whether a mode change would be spoken if one were found — which is
    /// what makes it worth asking the headphones for their mode when nothing
    /// on screen needs it.
    pub fn wants_mode_readings(&self) -> bool {
        let settings = self.inner.settings.get();
        settings.voice_enabled
            && settings.announce_mode_changes
            && self.inner.shared.lock().assets_ok
    }

    /// Takes a snapshot of the device as evidence of its mode.
    ///
    /// The only way a mode announcement comes about: the mode in a snapshot
    /// of real, connected headphones differs from the mode in the one before.
    pub fn observe_snapshot(&self, snapshot: &DeviceSnapshot) {
        let confirmed = if snapshot.source == DeviceSource::Real {
            policy::confirmed_mode(snapshot)
        } else {
            None
        };
        let settings = self.inner.settings.get();
        let mut shared = self.inner.shared.lock();
        let announcement = shared.mode.observe(confirmed);
        // Whatever was waiting to be said about the mode may have just
        // become untrue.
        let Shared { queue, mode, .. } = &mut *shared;
        queue.retain(|a| mode.still_true(a));

        if let Some(announcement) = announcement {
            self.push_automatic(&mut shared, announcement, &settings);
        }
        drop(shared);
        self.inner.wake.notify_all();
    }

    /// Takes what the background watcher saw: the headphones connecting,
    /// disconnecting, or running low.
    pub fn observe_presence(&self, step: &Step) {
        if !step.lost && !step.returned && step.low_battery.is_none() {
            return;
        }
        let settings = self.inner.settings.get();
        let mut shared = self.inner.shared.lock();

        if step.lost {
            // Whatever mode they were in, they are not in it for us any more.
            shared.mode.reset();
            // Something being spoken into headphones that have just gone is
            // being spoken to nobody.
            if let Some(current) = &shared.current {
                if current.automatic && !current.preparing {
                    current.cancel.store(true, Ordering::SeqCst);
                }
            }
            let gone = Announcement::Disconnected {
                name: step.name.clone(),
            };
            self.push_automatic(&mut shared, gone, &settings);
        }
        if step.returned {
            let back = Announcement::Connected {
                name: step.name.clone(),
            };
            self.push_automatic(&mut shared, back, &settings);
        }
        if let Some(percent) = step.low_battery {
            self.push_automatic(&mut shared, Announcement::LowBattery { percent }, &settings);
        }
        drop(shared);
        self.inner.wake.notify_all();
    }

    fn push_automatic(
        &self,
        shared: &mut Shared,
        announcement: Announcement,
        settings: &StoredSettings,
    ) {
        if !shared.assets_ok || !allowed(&announcement, settings) {
            return;
        }
        shared.queue.push(announcement);
        cancel_preparing(shared);
    }

    /// Stops the worker and waits briefly for it to finish.
    ///
    /// A sentence being synthesized cannot be interrupted part-way, so the
    /// wait is bounded: if the worker is still busy when it runs out, the
    /// process is exiting anyway and takes the thread with it.
    pub fn shutdown(&self) {
        let mut shared = self.inner.shared.lock();
        shared.shutdown = true;
        shared.queue.clear();
        shared.prepare.clear();
        if let Some(current) = &shared.current {
            current.cancel.store(true, Ordering::SeqCst);
        }
        self.inner.wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !shared.finished {
            if self.inner.wake.wait_until(&mut shared, deadline).timed_out() {
                break;
            }
        }
    }
}

/// A real announcement has arrived: stop synthesizing ahead, so that it is
/// not kept waiting behind a sentence nobody has asked for yet.
fn cancel_preparing(shared: &Shared) {
    if let Some(current) = &shared.current {
        if current.preparing {
            current.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// The fixed sentences worth having ready: without this, the first "Quiet
/// mode activated" after a voice change would arrive a couple of seconds
/// after the mode did.
fn sentences_to_prepare(settings: &StoredSettings) -> Vec<String> {
    use policy::SpokenMode;
    let mut sentences = Vec::new();
    if settings.announce_mode_changes {
        sentences.push(Announcement::Mode(SpokenMode::Quiet).text());
        sentences.push(Announcement::Mode(SpokenMode::Aware).text());
    }
    if settings.announce_connection {
        if let Some(name) = &settings.preferred_device_name {
            sentences.push(Announcement::Connected { name: name.clone() }.text());
            sentences.push(Announcement::Disconnected { name: name.clone() }.text());
        }
    }
    sentences
}

enum Job {
    Say(Announcement),
    Prepare(String),
    Recheck,
    Unload,
    Shutdown,
}

fn run(inner: Arc<Inner>, mut backend: Box<dyn Backend>) {
    check_assets(&inner, backend.as_mut());
    let mut last_used = Instant::now();

    loop {
        let job = {
            let mut shared = inner.shared.lock();
            loop {
                if shared.shutdown {
                    break Job::Shutdown;
                }
                if shared.recheck_requested != shared.recheck_done {
                    break Job::Recheck;
                }
                if let Some(announcement) = shared.queue.pop() {
                    break Job::Say(announcement);
                }
                if let Some(sentence) = shared.prepare.pop_front() {
                    break Job::Prepare(sentence);
                }
                if backend.is_loaded() {
                    let idle = last_used.elapsed();
                    if idle >= inner.timing.idle_unload {
                        break Job::Unload;
                    }
                    inner
                        .wake
                        .wait_for(&mut shared, inner.timing.idle_unload - idle);
                } else {
                    inner.wake.wait(&mut shared);
                }
            }
        };

        match job {
            Job::Shutdown => break,
            Job::Recheck => {
                let wanted = inner.shared.lock().recheck_requested;
                check_assets(&inner, backend.as_mut());
                inner.shared.lock().recheck_done = wanted;
                inner.wake.notify_all();
            }
            Job::Unload => {
                backend.unload();
                set_status(&inner, |s| s.model_loaded = false);
            }
            Job::Say(announcement) => say(&inner, backend.as_mut(), announcement, &mut last_used),
            Job::Prepare(sentence) => prepare(&inner, backend.as_mut(), sentence, &mut last_used),
        }
    }

    backend.unload();
    let mut shared = inner.shared.lock();
    shared.finished = true;
    shared.status.model_loaded = false;
    inner.wake.notify_all();
}

/// Changes the status and tells the window, if anything actually changed.
fn set_status(inner: &Inner, change: impl FnOnce(&mut VoiceStatus)) {
    let (before, after) = {
        let mut shared = inner.shared.lock();
        let before = shared.status.clone();
        change(&mut shared.status);
        (before, shared.status.clone())
    };
    if before != after {
        (inner.emit)(&after);
    }
}

fn check_assets(inner: &Inner, backend: &mut dyn Backend) {
    let result = backend.check_assets();
    let ok = result.is_ok();
    let loaded = backend.is_loaded();
    inner.shared.lock().assets_ok = ok;
    set_status(inner, |status| {
        status.model_loaded = loaded;
        status.speaking = None;
        status.is_test = false;
        match &result {
            Ok(()) => {
                status.state = VoiceState::Ready;
                status.detail = "Ready.".to_string();
                status.voices = assets::VOICES
                    .iter()
                    .map(|v| VoiceInfo {
                        id: v.id.to_string(),
                        label: v.label.to_string(),
                    })
                    .collect();
            }
            Err(problem) => {
                status.state = VoiceState::NotInstalled;
                status.detail = problem.clone();
                // No voices are offered for a model that has not been
                // confirmed to be the one they belong to.
                status.voices = Vec::new();
            }
        }
    });
    if ok && inner.timing.prepare_ahead {
        let settings = inner.settings.get();
        if settings.voice_enabled {
            inner.shared.lock().prepare = sentences_to_prepare(&settings).into();
        }
    }
}

fn cached(inner: &Inner, key: &CacheKey) -> Option<Arc<Speech>> {
    let shared = inner.shared.lock();
    shared
        .cache
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, speech)| Arc::clone(speech))
}

fn remember(inner: &Inner, key: CacheKey, speech: &Arc<Speech>) {
    if key.0.chars().count() > CACHEABLE_CHARS {
        return;
    }
    let mut shared = inner.shared.lock();
    shared.cache.retain(|(k, _)| k != &key);
    if shared.cache.len() >= CACHE_ENTRIES {
        shared.cache.remove(0);
    }
    shared.cache.push((key, Arc::clone(speech)));
}

/// Why a sentence is being synthesized, which decides what the status says
/// while it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// Something happened, and this says so.
    Announcement,
    /// The Test Voice button.
    Test,
    /// Made ready for later. Nothing is being said, and nothing has happened.
    Preparation,
}

/// Gets the audio for a sentence: from the cache, or from the model.
fn speech_for(
    inner: &Inner,
    backend: &mut dyn Backend,
    text: &str,
    settings: &StoredSettings,
    cancel: &Arc<AtomicBool>,
    purpose: Purpose,
    last_used: &mut Instant,
) -> Result<Arc<Speech>, Outcome> {
    let voice = assets::voice(&settings.voice_id)
        .or_else(|| assets::voice(assets::DEFAULT_VOICE))
        .expect("the default voice exists");
    let key: CacheKey = (text.to_string(), voice.id, settings.voice_speed_percent);
    if let Some(speech) = cached(inner, &key) {
        return Ok(speech);
    }

    if !backend.is_loaded() {
        set_status(inner, |s| {
            s.state = VoiceState::Initializing;
            s.detail = "Loading the voice model…".to_string();
        });
        backend.load().map_err(Outcome::Failed)?;
        set_status(inner, |s| s.model_loaded = true);
    }

    set_status(inner, |s| {
        s.state = VoiceState::Synthesizing;
        s.is_test = purpose == Purpose::Test;
        if purpose == Purpose::Preparation {
            // The status names a sentence only when that sentence is on its
            // way to being heard. Showing "Quiet mode activated." here would
            // read as an announcement of something that has not happened.
            s.detail = "Getting the announcements ready…".to_string();
            s.speaking = None;
        } else {
            s.detail = "Synthesizing speech…".to_string();
            s.speaking = Some(text.to_string());
        }
    });
    let speed = f32::from(settings.voice_speed_percent) / 100.0;
    *last_used = Instant::now();
    let speech = match backend.synthesize(text, voice, speed, cancel) {
        Ok(speech) => Arc::new(speech),
        Err(SynthError::Cancelled) => return Err(Outcome::Dropped),
        Err(SynthError::Failed(why)) => {
            return Err(Outcome::Failed(format!("Speech synthesis failed: {why}")))
        }
    };
    *last_used = Instant::now();
    remember(inner, key, &speech);
    Ok(speech)
}

/// Whether an announcement should still be spoken, checked again each time
/// something slow has happened since it was queued.
fn still_wanted(inner: &Inner, announcement: &Announcement, cancel: &AtomicBool) -> bool {
    if cancel.load(Ordering::SeqCst) {
        return false;
    }
    let settings = inner.settings.get();
    allowed(announcement, &settings) && inner.shared.lock().mode.still_true(announcement)
}

fn say(
    inner: &Inner,
    backend: &mut dyn Backend,
    announcement: Announcement,
    last_used: &mut Instant,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut shared = inner.shared.lock();
        if !shared.assets_ok {
            return;
        }
        shared.current = Some(Current {
            automatic: announcement.is_automatic(),
            preparing: false,
            cancel: Arc::clone(&cancel),
        });
    }

    let outcome = speak(inner, backend, &announcement, &cancel, last_used);

    inner.shared.lock().current = None;
    let loaded = backend.is_loaded();
    set_status(inner, |status| {
        status.speaking = None;
        status.is_test = false;
        status.model_loaded = loaded;
        match outcome {
            Outcome::Spoken | Outcome::Dropped => {
                status.state = VoiceState::Ready;
                status.detail = "Ready.".to_string();
            }
            Outcome::Unavailable(why) => {
                status.state = VoiceState::Unavailable;
                status.detail = why;
            }
            Outcome::Failed(why) => {
                status.state = VoiceState::Error;
                status.detail = why;
            }
        }
    });
}

fn speak(
    inner: &Inner,
    backend: &mut dyn Backend,
    announcement: &Announcement,
    cancel: &Arc<AtomicBool>,
    last_used: &mut Instant,
) -> Outcome {
    if !still_wanted(inner, announcement, cancel) {
        return Outcome::Dropped;
    }
    let settings = inner.settings.get();
    let text = announcement.text();
    let is_test = !announcement.is_automatic();

    // Where to. Decided before any synthesis: there is no point working out
    // how to say something there is nowhere to say.
    let via = if matches!(announcement, Announcement::Disconnected { .. }) {
        Via::NotTheHeadphones
    } else {
        Via::HeadphonesFirst
    };
    // Something that just happened to the headphones is worth a short wait
    // for their output to come up. A test is answered at once either way.
    let patience = if announcement.is_automatic() && via == Via::HeadphonesFirst {
        inner.timing.route_wait
    } else {
        Duration::ZERO
    };
    let asked = Instant::now();
    let output = loop {
        let waiting = asked.elapsed() < patience;
        // While waiting, only the headphones will do. The fallback, if it is
        // allowed at all, is a last resort and not a way to speak sooner.
        let allow_default = settings.voice_use_default_output && !waiting;
        match backend.route(via, allow_default) {
            Route::To(output) => break output,
            Route::Nowhere(why) if !waiting => return Outcome::Unavailable(why),
            Route::Nowhere(_) => {
                if !still_wanted(inner, announcement, cancel) {
                    return Outcome::Dropped;
                }
                std::thread::sleep(inner.timing.route_retry);
            }
        }
    };

    let purpose = if is_test {
        Purpose::Test
    } else {
        Purpose::Announcement
    };
    let speech = match speech_for(inner, backend, &text, &settings, cancel, purpose, last_used) {
        Ok(speech) => speech,
        Err(outcome) => return outcome,
    };

    // Synthesis took a moment. The mode may have moved on, or the setting
    // been switched off, while it ran.
    if !still_wanted(inner, announcement, cancel) {
        return Outcome::Dropped;
    }

    set_status(inner, |s| {
        s.state = VoiceState::Playing;
        s.detail = format!("Speaking through {}.", output.name);
        s.speaking = Some(text.clone());
        s.is_test = is_test;
        s.output = Some(output.name.clone());
    });
    // The volume is read as late as possible, so the slider applies to the
    // very next thing said.
    let gain = gain_for(inner.settings.get().voice_volume_percent);
    match backend.play(&output, &speech, gain, cancel) {
        Ok(played) if played.completed => Outcome::Spoken,
        Ok(_) => Outcome::Dropped,
        Err(why) => Outcome::Failed(format!("Playback through {} failed: {why}", output.name)),
    }
}

/// Synthesizes a sentence into the cache without playing it.
fn prepare(inner: &Inner, backend: &mut dyn Backend, sentence: String, last_used: &mut Instant) {
    let settings = inner.settings.get();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut shared = inner.shared.lock();
        if !shared.assets_ok || !settings.voice_enabled {
            return;
        }
        shared.current = Some(Current {
            automatic: true,
            preparing: true,
            cancel: Arc::clone(&cancel),
        });
    }

    let result = speech_for(
        inner,
        backend,
        &sentence,
        &settings,
        &cancel,
        Purpose::Preparation,
        last_used,
    );

    inner.shared.lock().current = None;
    let loaded = backend.is_loaded();
    set_status(inner, |status| {
        status.speaking = None;
        status.model_loaded = loaded;
        match result {
            Ok(_) | Err(Outcome::Dropped) | Err(Outcome::Spoken) | Err(Outcome::Unavailable(_)) => {
                status.state = VoiceState::Ready;
                status.detail = "Ready.".to_string();
            }
            Err(Outcome::Failed(why)) => {
                status.state = VoiceState::Error;
                status.detail = why;
            }
        }
    });
}

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
mod hardware_tests;
