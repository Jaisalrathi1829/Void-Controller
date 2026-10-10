//! The real thing behind the worker: Kokoro on disk, Windows audio out.

use super::assets::{self, ModelFiles, Voice};
use super::kokoro::{Kokoro, Speech, SynthError, Synthesizer};
use super::{Backend, Route, Via};
use crate::audio::{Output, Played};
use crate::settings::SettingsStore;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct KokoroBackend {
    /// The folder the model's own folder lives in.
    voice_root: PathBuf,
    settings: Arc<SettingsStore>,
    /// The installed files, once they have been confirmed to be the model.
    files: Option<ModelFiles>,
    kokoro: Option<Kokoro>,
}

impl KokoroBackend {
    pub fn new(voice_root: PathBuf, settings: Arc<SettingsStore>) -> Self {
        Self {
            voice_root,
            settings,
            files: None,
            kokoro: None,
        }
    }
}

impl Backend for KokoroBackend {
    fn assets_dir(&self) -> String {
        assets::model_dir(&self.voice_root).display().to_string()
    }

    fn check_assets(&mut self) -> Result<(), String> {
        let checked = assets::locate(&self.voice_root)
            .and_then(|files| assets::verify(&files).map(|()| files))
            .map_err(|problem| problem.describe());
        match checked {
            Ok(files) => {
                // A model loaded from files that are no longer the ones on
                // disk is not kept.
                if self.files.as_ref() != Some(&files) {
                    self.kokoro = None;
                }
                self.files = Some(files);
                Ok(())
            }
            Err(problem) => {
                self.files = None;
                self.kokoro = None;
                Err(problem)
            }
        }
    }

    fn load(&mut self) -> Result<(), String> {
        if self.kokoro.is_some() {
            return Ok(());
        }
        let files = self
            .files
            .clone()
            .ok_or_else(|| "The voice model is not installed.".to_string())?;
        // Checked again here, immediately before the runtime is handed the
        // files: they may have changed since they were last looked at, and
        // the runtime does not survive being given the wrong ones.
        self.kokoro = Some(Kokoro::load_verified(&files)?);
        Ok(())
    }

    fn is_loaded(&self) -> bool {
        self.kokoro.is_some()
    }

    fn unload(&mut self) {
        self.kokoro = None;
    }

    fn synthesize(
        &mut self,
        text: &str,
        voice: &Voice,
        speed: f32,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Speech, SynthError> {
        match self.kokoro.as_mut() {
            Some(kokoro) => kokoro.synthesize(text, voice, speed, cancel),
            None => Err(SynthError::Failed("the voice model is not loaded".into())),
        }
    }

    #[cfg(windows)]
    fn route(&mut self, via: Via, allow_default: bool) -> Route {
        use crate::audio::playback;
        use crate::bluetooth::pnp;

        // Which physical device the headphones are, so that their output can
        // be found — or avoided — whatever anything is named.
        let container = self
            .settings
            .get()
            .preferred_device_id
            .and_then(|id| pnp::container_id(&id));

        let headphones = match (via, &container) {
            (Via::HeadphonesFirst, Some(container)) => {
                playback::output_for_device(container).ok().flatten()
            }
            _ => None,
        };
        let default = if !allow_default {
            None
        } else if via == Via::NotTheHeadphones {
            playback::default_output_avoiding(container.as_deref())
                .ok()
                .flatten()
        } else {
            playback::default_output().ok().flatten()
        };
        super::choose_route(via, headphones, default, allow_default)
    }

    #[cfg(not(windows))]
    fn route(&mut self, _via: Via, _allow_default: bool) -> Route {
        let _ = &self.settings;
        Route::Nowhere("Speech output is only implemented for Windows.".into())
    }

    #[cfg(windows)]
    fn play(
        &mut self,
        output: &Output,
        speech: &Speech,
        gain: f32,
        stop: &AtomicBool,
    ) -> Result<Played, String> {
        crate::audio::playback::play(&output.id, &speech.samples, speech.sample_rate, gain, stop)
    }

    #[cfg(not(windows))]
    fn play(
        &mut self,
        _output: &Output,
        _speech: &Speech,
        _gain: f32,
        _stop: &AtomicBool,
    ) -> Result<Played, String> {
        Err("Speech output is only implemented for Windows.".into())
    }
}
