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
    /// A second place to look: a model that came with the application.
    bundled_root: Option<PathBuf>,
    settings: Arc<SettingsStore>,
    /// The installed files, once they have been confirmed to be the model.
    files: Option<ModelFiles>,
    kokoro: Option<Kokoro>,
}

impl KokoroBackend {
    pub fn new(voice_root: PathBuf, settings: Arc<SettingsStore>) -> Self {
        Self {
            voice_root,
            bundled_root: None,
            settings,
            files: None,
            kokoro: None,
        }
    }

    /// Also looks for the model in `root`, where an installer that carries
    /// the model puts it. A model the user installed themselves comes first.
    pub fn with_bundled(mut self, root: Option<PathBuf>) -> Self {
        self.bundled_root = root.map(plain_path);
        self
    }

    /// The first place that holds the model intact; failing that, the most
    /// telling reason none does — a damaged model over an absent one.
    fn find_model(&self) -> Result<ModelFiles, String> {
        let mut absent = None;
        let mut broken = None;
        for root in std::iter::once(&self.voice_root).chain(self.bundled_root.iter()) {
            match assets::locate(root).and_then(|files| assets::verify(&files).map(|()| files)) {
                Ok(files) => return Ok(files),
                Err(problem @ assets::AssetProblem::NotInstalled { .. }) => {
                    absent.get_or_insert(problem);
                }
                Err(problem) => {
                    broken.get_or_insert(problem);
                }
            }
        }
        Err(broken
            .or(absent)
            .map(|problem| problem.describe())
            .unwrap_or_else(|| "The voice model is not installed.".to_string()))
    }
}

/// Removes Windows' verbatim prefix (`\\?\`) from a path.
///
/// The application's own folder is handed over in that form, and the speech
/// runtime, which opens files through the C library, cannot open a path
/// written that way: it reports that the model could not be loaded. A
/// verbatim UNC path (`\\?\UNC\...`) has no plain spelling this simple and
/// is left as it is.
fn plain_path(path: PathBuf) -> PathBuf {
    match path.to_str() {
        Some(text) if !text.starts_with(r"\\?\UNC\") => match text.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => path,
        },
        _ => path,
    }
}

impl Backend for KokoroBackend {
    fn assets_dir(&self) -> String {
        assets::model_dir(&self.voice_root).display().to_string()
    }

    fn check_assets(&mut self) -> Result<(), String> {
        match self.find_model() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::StoredSettings;

    fn backend(user: &std::path::Path, bundled: Option<&std::path::Path>) -> KokoroBackend {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings::default()));
        KokoroBackend::new(user.to_path_buf(), store).with_bundled(bundled.map(Into::into))
    }

    #[test]
    fn with_no_model_anywhere_it_names_the_folder_to_install_into() {
        let user = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let mut b = backend(user.path(), Some(bundled.path()));
        let problem = b.check_assets().unwrap_err();
        assert!(problem.contains("not installed"), "{problem}");
        assert!(problem.contains(&user.path().display().to_string()), "{problem}");
        assert_eq!(b.assets_dir(), assets::model_dir(user.path()).display().to_string());
    }

    #[test]
    fn the_verbatim_prefix_is_removed_and_nothing_else_is_touched() {
        let plain = |p: &str| plain_path(PathBuf::from(p)).display().to_string();
        assert_eq!(
            plain(r"\\?\C:\Users\me\AppData\Local\Bose QC Control Center\voice"),
            r"C:\Users\me\AppData\Local\Bose QC Control Center\voice"
        );
        assert_eq!(plain(r"C:\Program Files\App\voice"), r"C:\Program Files\App\voice");
        assert_eq!(plain(r"\\?\UNC\server\share\voice"), r"\\?\UNC\server\share\voice");
        assert_eq!(plain(r"\\server\share\voice"), r"\\server\share\voice");
    }

    /// The installed application's folder has spaces in its name and arrives
    /// with the verbatim prefix. The real runtime has to load from there.
    #[test]
    #[ignore = "needs the voice model installed; copies it (150 MB) to a temporary folder"]
    fn kokoro_real_loads_a_bundled_model_from_the_installed_kind_of_path() {
        fn copy(from: &std::path::Path, to: &std::path::Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        let installed = crate::voice::kokoro::real_tests::installed_model();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Bose QC Control Center").join("voice");
        copy(&installed.dir, &assets::model_dir(&root));

        // As Tauri hands it over.
        let verbatim = std::fs::canonicalize(&root).unwrap();
        assert!(verbatim.to_str().unwrap().starts_with(r"\\?\"));

        let nowhere = temp.path().join("no user model");
        let mut b = backend(&nowhere, Some(&verbatim));
        b.check_assets().expect("the bundled model is found");
        b.load().expect("the runtime loads it");
        let voice = assets::voice(assets::DEFAULT_VOICE).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let speech = b.synthesize("Voice feedback is ready.", voice, 1.0, &cancel).unwrap();
        assert!(speech.samples.len() > 12_000);
        b.unload();
    }

    /// A model that is there but wrong is the thing to report, wherever it is.
    #[test]
    fn a_damaged_bundled_model_is_reported_rather_than_called_missing() {
        let user = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(assets::model_dir(bundled.path())).unwrap();
        let mut b = backend(user.path(), Some(bundled.path()));
        let problem = b.check_assets().unwrap_err();
        assert!(problem.contains("incomplete"), "{problem}");
        assert!(!b.is_loaded());
    }
}
