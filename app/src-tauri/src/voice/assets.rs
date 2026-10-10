//! The Kokoro model on disk: where it lives, what must be there, and which
//! voices it has.
//!
//! The application is built against one exact model, `kokoro-int8-en-v0_19`,
//! installed by `tools/install-voice-model.ps1`. Its file layout and voice
//! list are fixed properties of that model, so they are written down here
//! rather than discovered — and then checked against what the loaded model
//! itself reports before any voice is offered (see `kokoro.rs`).
//!
//! Nothing here downloads anything. If the model is not installed, that is
//! reported, and installing it is an explicit step the user takes.
//!
//! # Why the files are checksummed
//!
//! Measured against the real runtime: given a model file that is not a model,
//! it does not return an error — it throws a C++ exception across the library
//! boundary, which ends the whole process. A truncated download or a damaged
//! file would therefore take the application down the first time it tried to
//! speak. So before the runtime is given anything, every file it will read is
//! compared against the checksum of the pinned release. Only byte-for-byte
//! known files ever reach it. The same check is what makes the compiled-in
//! voice list trustworthy: it describes exactly these bytes.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The pinned model. Also the name of its folder.
pub const MODEL_NAME: &str = "kokoro-int8-en-v0_19";

/// Kokoro's output rate. Fixed by the model.
pub const SAMPLE_RATE: u32 = 24_000;

const MODEL_FILE: &str = "model.int8.onnx";
const VOICES_FILE: &str = "voices.bin";
const TOKENS_FILE: &str = "tokens.txt";
const ESPEAK_DATA_DIR: &str = "espeak-ng-data";

/// A file of the pinned release: its path inside the model folder, its size,
/// and its SHA-256.
struct Pinned {
    path: &'static str,
    bytes: u64,
    sha256: &'static str,
}

/// Every file the runtime reads to speak English with this model, as shipped
/// in `kokoro-int8-en-v0_19.tar.bz2`
/// (SHA-256 c9f0dd393615805b0bab050c340834d5e684e732aec91c0e860cd30e982c08bd).
///
/// The pronunciation data has several hundred more files for other languages.
/// They are not read for English and are not listed.
const PINNED_FILES: [Pinned; 8] = [
    Pinned {
        path: "model.int8.onnx",
        bytes: 134_186_977,
        sha256: "e0530c5031c03c5dc957edfc6d72582c2246e9b4566643ff67eeb03610c11641",
    },
    Pinned {
        path: "voices.bin",
        bytes: 5_755_904,
        sha256: "a372c67b056ef0b695c375d39b99630d23fb07ad4c8d87aa32a19a62fca523ad",
    },
    Pinned {
        path: "tokens.txt",
        bytes: 1_078,
        sha256: "4f31c71282d14af4e926cd12462078fe9d20d00c589e63fe2750a8f56d6d7f7b",
    },
    Pinned {
        path: "espeak-ng-data/phontab",
        bytes: 55_796,
        sha256: "886f3fa402cb0ba73d483aa8ad000af47a6b7cc06293c75a97913fba68a530f6",
    },
    Pinned {
        path: "espeak-ng-data/phonindex",
        bytes: 39_074,
        sha256: "3ca7b8fa3b42624e4b0f152707e7a39245fce569aa99ea47c055d9e622fcf0c4",
    },
    Pinned {
        path: "espeak-ng-data/phondata",
        bytes: 550_424,
        sha256: "4e0288957874029a8c3c9f41a8f517ad4bf18127046decbdd4b9d1d6807ce3a3",
    },
    Pinned {
        path: "espeak-ng-data/intonations",
        bytes: 2_040,
        sha256: "3f8af65fd3eda9759a10f021d61361c120871f463515229c925995c7f90918cc",
    },
    Pinned {
        path: "espeak-ng-data/en_dict",
        bytes: 166_944,
        sha256: "71bd330ba8a2e3e8076e631508208ef49449d6147c17b7bd2b4b1e1468292e35",
    },
];

