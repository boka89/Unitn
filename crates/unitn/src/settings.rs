use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Where Unitn keeps its files.
#[derive(Clone)]
pub struct Paths {
    pub log: PathBuf,
    pub database: PathBuf,
    pub covers: PathBuf,
    /// Downloaded artist photos, named after the artist.
    pub artists: PathBuf,
    /// Small copies (320px JPEG) of album covers and artist photos, for the
    /// Albums and Artists grids.
    pub thumbs: PathBuf,
    /// The queue, current song and position, kept for the next start.
    pub session: PathBuf,
    pub settings: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("net", "bokors", "Unitn")
            .ok_or_else(|| anyhow!("could not determine the user's data directory"))?;
        let data = dirs.data_dir().to_path_buf();
        let config = dirs.config_dir().to_path_buf();
        std::fs::create_dir_all(&data)?;
        std::fs::create_dir_all(&config)?;
        Ok(Self {
            log: data.join("unitn.log"),
            database: data.join("library.sqlite3"),
            covers: data.join("covers"),
            artists: data.join("artists"),
            thumbs: data.join("thumbs"),
            session: data.join("session.json"),
            settings: config.join("settings.json"),
        })
    }
}

/// Theme mode as used by the UI: follow the OS, or force light/dark.
pub const THEME_SYSTEM: i32 = 0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0 = follow system, 1 = light, 2 = dark.
    pub theme: i32,
    pub volume: f32,
    pub shuffle: bool,
    /// 0 = off, 1 = repeat all, 2 = repeat one.
    pub repeat: i32,
    /// Window opacity, 0.5 (see-through) to 1.0 (solid).
    pub opacity: f32,
    /// Visualizer style: 0 Spectrum, 1 Mirror, 2 Wave, 3 Oscilloscope, 4 Radial.
    pub viz_style: i32,
    /// Look up missing album art on MusicBrainz / Cover Art Archive.
    pub download_art: bool,
    /// "Cozy" (compact) song lists.
    pub compact_lists: bool,
    pub eq_enabled: bool,
    /// Gain per band in dB (31 Hz … 16 kHz).
    pub eq_gains: [f32; 10],
    /// Selected preset, or -1 for custom.
    pub eq_preset: i32,
    pub eq_preamp: f32,
    /// The equalizer section is shown on Now Playing.
    pub eq_open: bool,
    /// Frosted-glass (blurred) window background, on macOS. Saved under a new
    /// name: 0.3.1 stored "glass" and its blur hid the whole window.
    #[serde(rename = "frosted_glass")]
    pub glass: bool,
    /// Connected music servers.
    pub servers: Vec<unitn_core::servers::ServerConfig>,
    /// Crossfade between songs in seconds (0 = off, gapless).
    pub crossfade: i32,
    /// Don't crossfade between songs of the same album.
    pub gapless_albums: bool,
    /// Look up lyrics on LRCLIB when there are none locally.
    pub lyrics_online: bool,
    /// The lyrics panel is shown on Now Playing.
    pub lyrics_open: bool,
    /// Play the sound of the startup animation.
    pub startup_sound: bool,
    /// Window size (logical pixels) when the app was last closed.
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
    pub window_maximized: bool,
    /// Window position on the screen (physical pixels, including the frame).
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// The app was closed as the mini player: it starts as one again.
    pub mini_player: bool,
    /// The mini player's position (physical pixels) and width (logical).
    pub mini_x: Option<i32>,
    pub mini_y: Option<i32>,
    pub mini_width: Option<f32>,
    /// The mini player shows the line of lyrics being sung.
    pub mini_lyrics: bool,
    /// Sort order of the Albums / Artists / Songs pages (an index into
    /// `unitn_core::sort::*_ORDERS`) and whether its direction is flipped.
    pub album_sort: usize,
    pub album_sort_reversed: bool,
    pub artist_sort: usize,
    pub artist_sort_reversed: bool,
    pub song_sort: usize,
    pub song_sort_reversed: bool,
    /// Sorting skips a leading "The", "A" or "An" (The Beatles under B).
    pub sort_ignore_articles: bool,
    /// On start, the last song is back on the player (paused where it was),
    /// with the queue.
    pub resume_session: bool,
    /// Save lyrics found online with the song: 0 no, 1 as a file next to it,
    /// 2 in the song's tags, 3 both.
    pub lyrics_autosave: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: THEME_SYSTEM, volume: 0.8, shuffle: false, repeat: 0, opacity: 1.0, viz_style: 0, download_art: true, compact_lists: false, eq_enabled: false, eq_gains: [0.0; 10], eq_preset: 0, eq_preamp: 0.0, eq_open: false, glass: false, servers: Vec::new(), crossfade: 0, gapless_albums: false, lyrics_online: true, lyrics_open: false, startup_sound: true, window_width: None, window_height: None, window_maximized: false, window_x: None, window_y: None, mini_player: false, mini_x: None, mini_y: None, mini_width: None, mini_lyrics: false, album_sort: 0, album_sort_reversed: false, artist_sort: 0, artist_sort_reversed: false, song_sort: 0, song_sort_reversed: false, sort_ignore_articles: true, resume_session: true, lyrics_autosave: 0 }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if let Err(e) = std::fs::write(path, json) {
                eprintln!("unitn: could not save settings: {e}");
            }
        }
    }
}
