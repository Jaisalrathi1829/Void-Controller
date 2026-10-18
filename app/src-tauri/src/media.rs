//! What is playing on this computer, and its controls.
//!
//! From Windows' own media sessions — the ones the volume flyout shows — so
//! it is whatever application is playing: a music app, a browser tab, a video
//! player. Nothing here is the headphones': they play what the computer
//! sends them, and this is that.
//!
//! Only what the playing application reports is shown, and only the controls
//! it says are available are offered. Its artwork is read from the session
//! and handed to the window as data; nothing is fetched from anywhere.

use serde::{Deserialize, Serialize};

/// Whether something is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    /// Opening, changing track: about to be one of the others.
    Changing,
}

/// The repeat setting, where the application has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepeatMode {
    Off,
    Track,
    List,
}

/// What is playing, as the playing application reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The application playing it.
    pub app: String,
    /// The artwork, as a `data:` URL. `None` when there is none.
    pub artwork: Option<String>,
    pub state: PlaybackState,
    /// How far in, at `sampled_at_ms`.
    pub position_ms: u64,
    /// How long it is. `None` when the application does not say.
    pub duration_ms: Option<u64>,
    /// When the position was true, in milliseconds since 1970. While playing,
    /// the position now is this much later.
    pub sampled_at_ms: i64,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub can_seek: bool,
    /// Shuffle, where the application lets it be changed.
    pub shuffle: Option<bool>,
    /// Repeat, where the application lets it be changed.
    pub repeat: Option<RepeatMode>,
}

/// One thing to ask the playing application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    #[serde(rename_all = "camelCase")]
    Seek {
        position_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    Shuffle {
        on: bool,
    },
    #[serde(rename_all = "camelCase")]
    Repeat {
        mode: RepeatMode,
    },
}