/// A voice the model can speak with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    /// The model's own name for the voice. This is what is stored in settings.
    pub id: &'static str,
    /// How it is described to a person.
    pub label: &'static str,
    /// The model's speaker index for it.
    pub speaker_id: i32,
}

/// The voices of `kokoro-en-v0_19`, in the model's speaker order.
///
/// From the upstream documentation for this model. The loaded model must
/// report exactly this many speakers, or none of them is offered.
pub const VOICES: [Voice; 11] = [
    Voice { id: "af", label: "American, female (default blend)", speaker_id: 0 },
    Voice { id: "af_bella", label: "Bella — American, female", speaker_id: 1 },
    Voice { id: "af_nicole", label: "Nicole — American, female", speaker_id: 2 },
    Voice { id: "af_sarah", label: "Sarah — American, female", speaker_id: 3 },
    Voice { id: "af_sky", label: "Sky — American, female", speaker_id: 4 },
    Voice { id: "am_adam", label: "Adam — American, male", speaker_id: 5 },
    Voice { id: "am_michael", label: "Michael — American, male", speaker_id: 6 },
    Voice { id: "bf_emma", label: "Emma — British, female", speaker_id: 7 },
    Voice { id: "bf_isabella", label: "Isabella — British, female", speaker_id: 8 },
    Voice { id: "bm_george", label: "George — British, male", speaker_id: 9 },
    Voice { id: "bm_lewis", label: "Lewis — British, male", speaker_id: 10 },
];

/// The voice used until the user picks one.
pub const DEFAULT_VOICE: &str = "af_bella";

/// Looks a voice up by its id.
pub fn voice(id: &str) -> Option<&'static Voice> {
    VOICES.iter().find(|v| v.id == id)
}

/// The files of an installed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFiles {
    pub dir: PathBuf,
    pub model: PathBuf,
    pub voices: PathBuf,
    pub tokens: PathBuf,
    pub espeak_data: PathBuf,
}

/// Why the model cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetProblem {
    /// Nothing is installed where the model should be.
    NotInstalled { expected_dir: PathBuf },
    /// The folder exists, but something it must contain is missing or empty.
    Incomplete { dir: PathBuf, missing: Vec<String> },
    /// A file is there but is not the file of the pinned release: damaged,
    /// cut short, or from a different model.
    Modified { dir: PathBuf, file: String },
}

impl AssetProblem {
    pub fn describe(&self) -> String {
        match self {
            Self::NotInstalled { expected_dir } => format!(
                "The voice model is not installed. Expected it at {}.",
                expected_dir.display()
            ),
            Self::Incomplete { dir, missing } => format!(
                "The voice model at {} is incomplete: missing {}. Reinstall it.",
                dir.display(),
                missing.join(", ")
            ),
            Self::Modified { dir, file } => format!(
                "The voice model at {} cannot be used: {file} is damaged or is not from the \
                 model this build was made for. Reinstall it.",
                dir.display()
            ),
        }
    }
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn check(dir: &Path, pinned: &[Pinned]) -> Result<(), AssetProblem> {
    for file in pinned {
        let path = dir.join(file.path);
        let modified = || AssetProblem::Modified {
            dir: dir.to_path_buf(),
            file: file.path.to_string(),
        };
        // Size first: it is free, and catches a download that was cut short.
        let size = path.metadata().map(|m| m.len()).map_err(|_| modified())?;
        if size != file.bytes {
            return Err(modified());
        }
        if sha256_of(&path).map_err(|_| modified())? != file.sha256 {
            return Err(modified());
        }
    }
    Ok(())
}

/// Confirms the installed files are exactly the pinned release.
///
/// Must pass before the runtime is given the files; see the module docs for
/// why. It reads the whole model once, which takes a moment, so callers do it
/// once per run rather than before every sentence.
pub fn verify(files: &ModelFiles) -> Result<(), AssetProblem> {
    check(&files.dir, &PINNED_FILES)
}

/// Where the pinned model belongs under the application's local data folder.
pub fn model_dir(voice_root: &Path) -> PathBuf {
    voice_root.join(MODEL_NAME)
}

/// Checks that everything the model needs is present.
///
/// This looks at names and sizes only; it does not read the model. Whether
/// the files are a model the runtime can actually load is established when
/// it is loaded, and reported from there.
pub fn locate(voice_root: &Path) -> Result<ModelFiles, AssetProblem> {
    let dir = model_dir(voice_root);
    if !dir.is_dir() {
        return Err(AssetProblem::NotInstalled { expected_dir: dir });
    }

    let files = ModelFiles {
        model: dir.join(MODEL_FILE),
        voices: dir.join(VOICES_FILE),
        tokens: dir.join(TOKENS_FILE),
        espeak_data: dir.join(ESPEAK_DATA_DIR),
        dir: dir.clone(),
    };

    let non_empty_file = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0);
    let non_empty_dir =
        |p: &Path| p.is_dir() && p.read_dir().is_ok_and(|mut entries| entries.next().is_some());

