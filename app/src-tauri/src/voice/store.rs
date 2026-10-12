//! Sentences already synthesized, kept on disk.
//!
//! The application says the same few sentences over and over. Making one
//! needs the model — about a quarter of a gigabyte of memory once loaded, and
//! a second or more of work — so each sentence is kept, as audio, the first
//! time it is made, and read back from then on. With the usual sentences on
//! disk the model is not loaded at all in ordinary use, and an announcement
//! is as quick as reading a small file.
//!
//! What is kept is exactly what Kokoro produced for that sentence, in that
//! voice, at that speed. Nothing here makes speech, and nothing is fetched
//! from anywhere.
//!
//! A file that is not byte-for-byte what was written is not played. Each one
//! names the model, voice, speed and sentence it was made for, and ends with
//! a checksum of everything before it; anything that does not add up is
//! treated as not there, and the sentence is synthesized again.

use super::assets::MODEL_NAME;
use super::kokoro::Speech;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAGIC: &[u8; 8] = b"BQCSPCH1";
const EXTENSION: &str = "speech";
const DIGEST_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC.len() + 12;

/// How many sentences are kept. The fixed announcements are a handful per
/// voice and speed; the rest of the room is for the ones with a number in
/// them. The least recently used goes first.
pub const MAX_FILES: usize = 64;

/// Nothing the application says is this long. A file claiming more is not
/// one this module wrote.
const MAX_SECONDS: u32 = 30;

pub struct SpeechStore {
    dir: PathBuf,
}