/// A readable name for the application, from the id Windows knows it by:
/// `Spotify.exe`, `chrome`, `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify`.
pub fn app_name(id: &str) -> String {
    let id = id.rsplit('!').next().unwrap_or(id);
    let id = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let id = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    let known = match id.to_ascii_lowercase().as_str() {
        "chrome" => Some("Chrome"),
        "msedge" | "microsoftedge" => Some("Edge"),
        "firefox" => Some("Firefox"),
        "brave" => Some("Brave"),
        "opera" => Some("Opera"),
        "spotify" => Some("Spotify"),
        "vlc" => Some("VLC"),
        "microsoft.zunemusic" | "zunemusic" | "music.ui" => Some("Media Player"),
        "applemusic" | "applemusic.exe" => Some("Apple Music"),
        "amazon music" | "amazonmusic" => Some("Amazon Music"),
        _ => None,
    };
    match known {
        Some(name) => name.to_string(),
        None => {
            let mut chars = id.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

/// Bytes as base64, for a `data:` URL.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Windows' file time (100-nanosecond ticks since 1601) as milliseconds
/// since 1970.
fn unix_ms(ticks: i64) -> i64 {
    (ticks - 116_444_736_000_000_000) / 10_000
}

#[cfg(windows)]
mod win {
    use super::*;
    use parking_lot::Mutex;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Media::MediaPlaybackAutoRepeatMode as Repeat;
    use windows::Storage::Streams::DataReader;

    /// The largest artwork that is passed on.
    const MOST_ARTWORK: u32 = 4 * 1024 * 1024;

    /// The last artwork read, by the track it belongs to: reading it is the
    /// one costly part, and it does not change while the track plays.
    static ARTWORK: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);

    fn session() -> Result<Option<Session>, String> {
        let manager = Manager::RequestAsync()
            .and_then(|op| op.get())
            .map_err(|e| format!("the media sessions could not be read: {e}"))?;
        Ok(manager.GetCurrentSession().ok())
    }

    fn artwork(session: &Session, key: &str) -> Option<String> {
        if let Some((cached_key, art)) = ARTWORK.lock().as_ref() {
            if cached_key == key {
                return art.clone();
            }
        }
        let read = || -> windows::core::Result<Option<String>> {
            let props = session.TryGetMediaPropertiesAsync()?.get()?;
            let Ok(thumbnail) = props.Thumbnail() else {
                return Ok(None);
            };
            let stream = thumbnail.OpenReadAsync()?.get()?;
            let size = stream.Size()?;
            if size == 0 || size > u64::from(MOST_ARTWORK) {
                return Ok(None);
            }
            let kind = stream.ContentType()?.to_string();
            let reader = DataReader::CreateDataReader(&stream)?;
            reader.LoadAsync(size as u32)?.get()?;
            let mut bytes = vec![0u8; size as usize];
            reader.ReadBytes(&mut bytes)?;
            let kind = if kind.starts_with("image/") { kind } else { "image/png".to_string() };
            Ok(Some(format!("data:{kind};base64,{}", base64(&bytes))))
        };
        let art = read().ok().flatten();
        *ARTWORK.lock() = Some((key.to_string(), art.clone()));
        art
    }

    pub fn now_playing() -> Result<Option<NowPlaying>, String> {
        let Some(session) = session()? else {
            return Ok(None);
        };
        let read = || -> windows::core::Result<Option<NowPlaying>> {
            let props = session.TryGetMediaPropertiesAsync()?.get()?;
            let title = props.Title()?.to_string();
            let artist = props.Artist()?.to_string();
            let album = props.AlbumTitle()?.to_string();
            if title.trim().is_empty() && artist.trim().is_empty() {
                return Ok(None);
            }
            let app = app_name(&session.SourceAppUserModelId()?.to_string());

            let info = session.GetPlaybackInfo()?;
            let state = match info.PlaybackStatus()? {
                Status::Playing => PlaybackState::Playing,
                Status::Paused => PlaybackState::Paused,
                Status::Changing | Status::Opened => PlaybackState::Changing,
                _ => PlaybackState::Stopped,
            };
            let controls = info.Controls()?;
            let can_play_pause = controls.IsPlayPauseToggleEnabled()?
                || controls.IsPlayEnabled()?
                || controls.IsPauseEnabled()?;
            let shuffle = if controls.IsShuffleEnabled()? {
                info.IsShuffleActive().ok().and_then(|v| v.Value().ok())
            } else {
                None
            };
            let repeat = if controls.IsRepeatEnabled()? {
                info.AutoRepeatMode().ok().and_then(|v| v.Value().ok()).map(|m| match m {
                    Repeat::Track => RepeatMode::Track,
                    Repeat::List => RepeatMode::List,
                    _ => RepeatMode::Off,
                })
            } else {
                None
            };

            let timeline = session.GetTimelineProperties()?;
            let start = timeline.StartTime()?.Duration;
            let end = timeline.EndTime()?.Duration;
            let position = (timeline.Position()?.Duration - start).max(0);
            let duration = end - start;
            let sampled = timeline.LastUpdatedTime()?.UniversalTime;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or_default();
            // A time from before the epoch, or from the future, is not one
            // to count on: then the position is taken as of now.
            let sampled_at_ms = match unix_ms(sampled) {
                ms if ms > 0 && ms <= now_ms => ms,
                _ => now_ms,
            };

            let key = format!("{app}\u{1f}{title}\u{1f}{artist}\u{1f}{album}");
            Ok(Some(NowPlaying {
                artwork: artwork(&session, &key),
                title,
                artist,
                album,
                app,
                state,
                position_ms: (position / 10_000) as u64,
                duration_ms: (duration > 0).then_some((duration / 10_000) as u64),
                sampled_at_ms,
                can_play_pause,
                can_next: controls.IsNextEnabled()?,
                can_previous: controls.IsPreviousEnabled()?,
                can_seek: controls.IsPlaybackPositionEnabled()? && duration > 0,
                shuffle,
                repeat,
            }))
        };
        read().map_err(|e| format!("what is playing could not be read: {e}"))
    }

    pub fn control(action: MediaAction) -> Result<bool, String> {
        let Some(session) = session()? else {
            return Err("Nothing is playing.".to_string());
        };
        let asked = match action {
            MediaAction::PlayPause => session.TryTogglePlayPauseAsync(),
            MediaAction::Next => session.TrySkipNextAsync(),
            MediaAction::Previous => session.TrySkipPreviousAsync(),
            MediaAction::Seek { position_ms } => {
                let start = session
                    .GetTimelineProperties()
                    .and_then(|t| t.StartTime())
                    .map(|t| t.Duration)
                    .unwrap_or(0);
                session.TryChangePlaybackPositionAsync(start + position_ms as i64 * 10_000)
            }
            MediaAction::Shuffle { on } => session.TryChangeShuffleActiveAsync(on),
            MediaAction::Repeat { mode } => session.TryChangeAutoRepeatModeAsync(match mode {
                RepeatMode::Off => Repeat::None,
                RepeatMode::Track => Repeat::Track,
                RepeatMode::List => Repeat::List,
            }),
        };
        asked
            .and_then(|op| op.get())
            .map_err(|e| format!("the request could not be made: {e}"))
    }
}

/// What is playing now. `None` when nothing is, and always on a phone.
pub fn now_playing() -> Result<Option<NowPlaying>, String> {
    #[cfg(windows)]
    {
        win::now_playing()
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

/// Asks the playing application to do something. `Ok(false)` when it said
/// it would not.
pub fn control(action: MediaAction) -> Result<bool, String> {
    #[cfg(windows)]
    {
        win::control(action)
    }
    #[cfg(not(windows))]
    {
        let _ = action;
        Err("Media controls are on the Windows application.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applications_are_named_readably() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(app_name("chrome"), "Chrome");
        assert_eq!(app_name("MSEdge"), "Edge");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(app_name("foobar2000.exe"), "Foobar2000");
        assert_eq!(app_name(""), "");
    }

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn windows_time_is_converted_to_unix_time() {
        assert_eq!(unix_ms(116_444_736_000_000_000), 0);
        assert_eq!(unix_ms(116_444_736_000_000_000 + 10_000), 1);
    }

    #[test]
    fn actions_are_read_from_the_window() {
        let a: MediaAction = serde_json::from_str(r#"{"kind":"seek","positionMs":12000}"#).unwrap();
        assert_eq!(a, MediaAction::Seek { position_ms: 12000 });
        let a: MediaAction = serde_json::from_str(r#"{"kind":"repeat","mode":"list"}"#).unwrap();
        assert_eq!(a, MediaAction::Repeat { mode: RepeatMode::List });
        assert!(serde_json::from_str::<MediaAction>(r#"{"kind":"eject"}"#).is_err());
    }

    /// Read-only: prints what Windows says is playing.
    #[test]
    #[ignore = "reads this computer's media sessions; diagnostic output only"]
    fn print_now_playing() {
        match now_playing() {
            Ok(Some(np)) => {
                let art = np.artwork.as_ref().map(|a| a.len()).unwrap_or(0);
                println!(
                    "  {} — {} [{}] on {} ({:?}) {}ms / {:?}ms, art {} chars, play/pause {} next {} prev {} seek {} shuffle {:?} repeat {:?}",
                    np.title, np.artist, np.album, np.app, np.state, np.position_ms, np.duration_ms,
                    art, np.can_play_pause, np.can_next, np.can_previous, np.can_seek, np.shuffle, np.repeat
                );
            }
            Ok(None) => println!("  nothing playing"),
            Err(e) => println!("  error: {e}"),
        }
    }
}