    let mut missing = Vec::new();
    for (path, ok) in [
        (&files.model, non_empty_file(&files.model)),
        (&files.voices, non_empty_file(&files.voices)),
        (&files.tokens, non_empty_file(&files.tokens)),
        (&files.espeak_data, non_empty_dir(&files.espeak_data)),
    ] {
        if !ok {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            missing.push(name.into_owned());
        }
    }

    if missing.is_empty() {
        Ok(files)
    } else {
        Err(AssetProblem::Incomplete { dir, missing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &Path) -> PathBuf {
        let dir = model_dir(root);
        std::fs::create_dir_all(dir.join(ESPEAK_DATA_DIR)).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"model").unwrap();
        std::fs::write(dir.join(VOICES_FILE), b"voices").unwrap();
        std::fs::write(dir.join(TOKENS_FILE), b"tokens").unwrap();
        std::fs::write(dir.join(ESPEAK_DATA_DIR).join("en_dict"), b"dict").unwrap();
        dir
    }

    #[test]
    fn a_complete_install_is_located() {
        let root = tempfile::tempdir().unwrap();
        let dir = install(root.path());
        let files = locate(root.path()).unwrap();
        assert_eq!(files.dir, dir);
        assert!(files.model.ends_with("model.int8.onnx"));
        assert!(files.espeak_data.ends_with("espeak-ng-data"));
    }

    #[test]
    fn nothing_installed_says_where_it_was_looked_for() {
        let root = tempfile::tempdir().unwrap();
        let problem = locate(root.path()).unwrap_err();
        assert!(matches!(problem, AssetProblem::NotInstalled { .. }));
        assert!(problem.describe().contains(MODEL_NAME));
    }

    #[test]
    fn each_missing_piece_is_named() {
        for gone in [MODEL_FILE, VOICES_FILE, TOKENS_FILE] {
            let root = tempfile::tempdir().unwrap();
            let dir = install(root.path());
            std::fs::remove_file(dir.join(gone)).unwrap();
            let AssetProblem::Incomplete { missing, .. } = locate(root.path()).unwrap_err() else {
                panic!("expected Incomplete");
            };
            assert_eq!(missing, vec![gone.to_string()]);
        }
    }

    /// A download cut short can leave a zero-byte file, or an empty folder.
    #[test]
    fn empty_files_and_folders_do_not_count_as_installed() {
        let root = tempfile::tempdir().unwrap();
        let dir = install(root.path());
        std::fs::write(dir.join(MODEL_FILE), b"").unwrap();
        std::fs::remove_file(dir.join(ESPEAK_DATA_DIR).join("en_dict")).unwrap();

        let AssetProblem::Incomplete { missing, .. } = locate(root.path()).unwrap_err() else {
            panic!("expected Incomplete");
        };
        assert!(missing.contains(&MODEL_FILE.to_string()));
        assert!(missing.contains(&ESPEAK_DATA_DIR.to_string()));
    }

    fn pin(path: &'static str, content: &[u8]) -> Pinned {
        let digest: String = Sha256::digest(content)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Pinned {
            path,
            bytes: content.len() as u64,
            // Leaked so the test can build a `&'static str`; a few bytes.
            sha256: Box::leak(digest.into_boxed_str()),
        }
    }

    #[test]
    fn files_matching_their_checksums_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), b"the right bytes").unwrap();
        let pinned = [pin("a.bin", b"the right bytes")];
        assert!(check(dir.path(), &pinned).is_ok());
    }

    /// The case that crashes the real runtime: a file of the right name that
    /// is not the right file. It is caught here, before the runtime sees it.
    #[test]
    fn a_file_with_the_wrong_content_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        // Same length, different bytes: only the checksum can tell.
        std::fs::write(dir.path().join("a.bin"), b"the wrong bytes").unwrap();
        let pinned = [pin("a.bin", b"the right bytes")];

        let problem = check(dir.path(), &pinned).unwrap_err();
        assert!(matches!(&problem, AssetProblem::Modified { file, .. } if file == "a.bin"));
        assert!(problem.describe().contains("Reinstall"));
    }

    #[test]
    fn a_truncated_or_missing_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = [pin("a.bin", b"the right bytes")];

        std::fs::write(dir.path().join("a.bin"), b"the right").unwrap();
        assert!(matches!(
            check(dir.path(), &pinned),
            Err(AssetProblem::Modified { .. })
        ));

        std::fs::remove_file(dir.path().join("a.bin")).unwrap();
        assert!(matches!(
            check(dir.path(), &pinned),
            Err(AssetProblem::Modified { .. })
        ));
    }

    /// The first file to fail is the one named.
    #[test]
    fn the_damaged_file_is_the_one_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), b"good").unwrap();
        std::fs::write(dir.path().join("b.bin"), b"bad!").unwrap();
        let pinned = [pin("a.bin", b"good"), pin("b.bin", b"good")];
        let problem = check(dir.path(), &pinned).unwrap_err();
        assert!(matches!(&problem, AssetProblem::Modified { file, .. } if file == "b.bin"));
    }

    #[test]
    fn the_pinned_list_covers_the_model_and_the_english_pronunciation_data() {
        let paths: Vec<_> = PINNED_FILES.iter().map(|p| p.path).collect();
        for needed in [MODEL_FILE, VOICES_FILE, TOKENS_FILE] {
            assert!(paths.contains(&needed), "{needed}");
        }
        assert!(paths.contains(&"espeak-ng-data/phontab"));
        assert!(paths.contains(&"espeak-ng-data/en_dict"));
        for p in &PINNED_FILES {
            assert_eq!(p.sha256.len(), 64, "{}", p.path);
            assert!(p.bytes > 0);
        }
    }

    /// The real installation, if there is one, is the pinned release.
    #[test]
    #[ignore = "needs the Kokoro model installed"]
    fn kokoro_real_installed_model_matches_the_pinned_checksums() {
        let files = crate::voice::kokoro::real_tests::installed_model();
        let started = std::time::Instant::now();
        verify(&files).unwrap_or_else(|p| panic!("{}", p.describe()));
        println!("  verified {} files in {:?}", PINNED_FILES.len(), started.elapsed());
    }

    #[test]
    fn voices_are_in_speaker_order_with_unique_ids() {
        for (index, v) in VOICES.iter().enumerate() {
            assert_eq!(v.speaker_id, index as i32, "{}", v.id);
            assert_eq!(VOICES.iter().filter(|o| o.id == v.id).count(), 1);
            assert!(!v.label.is_empty());
        }
        assert!(voice(DEFAULT_VOICE).is_some(), "the default voice exists");
        assert!(voice("af_heart").is_none(), "a voice of a different model");
    }
}