/// What a file must have been made for to be the one asked for.
fn key(text: &str, voice: &str, speed_percent: u16) -> String {
    format!("{MODEL_NAME}\n{voice}\n{speed_percent}\n{text}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl SpeechStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let digest = Sha256::digest(key.as_bytes());
        self.dir.join(format!("{}.{EXTENSION}", hex(&digest[..16])))
    }

    /// The audio kept for this sentence, if there is any and it is intact.
    pub fn load(&self, text: &str, voice: &str, speed_percent: u16) -> Option<Speech> {
        let key = key(text, voice, speed_percent);
        let path = self.path_for(&key);
        let bytes = std::fs::read(&path).ok()?;
        match decode(&bytes, &key) {
            Some(speech) => {
                // Used just now: the last to be dropped when room is needed.
                touch(&path);
                Some(speech)
            }
            None => {
                // Not something to try again next time.
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Keeps the audio for this sentence. Failing to is not an error worth
    /// anyone's attention: the sentence is simply synthesized again next time.
    pub fn save(
        &self,
        text: &str,
        voice: &str,
        speed_percent: u16,
        speech: &Speech,
    ) -> io::Result<()> {
        let key = key(text, voice, speed_percent);
        let bytes = encode(speech, &key)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not audio worth keeping"))?;
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path_for(&key);
        // Written whole or not at all: a half-written file would fail its
        // checksum anyway, but there is no reason to leave one lying about.
        let partial = path.with_extension("partial");
        {
            let mut file = std::fs::File::create(&partial)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&partial, &path)?;
        self.prune();
        Ok(())
    }

    /// Drops the least recently used files beyond [`MAX_FILES`].
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
            .map(|path| {
                let used = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (used, path)
            })
            .collect();
        if files.len() <= MAX_FILES {
            return;
        }
        files.sort();
        let excess = files.len() - MAX_FILES;
        for (_, path) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn touch(path: &Path) {
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn encode(speech: &Speech, key: &str) -> Option<Vec<u8>> {
    let samples = u32::try_from(speech.samples.len()).ok()?;
    let key_len = u32::try_from(key.len()).ok()?;
    if samples == 0 || speech.sample_rate == 0 {
        return None;
    }
    if u64::from(samples) > u64::from(speech.sample_rate) * u64::from(MAX_SECONDS) {
        return None;
    }
    let mut out =
        Vec::with_capacity(HEADER_LEN + key.len() + speech.samples.len() * 2 + DIGEST_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&speech.sample_rate.to_le_bytes());
    out.extend_from_slice(&samples.to_le_bytes());
    out.extend_from_slice(&key_len.to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    for sample in &speech.samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    let digest = Sha256::digest(&out);
    out.extend_from_slice(&digest);
    Some(out)
}

fn decode(bytes: &[u8], key: &str) -> Option<Speech> {
    if bytes.len() < HEADER_LEN + DIGEST_LEN || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    let (body, digest) = bytes.split_at(bytes.len() - DIGEST_LEN);
    if Sha256::digest(body).as_slice() != digest {
        return None;
    }
    let word = |at: usize| {
        let field: [u8; 4] = body.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(field))
    };
    let sample_rate = word(MAGIC.len())?;
    let samples = word(MAGIC.len() + 4)? as usize;
    let key_len = word(MAGIC.len() + 8)? as usize;
    if sample_rate == 0 || samples == 0 {
        return None;
    }
    if samples as u64 > u64::from(sample_rate) * u64::from(MAX_SECONDS) {
        return None;
    }
    let rest = &body[HEADER_LEN..];
    if rest.len() != key_len.checked_add(samples.checked_mul(2)?)? {
        return None;
    }
    let (stored_key, pcm) = rest.split_at(key_len);
    // A different sentence that happens to share a file name is not this one.
    if stored_key != key.as_bytes() {
        return None;
    }
    Some(Speech {
        samples: pcm
            .chunks_exact(2)
            .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / f32::from(i16::MAX))
            .collect(),
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(samples: usize) -> Speech {
        Speech {
            samples: (0..samples)
                .map(|i| ((i as f32) * 0.01).sin() * 0.8)
                .collect(),
            sample_rate: 24_000,
        }
    }

    fn store() -> (tempfile::TempDir, SpeechStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SpeechStore::new(dir.path().join("spoken"));
        (dir, store)
    }

    #[test]
    fn what_was_kept_comes_back_as_the_same_audio() {
        let (_dir, store) = store();
        let made = speech(36_000);
        store.save("Quiet mode activated.", "af_bella", 100, &made).unwrap();

        let back = store.load("Quiet mode activated.", "af_bella", 100).expect("kept");
        assert_eq!(back.sample_rate, 24_000);
        assert_eq!(back.samples.len(), made.samples.len());
        // Sixteen bits of it, which is all that is ever played.
        let worst = made
            .samples
            .iter()
            .zip(&back.samples)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1.0 / 16_000.0, "worst difference {worst}");
    }

    #[test]
    fn nothing_is_there_until_something_has_been_kept() {
        let (_dir, store) = store();
        assert!(store.load("Quiet mode activated.", "af_bella", 100).is_none());
    }

    /// The same words in another voice, or at another speed, are other audio.
    #[test]
    fn a_sentence_is_kept_per_voice_and_per_speed() {
        let (_dir, store) = store();
        store.save("Aware mode activated.", "af_bella", 100, &speech(30_000)).unwrap();
        assert!(store.load("Aware mode activated.", "am_adam", 100).is_none());
        assert!(store.load("Aware mode activated.", "af_bella", 120).is_none());
        assert!(store.load("Quiet mode activated.", "af_bella", 100).is_none());
        assert!(store.load("Aware mode activated.", "af_bella", 100).is_some());
    }

    fn only_file(store: &SpeechStore) -> PathBuf {
        std::fs::read_dir(&store.dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == EXTENSION))
            .expect("one file")
    }

    /// A file that has been changed — by a failing disk, or by anything else
    /// — is not played. It is removed, and the sentence is made again.
    #[test]
    fn a_damaged_file_is_not_played() {
        let (_dir, store) = store();
        store.save("Quiet mode activated.", "af_bella", 100, &speech(24_000)).unwrap();
        let path = only_file(&store);

        let mut bytes = std::fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x40;
        std::fs::write(&path, &bytes).unwrap();

        assert!(store.load("Quiet mode activated.", "af_bella", 100).is_none());
        assert!(!path.exists(), "the damaged file is gone");
    }

    #[test]
    fn a_cut_short_file_is_not_played() {
        let (_dir, store) = store();
        store.save("Quiet mode activated.", "af_bella", 100, &speech(24_000)).unwrap();
        let path = only_file(&store);
        let bytes = std::fs::read(&path).unwrap();
        for keep in [0, 7, HEADER_LEN, bytes.len() - 1] {
            std::fs::write(&path, &bytes[..keep]).unwrap();
            assert!(
                store.load("Quiet mode activated.", "af_bella", 100).is_none(),
                "{keep} bytes"
            );
        }
    }

    /// The file says what it was made for, and is believed only if that is
    /// what was asked for — whatever it happens to be called.
    #[test]
    fn a_file_under_another_sentences_name_is_not_played() {
        let (_dir, store) = store();
        store.save("Quiet mode activated.", "af_bella", 100, &speech(24_000)).unwrap();
        let quiet = only_file(&store);
        let aware = store.path_for(&key("Aware mode activated.", "af_bella", 100));
        std::fs::rename(&quiet, &aware).unwrap();
        assert!(store.load("Aware mode activated.", "af_bella", 100).is_none());
    }

    #[test]
    fn silence_and_overlong_audio_are_not_kept() {
        let (_dir, store) = store();
        let nothing = Speech { samples: Vec::new(), sample_rate: 24_000 };
        assert!(store.save("x", "af", 100, &nothing).is_err());
        let endless = speech(24_000 * (MAX_SECONDS as usize + 1));
        assert!(store.save("x", "af", 100, &endless).is_err());
    }

    /// Out-of-range samples are clipped when kept, as they are when played.
    #[test]
    fn samples_beyond_full_scale_are_clipped() {
        let (_dir, store) = store();
        let loud = Speech { samples: vec![2.0, -3.0, 0.5], sample_rate: 24_000 };
        store.save("x", "af", 100, &loud).unwrap();
        let back = store.load("x", "af", 100).unwrap();
        assert_eq!(back.samples[0], 1.0);
        assert_eq!(back.samples[1], -1.0);
        assert!((back.samples[2] - 0.5).abs() < 0.001);
    }

    #[test]
    fn the_least_recently_used_are_dropped_beyond_the_limit() {
        let (_dir, store) = store();
        let clip = speech(2_400);
        let old = SystemTime::now() - std::time::Duration::from_secs(3_600);
        for i in 0..MAX_FILES {
            let text = format!("Battery low. {i} percent.");
            store.save(&text, "af", 100, &clip).unwrap();
            // Made an hour ago, each a little later than the one before.
            let path = store.path_for(&key(&text, "af", 100));
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(old + std::time::Duration::from_secs(i as u64)).unwrap();
        }
        // The oldest is used again, and then one more sentence arrives.
        assert!(store.load("Battery low. 0 percent.", "af", 100).is_some());
        store.save("Quiet mode activated.", "af", 100, &clip).unwrap();

        assert!(store.load("Quiet mode activated.", "af", 100).is_some());
        assert!(store.load("Battery low. 0 percent.", "af", 100).is_some(), "just used");
        assert!(store.load("Battery low. 1 percent.", "af", 100).is_none(), "the oldest unused");
        assert!(store.load("Battery low. 2 percent.", "af", 100).is_some());
    }
}
