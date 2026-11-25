//! Application state and the glue between the UI and `unitn-core`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use unitn_core::artwork::{AlbumCandidate, ArtFetcher};
use unitn_core::audio::eq::EqSettings;
use unitn_core::audio::{PlayState, Player, PlayerEvent, Source, Spectrum};
use unitn_core::library::{m3u, scanner, EpisodeState, Library, Podcast};
use unitn_core::lyrics::{self, Lyrics};
use unitn_core::podcasts::{self, Show};
use unitn_core::model::{Album, Artist, Playlist, Station, Track};
use unitn_core::radio::RadioBrowser;
use unitn_core::servers::{self, ServerAlbum, ServerConfig, ServerKind};
use unitn_core::log::log;
use unitn_core::util::format_time;

use crate::media::{Media, MediaCmd};
use crate::settings::{Paths, Settings};
use crate::{
    AlbumItem, AppWindow, ArtistItem, DayBar, Dnd, EpisodeItem, FixCandidate, PlaylistItem, PodcastItem, QueueEntry, ServerItem,
    StatRow, StatTile, StationItem, Theme, TrackItem, VizInfo,
};

const RADIO_LIMIT: u32 = 150;
/// Bars in the large visualizer; the mini one in the player bar groups them by four.
const VIZ_BARS: usize = 64;
const RADIO_TAB_POPULAR: i32 = 0;
const RADIO_TAB_FAVORITES: i32 = 1;
/// The browse buttons on the Radio page, in their order there.
const RADIO_PICK_COUNTRY: i32 = 0;
const RADIO_PICK_LANGUAGE: i32 = 1;
const RADIO_PICK_TAG: i32 = 2;
const RADIO_PICK_CODEC: i32 = 3;
const RADIO_PICK_BITRATE: i32 = 4;
const RADIO_FILTER_NAMES: [&str; 5] = ["Country", "Language", "Tag", "Format", "Bitrate"];
/// Minimum bitrates offered by the Bitrate button (0 = any).
const RADIO_BITRATES: [u32; 5] = [0, 64, 128, 192, 320];
/// At most this many rows in a browse list (the tag list is long).
const RADIO_PICKER_ROWS: usize = 400;
/// Genre chips on the Radio page, in their order there (Radio Browser tags).
const RADIO_GENRES: [&str; 6] = ["pop", "rock", "jazz", "classical", "electronic", "news"];
/// No chip selected (search results or a genre).
const RADIO_TAB_OTHER: i32 = -1;

/// Results of background work, delivered to the UI thread.
enum Bg {
    ScanProgress(usize),
    /// Artist photos found (in music folders, already downloaded, or just
    /// downloaded) for the Artists page: (artist, image file).
    ArtistPhotos(Vec<(String, PathBuf)>),
    ArtistPhotosDone,
    /// Album covers made small enough for lists, decoded off the UI thread.
    Thumbs(Vec<(PathBuf, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>),
    /// The cover thread finished its list.
    ThumbsDone,
    ScanDone(Result<scanner::ScanStats, String>),
    Stations { request: u64, result: Result<Vec<Station>, String> },
    /// The countries, languages, tags or formats of the radio directory.
    RadioFacets { kind: unitn_core::radio::FacetKind, result: Result<Vec<unitn_core::radio::Facet>, String> },
    /// A cover was downloaded for an album.
    ArtFound,
    /// Artist photo lookup finished.
    ArtistImage { artist: String, path: Option<PathBuf> },
    /// The online album art lookup finished (number of covers found).
    ArtDone(usize),
    /// "Fix album info" search results.
    FixResults { request: u64, result: Result<Vec<AlbumCandidate>, String> },
    /// A thumbnail for search result `index` was downloaded.
    FixThumb { request: u64, index: usize, path: PathBuf },
    /// The corrected album was saved (new artist and title).
    FixSaved(Result<(String, String, bool), String>),
    /// Signing in to a new server finished.
    ServerConnected(Result<ServerConfig, String>),
    /// Songs found on the music servers for a library search.
    ServerSearch { request: u64, tracks: Vec<Track> },
    /// Lyrics lookup finished (`lyrics` is `None` when there are none).
    Lyrics { track: PathBuf, lyrics: Option<Lyrics>, failed: bool },
    /// Podcast search (or a pasted RSS link) finished.
    PodcastResults { request: u64, result: Result<Vec<Show>, String> },
    /// Subscribing finished: the show's title, or why it failed.
    PodcastSubscribed(Result<String, String>),
    /// A show's feed was reloaded.
    PodcastRefreshed { feed: String, result: Result<(), String> },
    /// "Sign in with Plex": waiting for the user to approve in the browser.
    PlexWaiting { login: u64, code: Option<String>, opened: bool },
    /// The Plex sign-in was approved in the browser; connecting to the server.
    PlexApproved { login: u64 },
    /// The album list of a server arrived.
    ServerAlbums { request: u64, key: String, result: Result<Vec<ServerAlbum>, String> },
    /// A server album cover was downloaded.
    ServerCover { request: u64, id: String, path: PathBuf },
    /// The songs of a server album arrived.
    ServerTracks { request: u64, album: ServerAlbum, result: Result<Vec<Track>, String> },
}

/// What is shown for the open server.
#[derive(Default)]
struct ServerView {
    index: Option<usize>,
    /// All albums, per server (url|user), loaded once per session.
    cache: HashMap<String, Vec<ServerAlbum>>,
    /// The albums shown.
    shown: Vec<ServerAlbum>,
    model: Option<Rc<VecModel<AlbumItem>>>,
}

fn server_key(cfg: &ServerConfig) -> String {
    format!("{}|{}", cfg.url, cfg.user)
}

fn server_cover_key(cfg_key: &str, album_id: &str) -> String {
    format!("server\u{1f}{cfg_key}\u{1f}{album_id}")
}

/// State of the "Fix album info" dialog.
#[derive(Default)]
struct FixState {
    /// The album being corrected (grouping artist, title).
    target: Option<(String, String)>,
    candidates: Vec<AlbumCandidate>,
    selected: Option<usize>,
    local_cover: Option<PathBuf>,
    rows: Vec<FixCandidate>,
    saving: bool,
}

/// The song being listened to, for the Statistics page: how much of it was
/// heard (seeking doesn't count) and where it was at the last update.
struct Listen {
    track: Track,
    heard: f64,
    last: f64,
}

/// Statistics page.
const PAGE_STATS: i32 = 11;
/// Rows in each top list.
const STATS_TOP: usize = 10;
/// Days in the listening chart.
const STATS_DAYS: u32 = 30;

enum StationQuery {
    Top,
    /// Name search and/or filters.
    Filtered(unitn_core::radio::Filter),
}

/// Something in the play queue.
#[derive(Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
enum Item {
    Track(Track),
    Station(Station),
}

/// What's playing, kept between starts ("Continue where I left off").
#[derive(serde::Serialize, serde::Deserialize)]
struct Session {
    queue: Vec<Item>,
    current: Option<usize>,
    position: f64,
}

/// Loads a cover and shrinks it to list size (at most 400 px, enough for the
/// 200 px album page cover on high-DPI screens). Runs on the cover thread.
const THUMB_SIZE: u32 = 320;

/// Full-size images (Now Playing cover, artist photo behind it) are made no
/// bigger than this, and only the last few are kept in memory.
const COVER_MAX: u32 = 1920;
const COVER_CACHE: usize = 3;

/// Loads a full-size image, shrunk to at most [`COVER_MAX`] on its longer side.
fn decode_cover(path: &Path) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > COVER_MAX || img.height() > COVER_MAX {
        img.resize(COVER_MAX, COVER_MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(slint::SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}

/// Where the small copy of `path` is kept in the thumbnail folder. The name
/// depends on the file's path, size and modification time, so an edited or
/// replaced image gets a new thumbnail.
fn thumb_file(path: &Path, dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    // FNV-1a: stable across Rust versions (unlike DefaultHasher).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().bytes().chain(meta.len().to_le_bytes()).chain(mtime.to_le_bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    Some(dir.join(format!("{h:016x}.jpg")))
}

/// A cover or artist photo at grid size (at most [`THUMB_SIZE`]). It is read
/// from the thumbnail folder when it's there; otherwise the original is
/// decoded once, shrunk and saved there, so later visits only read a small
/// JPEG instead of decoding a big image again.
fn decode_thumb(path: &Path, dir: &Path) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let cached = thumb_file(path, dir);
    let from_cache = cached
        .as_ref()
        .and_then(|c| image::ImageReader::open(c).ok())
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.decode().ok());
    let img = match from_cache {
        Some(img) => img,
        None => {
            let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
            // 320px is sharp for the biggest album / artist cards; a decoded
            // thumbnail costs width × height × 4 bytes of memory.
            let img = if img.width() > THUMB_SIZE || img.height() > THUMB_SIZE { img.thumbnail(THUMB_SIZE, THUMB_SIZE) } else { img };
            if let Some(c) = &cached {
                let _ = std::fs::create_dir_all(dir);
                let rgb = img.to_rgb8();
                if let Ok(file) = std::fs::File::create(c) {
                    let mut out = std::io::BufWriter::new(file);
                    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
                    if rgb.write_with_encoder(enc).is_err() {
                        drop(out);
                        let _ = std::fs::remove_file(c);
                    }
                }
            }
            img
        }
    };
    let rgba = img.to_rgba8();
    Some(slint::SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}


/// A podcast episode (its path is a made-up "podcast://…" key).
fn is_episode(t: &Track) -> bool {
    t.path.to_string_lossy().starts_with("podcast://")
}

fn track_key(t: &Track) -> String {
    t.path.to_string_lossy().into_owned()
}

/// Identifies an album by artist and title, the same way for local and
/// server albums, so the album that is playing can be marked.
fn album_key(artist: &str, title: &str) -> String {
    if title.trim().is_empty() {
        return String::new();
    }
    format!("{}\u{1f}{}", artist.trim().to_lowercase(), title.trim().to_lowercase())
}

fn station_key(s: &Station) -> String {
    format!("radio:{}", s.uuid)
}

/// The browse list a Radio page button opens, for the ones the directory lists.
fn facet_kind(which: i32) -> Option<unitn_core::radio::FacetKind> {
    use unitn_core::radio::FacetKind;
    match which {
        RADIO_PICK_COUNTRY => Some(FacetKind::Country),
        RADIO_PICK_LANGUAGE => Some(FacetKind::Language),
        RADIO_PICK_TAG => Some(FacetKind::Tag),
        RADIO_PICK_CODEC => Some(FacetKind::Codec),
        _ => None,
    }
}

fn facet_slot(kind: unitn_core::radio::FacetKind) -> usize {
    use unitn_core::radio::FacetKind;
    match kind {
        FacetKind::Country => 0,
        FacetKind::Language => 1,
        FacetKind::Tag => 2,
        FacetKind::Codec => 3,
    }
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

fn initial(name: &str) -> SharedString {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "♪".into())
        .into()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

// ---- Visualizer paths (SVG commands in a 1000 × 1000 viewbox) ----

/// Smooth spectrum curve: (filled area, outline), `width` × 1000 viewbox.
fn wave_paths(bars: &[f32], width: f32) -> (String, String) {
    use std::fmt::Write;
    let n = bars.len().max(2);
    let pts: Vec<(f32, f32)> = (0..n)
        .map(|i| (i as f32 / (n - 1) as f32 * width, 985.0 - bars.get(i).copied().unwrap_or(0.0) * 930.0))
        .collect();
    let mut line = format!("M {:.1} {:.1}", pts[0].0, pts[0].1);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (pts[i.saturating_sub(1)], pts[i], pts[i + 1], pts[(i + 2).min(n - 1)]);
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let _ = write!(line, " C {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
    }
    let area = format!("{line} L {width:.1} 1000 L 0 1000 Z");
    (area, line)
}

/// Oscilloscope trace with automatic gain, `width` × 1000 viewbox.
fn scope_path(wave: &[f32], width: f32) -> String {
    use std::fmt::Write;
    let peak = wave.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = (0.9 / peak.max(0.05)).min(6.0);
    let n = wave.len().max(2);
    let mut path = String::with_capacity(n * 14);
    for (i, s) in wave.iter().enumerate() {
        let x = i as f32 / (n - 1) as f32 * width;
        let y = 500.0 - (s * gain).clamp(-1.0, 1.0) * 460.0;
        let _ = write!(path, "{} {:.1} {:.1} ", if i == 0 { "M" } else { "L" }, x, y);
    }
    path
}

/// Symmetric ring of spokes around the album art.
fn radial_path(bars: &[f32]) -> String {
    use std::fmt::Write;
    let n = bars.len().max(1);
    let spokes = n * 2;
    let mut path = String::with_capacity(spokes * 32);
    for k in 0..spokes {
        let v = bars[if k < n { k } else { spokes - 1 - k }.min(n - 1)];
        let angle = -std::f32::consts::FRAC_PI_2 + k as f32 / spokes as f32 * std::f32::consts::TAU;
        let (r0, r1) = (225.0, 240.0 + v * 245.0);
        let (c, s) = (angle.cos(), angle.sin());
        let _ = write!(path, "M {:.1} {:.1} L {:.1} {:.1} ", 500.0 + c * r0, 500.0 + s * r0, 500.0 + c * r1, 500.0 + s * r1);
    }
    path
}

/// Equalizer presets, in the order shown in Settings.
const EQ_PRESETS: [[f32; 10]; 10] = [
    [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],       // Flat
    [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],       // Bass boost
    [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 4.0, 5.0, 6.0],       // Treble boost
    [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.5, 0.0, -1.0],   // Vocal
    [5.0, 4.0, 2.0, -1.0, -2.0, -1.0, 1.0, 3.0, 4.0, 5.0],    // Rock
    [-1.0, 1.0, 3.0, 4.0, 3.0, 0.0, -1.0, -1.0, 1.0, 2.0],    // Pop
    [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0],     // Jazz
    [4.0, 3.0, 2.0, 1.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0],     // Classical
    [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 1.0, 4.0, 5.0],      // Electronic
    [6.0, 4.0, 1.0, 0.0, -1.0, 0.0, -1.0, 0.0, 3.0, 4.0],     // Loudness
];

/// Smooth SVG path through the band gains, for a viewbox of
/// (100 × aspect) × 100 (the display's own aspect ratio).
fn eq_curve(gains: &[f32; 10], aspect: f32) -> String {
    let w = 100.0 * aspect.clamp(0.2, 20.0);
    let pts: Vec<(f32, f32)> = gains
        .iter()
        .enumerate()
        .map(|(i, g)| ((0.05 + i as f32 * 0.1) * w, 50.0 - g.clamp(-12.0, 12.0) / 12.0 * 44.0))
        .collect();
    let mut d = format!("M 0 {:.1} L {:.1} {:.1}", pts[0].1, pts[0].0, pts[0].1);
    // Catmull-Rom through the points, as cubic Béziers.
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        d += &format!(" C {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
    }
    d += &format!(" L {w:.1} {:.1}", pts[9].1);
    d
}

fn long_duration(ms: u64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as u64;
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// The playlist the Like button on Now Playing fills. Other features
/// (playing Likes shuffled) look it up by this exact name.
pub const LIKES_PLAYLIST: &str = "Likes";

/// Marks drag payloads that started in Unitn (see `App::drop_items`).
pub const DRAG_PREFIX: &str = "unitn:";

pub struct App {
    ui: Weak<AppWindow>,
    paths: Paths,
    settings: Settings,
    library: Library,
    player: Player,
    player_events: Receiver<PlayerEvent>,
    media: Media,
    media_rx: Receiver<MediaCmd>,
    radio: RadioBrowser,
    bg_tx: Sender<Bg>,
    bg_rx: Receiver<Bg>,

    covers: HashMap<PathBuf, Option<Image>>,
    /// Paths in `covers`, least recently used first.
    cover_order: std::collections::VecDeque<PathBuf>,
    /// The Artists page is shown (its pictures and photo lookups are wanted).
    artist_pictures: bool,
    /// The page shown (0 Albums, 1 Artists, …).
    page: i32,
    /// Albums / artists on screen in their grid, with a screen's margin
    /// above and below: only their pictures are kept in memory.
    album_range: Option<(usize, usize)>,
    artist_range: Option<(usize, usize)>,
    albums: Vec<Album>,
    artist_filter: Option<String>,
    album_tracks: Vec<Track>,
    open_album: Option<Album>,
    album_from_server: bool,
    server: ServerView,
    server_request: Arc<AtomicU64>,
    /// Current "Sign in with Plex" attempt; bumping it cancels the old one.
    plex_login: Arc<AtomicU64>,
    /// Current library search, so late server results can be ignored.
    search_request: Arc<AtomicU64>,
    /// Lyrics of the current song, and the song they were asked for.
    lyrics: Option<Lyrics>,
    lyrics_for: Option<PathBuf>,
    lyrics_line: Option<usize>,
    podcasts: Vec<Podcast>,
    podcast_results: Vec<Show>,
    podcast_request: Arc<AtomicU64>,
    /// Feed of the show that is open.
    podcast_feed: Option<String>,
    podcast_episodes: Vec<EpisodeState>,
    /// Episodes started in this session: song key → (feed, guid).
    episode_keys: HashMap<PathBuf, (String, String)>,
    /// Where to continue an episode once it starts playing.
    resume_at: Option<(PathBuf, f64)>,
    episode_saved_at: f64,
    fix: FixState,
    fix_request: Arc<AtomicU64>,
    artists: Vec<Artist>,
    songs: Vec<Track>,
    query: String,
    scanning: bool,
    rescan_pending: bool,

    stations: Vec<Station>,
    favorites: HashSet<String>,
    radio_tab: i32,
    radio_request: u64,
    /// Radio filters; several can be on at once (see `refresh_radio`).
    radio_favorites: bool,
    /// Picked in the browse lists: (value sent to the directory, name shown).
    radio_country: Option<(String, String)>,
    radio_language: Option<(String, String)>,
    radio_tag: Option<(String, String)>,
    radio_codec: Option<(String, String)>,
    radio_bitrate: u32,
    /// Browse lists loaded so far (country, language, tag, format).
    radio_facets: [Option<Vec<unitn_core::radio::Facet>>; 4],
    /// The open browse list (-1 none), and what each of its rows picks
    /// (None = "Any").
    radio_picker: i32,
    radio_picker_query: String,
    radio_picker_rows: Vec<Option<(String, String)>>,
    radio_genres: [bool; RADIO_GENRES.len()],
    radio_query: String,

    queue: Vec<Item>,
    queue_model: Rc<VecModel<QueueEntry>>,
    current: Option<usize>,
    history: Vec<usize>,
    state: PlayState,
    position: f64,
    duration: Option<f64>,
    stream_title: String,

    rng: u64,
    toast_timer: Timer,

    spectrum: Spectrum,
    viz_bars: Rc<VecModel<f32>>,
    viz_peaks: Rc<VecModel<f32>>,
    mini_bars: Rc<VecModel<f32>>,
    mini_peaks: Rc<VecModel<f32>>,
    viz_idle: bool,

    last_viz_style: i32,
    artist_image_pending: HashSet<String>,
    /// Photo of each artist for the Artists page (artist.jpg or downloaded).
    artist_photos: HashMap<String, PathBuf>,
    /// The Artists page's photo finder thread is running.
    artist_photos_running: bool,
    /// Small album covers for the album grid and album page.
    thumbs: HashMap<PathBuf, Option<Image>>,
    /// Covers waiting for the cover thread, and whether it runs.
    thumb_queue: Vec<PathBuf>,
    thumb_queued: HashSet<PathBuf>,
    thumbs_loading: bool,
    playlists: Vec<Playlist>,
    current_playlist: Option<usize>,
    playlist_tracks: Vec<Track>,
    /// (context, index) of the song menu that is open.
    menu_target: (i32, i32),
    /// Size, maximized state and position of the full window while the mini
    /// player is shown.
    full_window: Option<(f32, f32, bool, Option<(i32, i32)>)>,
    /// Queue index the engine was told to play next (gapless).
    planned_next: Option<usize>,
    eq_gains_model: Rc<VecModel<f32>>,
    eq_aspect: f32,
    glass_trial: Option<std::time::Instant>,
    last_eq_save: std::time::Instant,
    art_running: Option<Arc<AtomicBool>>,
    last_art_reload: std::time::Instant,
    buffering_since: Option<std::time::Instant>,
    buffering_warned: bool,
    logged_second: i64,
    listen: Option<Listen>,
    /// Statistics page: 0 = last 30 days, 1 = all time.
    stats_period: i32,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ui: &AppWindow,
        paths: Paths,
        settings: Settings,
        library: Library,
        player: Player,
        player_events: Receiver<PlayerEvent>,
        media: Media,
        media_rx: Receiver<MediaCmd>,
    ) -> Self {
        let (bg_tx, bg_rx) = mpsc::channel();
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        let queue_model = Rc::new(VecModel::default());
        ui.set_queue(ModelRc::from(queue_model.clone()));
        let zeros = |n: usize| Rc::new(VecModel::from(vec![0.0f32; n]));
        let (viz_bars, viz_peaks) = (zeros(VIZ_BARS), zeros(VIZ_BARS));
        let (mini_bars, mini_peaks) = (zeros(VIZ_BARS / 4), zeros(VIZ_BARS / 4));
        ui.set_viz_bars(ModelRc::from(viz_bars.clone()));
        ui.set_viz_peaks(ModelRc::from(viz_peaks.clone()));
        ui.set_mini_bars(ModelRc::from(mini_bars.clone()));
        ui.set_mini_peaks(ModelRc::from(mini_peaks.clone()));
        let spectrum = player.spectrum(VIZ_BARS);
        Self {
            ui: ui.as_weak(),
            paths,
            settings,
            library,
            player,
            player_events,
            media,
            media_rx,
            radio: RadioBrowser::new(),
            bg_tx,
            bg_rx,
            covers: HashMap::new(),
            cover_order: std::collections::VecDeque::new(),
            artist_pictures: false,
            page: 0,
            album_range: None,
            artist_range: None,
            albums: Vec::new(),
            artist_filter: None,
            album_tracks: Vec::new(),
            open_album: None,
            album_from_server: false,
            server: ServerView::default(),
            server_request: Arc::new(AtomicU64::new(0)),
            plex_login: Arc::new(AtomicU64::new(0)),
            search_request: Arc::new(AtomicU64::new(0)),
            lyrics: None,
            lyrics_for: None,
            lyrics_line: None,
            podcasts: Vec::new(),
            podcast_results: Vec::new(),
            podcast_request: Arc::new(AtomicU64::new(0)),
            podcast_feed: None,
            podcast_episodes: Vec::new(),
            episode_keys: HashMap::new(),
            resume_at: None,
            episode_saved_at: 0.0,
            fix: FixState::default(),
            fix_request: Arc::new(AtomicU64::new(0)),
            artists: Vec::new(),
            songs: Vec::new(),
            query: String::new(),
            scanning: false,
            rescan_pending: false,
            stations: Vec::new(),
            favorites: HashSet::new(),
            radio_tab: RADIO_TAB_POPULAR,
            radio_request: 0,
            radio_favorites: false,
            radio_country: None,
            radio_language: None,
            radio_tag: None,
            radio_codec: None,
            radio_bitrate: 0,
            radio_facets: Default::default(),
            radio_picker: -1,
            radio_picker_query: String::new(),
            radio_picker_rows: Vec::new(),
            radio_genres: [false; RADIO_GENRES.len()],
            radio_query: String::new(),
            queue: Vec::new(),
            queue_model,
            current: None,
            history: Vec::new(),
            state: PlayState::Stopped,
            position: 0.0,
            duration: None,
            stream_title: String::new(),
            rng: seed | 1,
            toast_timer: Timer::default(),
            spectrum,
            viz_bars,
            viz_peaks,
            mini_bars,
            mini_peaks,
            viz_idle: false,
            last_viz_style: -1,
            artist_image_pending: HashSet::new(),
            artist_photos: HashMap::new(),
            artist_photos_running: false,
            thumbs: HashMap::new(),
            thumb_queue: Vec::new(),
            thumb_queued: HashSet::new(),
            thumbs_loading: false,
            playlists: Vec::new(),
            current_playlist: None,
            playlist_tracks: Vec::new(),
            menu_target: (0, -1),
            full_window: None,
            planned_next: None,
            eq_gains_model: Rc::new(VecModel::from(vec![0.0f32; 10])),
            eq_aspect: 1.5,
            glass_trial: None,
            last_eq_save: std::time::Instant::now(),
            art_running: None,
            last_art_reload: std::time::Instant::now(),
            buffering_since: None,
            buffering_warned: false,
            logged_second: -1,
            listen: None,
            stats_period: 0,
        }
    }

    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Initial setup once the window is visible.
    pub fn start(&mut self) {
        if let Some(ui) = self.ui() {
            ui.invoke_apply_theme(self.settings.theme);
            ui.set_volume(self.settings.volume);
            ui.set_shuffle(self.settings.shuffle);
            ui.set_repeat_mode(self.settings.repeat);
            ui.global::<Theme>().set_opacity(self.settings.opacity.clamp(0.5, 1.0));
            ui.set_viz_style(self.settings.viz_style.clamp(0, 4));
            ui.set_download_art(self.settings.download_art);
            ui.set_crossfade(self.settings.crossfade);
            ui.set_gapless_albums(self.settings.gapless_albums);
            ui.set_lyrics_online(self.settings.lyrics_online);
            ui.set_startup_sound(self.settings.startup_sound);
            ui.set_resume_session(self.settings.resume_session);
            ui.set_lyrics_autosave(self.settings.lyrics_autosave as i32);
            ui.set_lyrics_open(self.settings.lyrics_open);
            self.show_sort();
            ui.set_sort_ignore_articles(self.settings.sort_ignore_articles);
            ui.set_glass_available(crate::glass::AVAILABLE);
            ui.set_glass(self.settings.glass && crate::glass::AVAILABLE);
            ui.global::<Theme>().set_glass(self.settings.glass && crate::glass::AVAILABLE);
            ui.global::<Theme>().set_compact(self.settings.compact_lists);
            ui.set_eq_gains(ModelRc::from(self.eq_gains_model.clone()));
            ui.set_app_version(env!("CARGO_PKG_VERSION").into());
            ui.set_log_path(self.paths.log.display().to_string().into());
        }
        self.player.set_volume(self.settings.volume);
        self.player.set_crossfade(self.settings.crossfade as f64);
        self.apply_eq();
        self.favorites = self.library.favorite_uuids().unwrap_or_default();
        // "Continue where I left off": the last song, paused.
        self.restore_session();

        // First run: use the OS music folder.
        if self.library.folders().map(|f| f.is_empty()).unwrap_or(false) {
            if let Some(music) = directories::UserDirs::new().and_then(|d| d.audio_dir().map(Path::to_path_buf)) {
                if music.is_dir() {
                    let _ = self.library.add_folder(&music);
                }
            }
        }

        let t = std::time::Instant::now();
        self.reload_library();
        log(format!("startup: library shown in {} ms", t.elapsed().as_millis()));
        self.refresh_folders();
        self.refresh_playlists();
        self.refresh_servers();
        self.refresh_podcasts();
        self.refresh_all_podcasts();
        self.load_radio_tab(RADIO_TAB_POPULAR);
        self.refresh_now_playing();
        self.refresh_position();
        self.start_scan();
        log(format!("startup: ready in {} ms", t.elapsed().as_millis()));
    }

    pub fn set_media(&mut self, media: Media) {
        self.media = media;
        self.refresh_now_playing();
        self.refresh_play_state();
    }

    /// Remembers the window size for the next start (called on exit).
    pub fn remember_window(&mut self, width: f32, height: f32, maximized: bool) {
        self.settings.window_maximized = maximized;
        if !maximized {
            self.settings.window_width = Some(width);
            self.settings.window_height = Some(height);
        }
    }

    /// Remembers where the full window or the mini player is on the screen.
    pub fn remember_position(&mut self, mini: bool, x: i32, y: i32) {
        // Windows reports -32000 for minimized windows.
        if x <= -30000 || y <= -30000 {
            return;
        }
        if mini {
            (self.settings.mini_x, self.settings.mini_y) = (Some(x), Some(y));
        } else {
            (self.settings.window_x, self.settings.window_y) = (Some(x), Some(y));
        }
    }

    pub fn remember_mini_width(&mut self, width: f32) {
        self.settings.mini_width = Some(width);
    }

    /// Switches between the full window and the mini player.
    pub fn toggle_mini_player(&mut self) {
        let Some(ui) = self.ui() else { return };
        let window = ui.window();
        if !ui.get_mini_player() {
            if ui.get_viz_fullscreen() {
                ui.set_viz_fullscreen(false);
            }
            let size = window.size().to_logical(window.scale_factor());
            let pos = window.position();
            let maximized = window.is_maximized();
            self.full_window = Some((size.width, size.height, maximized, (!maximized).then_some((pos.x, pos.y))));
            window.set_maximized(false);
            ui.set_mini_player(true);
            crate::keep_min_size(&ui);
            // The mini player comes back where it was last, at its last width.
            let width = self.settings.mini_width.filter(|w| w.is_finite()).unwrap_or(crate::MINI_DEFAULT_WIDTH);
            let width = width.clamp(crate::MINI_WIDTH.0 as f32, crate::MINI_WIDTH.1 as f32);
            window.set_size(slint::LogicalSize::new(width, crate::mini_height(&ui) as f32));
            if let Some((x, y)) = self.settings.mini_x.zip(self.settings.mini_y) {
                window.set_position(slint::PhysicalPosition::new(x, y));
                crate::keep_on_screen(&ui);
            }
            self.settings.mini_player = true;
            log("window: mini player");
        } else {
            ui.set_mini_player(false);
            crate::keep_min_size(&ui);
            // Back to the full window's size and place (also after the app
            // was started as the mini player).
            let s = &self.settings;
            let saved = (
                s.window_width.unwrap_or(1280.0).max(960.0),
                s.window_height.unwrap_or(820.0).max(620.0),
                s.window_maximized,
                s.window_x.zip(s.window_y),
            );
            let (w, h, maximized, pos) = self.full_window.take().unwrap_or(saved);
            window.set_size(slint::LogicalSize::new(w, h));
            if let Some((x, y)) = pos {
                window.set_position(slint::PhysicalPosition::new(x, y));
                crate::keep_on_screen(&ui);
            }
            if maximized {
                window.set_maximized(true);
            }
            self.settings.mini_player = false;
            log("window: full player");
        }
        self.save_settings();
        // The mini player may show lyrics while the lyrics panel is closed.
        self.load_lyrics();
    }

    /// Shows or hides the mini player's line of lyrics.
    pub fn toggle_mini_lyrics(&mut self) {
        let Some(ui) = self.ui() else { return };
        self.settings.mini_lyrics = !self.settings.mini_lyrics;
        ui.set_mini_lyrics(self.settings.mini_lyrics);
        if ui.get_mini_player() {
            crate::keep_min_size(&ui);
            let width = ui.window().size().to_logical(ui.window().scale_factor()).width;
            ui.window().set_size(slint::LogicalSize::new(width, crate::mini_height(&ui) as f32));
        }
        self.save_settings();
        self.load_lyrics();
    }

    /// Whether lyrics are shown anywhere: the lyrics panel, or the mini
    /// player's line.
    fn lyrics_wanted(&self) -> bool {
        self.settings.lyrics_open || (self.settings.mini_lyrics && self.ui().is_some_and(|ui| ui.get_mini_player()))
    }

    pub fn save_settings(&self) {
        self.settings.save(&self.paths.settings);
    }

    fn show_toast(&mut self, message: &str) {
        let Some(ui) = self.ui() else { return };
        ui.set_toast(message.into());
        let weak = self.ui.clone();
        self.toast_timer.start(TimerMode::SingleShot, Duration::from_secs(5), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_toast(SharedString::new());
            }
        });
    }

    // ------------------------------------------------------------------
    // Event pump
    // ------------------------------------------------------------------

    pub fn poll(&mut self) {
        self.tick_glass_trial();
        // One event at a time, so nothing is lost if handling one goes wrong.
        while let Ok(e) = self.player_events.try_recv() {
            self.on_player_event(e);
        }
        if self.state == PlayState::Buffering && !self.buffering_warned {
            if let Some(since) = self.buffering_since {
                if since.elapsed() > Duration::from_secs(12) {
                    self.buffering_warned = true;
                    log("ui: still buffering after 12 s");
                    self.show_toast("Still buffering… if this persists, please share the log file");
                }
            }
        }
        let jobs: Vec<Bg> = self.bg_rx.try_iter().collect();
        for j in jobs {
            self.on_background(j);
        }
        let keys: Vec<MediaCmd> = self.media_rx.try_iter().collect();
        for k in keys {
            self.on_media_key(k);
        }
    }

    /// Advances the visualizer animation (called ~30 times per second).
    pub fn tick_visualizer(&mut self) {
        let active = self.spectrum.update();
        let style_changed = self.update_viz_paths(active);
        if !active && self.viz_idle && !style_changed {
            return; // already drawn at rest
        }
        self.viz_idle = !active;
        let bars = self.spectrum.bars();
        let peaks = self.spectrum.peaks();
        for (i, v) in bars.iter().enumerate() {
            self.viz_bars.set_row_data(i, *v);
        }
        for (i, v) in peaks.iter().enumerate() {
            self.viz_peaks.set_row_data(i, *v);
        }
        for i in 0..self.mini_bars.row_count() {
            let group = |s: &[f32]| s.iter().skip(4 * i).take(4).cloned().fold(0.0f32, f32::max);
            self.mini_bars.set_row_data(i, group(bars));
            self.mini_peaks.set_row_data(i, group(peaks));
        }
    }

    /// Computes the path-based visualizer styles when one is on screen.
    /// Returns whether the style changed since the last frame.
    fn update_viz_paths(&mut self, active: bool) -> bool {
        let Some(ui) = self.ui() else { return false };
        let style = ui.get_viz_style();
        let changed = style != self.last_viz_style;
        self.last_viz_style = style;
        let visible = ui.get_viz_fullscreen() || ui.get_page() == 6;
        if !visible || style < 2 || (!active && !changed && self.viz_idle) {
            return changed;
        }
        let bars = self.spectrum.bars();
        let width = 1000.0 * ui.global::<VizInfo>().get_aspect().max(0.2);
        match style {
            2 => {
                let (area, line) = wave_paths(bars, width);
                ui.set_viz_path_a(area.into());
                ui.set_viz_path_b(line.into());
            }
            3 => ui.set_viz_path_a(scope_path(&self.spectrum.waveform(600), width).into()),
            _ => ui.set_viz_path_a(radial_path(bars).into()),
        }
        changed
    }

    fn apply_eq(&mut self) {
        let settings = EqSettings {
            enabled: self.settings.eq_enabled,
            gains: self.settings.eq_gains,
            preamp_db: self.settings.eq_preamp,
        };
        self.player.set_equalizer(settings);
        for (i, g) in self.settings.eq_gains.iter().enumerate() {
            if self.eq_gains_model.row_data(i) != Some(*g) {
                self.eq_gains_model.set_row_data(i, *g);
            }
        }
        let curve = eq_curve(&self.settings.eq_gains, self.eq_aspect);
        if let Some(ui) = self.ui() {
            ui.set_eq_enabled(self.settings.eq_enabled);
            ui.set_eq_preset(self.settings.eq_preset);
            ui.set_eq_preamp(self.settings.eq_preamp);
            ui.set_eq_open(self.settings.eq_open);
            ui.set_eq_curve(curve.into());
        }
        // Dragging sends many changes: save at most once a second (and on exit).
        if self.last_eq_save.elapsed() > Duration::from_secs(1) {
            self.last_eq_save = std::time::Instant::now();
            self.save_settings();
        }
    }

    pub fn set_eq_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect > 0.0 && (aspect - self.eq_aspect).abs() > 0.001 {
            self.eq_aspect = aspect;
            let curve = eq_curve(&self.settings.eq_gains, aspect);
            if let Some(ui) = self.ui() {
                ui.set_eq_curve(curve.into());
            }
        }
    }

    pub fn set_eq_open(&mut self, open: bool) {
        self.settings.eq_open = open;
        if let Some(ui) = self.ui() {
            ui.set_eq_open(open);
        }
        self.save_settings();
    }

    pub fn set_eq_preamp(&mut self, db: f32) {
        self.settings.eq_preamp = ((db * 2.0).round() / 2.0).clamp(-12.0, 12.0);
        self.settings.eq_enabled = true;
        self.apply_eq();
    }

    pub fn reset_eq(&mut self) {
        self.settings.eq_preamp = 0.0;
        self.select_eq_preset(0);
    }

    pub fn set_eq_enabled(&mut self, enabled: bool) {
        self.settings.eq_enabled = enabled;
        self.apply_eq();
    }

    pub fn select_eq_preset(&mut self, preset: i32) {
        if let Some(gains) = EQ_PRESETS.get(preset.max(0) as usize) {
            self.settings.eq_gains = *gains;
            self.settings.eq_preset = preset;
            self.settings.eq_enabled = true;
            self.apply_eq();
        }
    }

    pub fn set_eq_band(&mut self, band: usize, gain: f32) {
        if band < 10 {
            self.settings.eq_gains[band] = (gain * 2.0).round() / 2.0; // 0.5 dB steps
            self.settings.eq_preset = -1;
            self.settings.eq_enabled = true;
            self.apply_eq();
        }
    }

    pub fn set_compact_lists(&mut self, compact: bool) {
        self.settings.compact_lists = compact;
        self.save_settings();
    }

    pub fn set_viz_style(&mut self, style: i32) {
        self.settings.viz_style = style;
        self.save_settings();
    }

    fn on_player_event(&mut self, event: PlayerEvent) {
        match &event {
            PlayerEvent::Position { position, .. } => {
                // About once per second is enough for the log.
                let second = position.floor() as i64;
                if second != self.logged_second {
                    self.logged_second = second;
                    log(format!("ui: event {event:?}"));
                }
            }
            _ => log(format!("ui: event {event:?}")),
        }
        match event {
            PlayerEvent::State(s) => {
                if s == PlayState::Buffering {
                    self.buffering_since = Some(std::time::Instant::now());
                    self.buffering_warned = false;
                }
                self.state = s;
                self.refresh_play_state();
                if s == PlayState::Playing {
                    self.resume_episode();
                }
            }
            PlayerEvent::Position { position, duration } => {
                self.note_listening(position);
                self.position = position;
                if duration.is_some() {
                    self.duration = duration;
                }
                self.refresh_position();
                self.update_lyrics_line();
                self.track_episode_progress();
            }
            PlayerEvent::Advanced => {
                // Gapless: the engine already started the planned next track.
                if let Some(next) = self.planned_next.take() {
                    if let Some(cur) = self.current {
                        self.history.push(cur);
                    }
                    if let Some(item) = self.queue.get(next).cloned() {
                        self.start_listen(&item);
                    }
                    self.mark_current(Some(next));
                    self.position = 0.0;
                    self.stream_title.clear();
                    self.duration = match self.queue.get(next) {
                        Some(Item::Track(t)) => Some(t.duration_secs()).filter(|d| *d > 0.0),
                        _ => None,
                    };
                    self.refresh_now_playing();
                    self.refresh_position();
                    self.update_next();
                }
            }
            PlayerEvent::Ended => {
                if matches!(self.current_item(), Some(Item::Station(_))) {
                    self.state = PlayState::Stopped;
                    self.refresh_play_state();
                } else {
                    self.next(true);
                }
            }
            PlayerEvent::StreamTitle(title) => {
                self.stream_title = title;
                self.refresh_now_playing();
            }
            PlayerEvent::Error(message) => self.show_toast(&message),
        }
    }

    fn on_background(&mut self, job: Bg) {
        match job {
            Bg::Thumbs(done) => self.on_thumbs(done),
            Bg::ArtistPhotos(photos) => self.on_artist_photos(photos),
            Bg::ArtistPhotosDone => self.artist_photos_running = false,
            Bg::ThumbsDone => {
                self.thumbs_loading = false;
                self.load_thumbs();
            }
            Bg::ScanProgress(n) => {
                if let Some(ui) = self.ui() {
                    ui.set_library_status(format!("Scanning… {} found", plural(n, "file", "files")).into());
                }
                // Show music as it arrives on big first scans.
                if n > 0 && n % 1000 == 0 {
                    self.reload_library();
                }
            }
            Bg::ScanDone(result) => {
                self.scanning = false;
                if let Some(ui) = self.ui() {
                    ui.set_scanning(false);
                }
                match result {
                    Ok(_) => {
                        self.reload_library();
                        let n = self.library.track_count().unwrap_or(0) as usize;
                        if let Some(ui) = self.ui() {
                            ui.set_library_status(format!("{} in library", plural(n, "song", "songs")).into());
                        }
                    }
                    Err(e) => {
                        if let Some(ui) = self.ui() {
                            ui.set_library_status("Scan failed".into());
                        }
                        self.show_toast(&format!("Library scan failed: {e}"));
                    }
                }
                if self.rescan_pending {
                    self.rescan_pending = false;
                    self.start_scan();
                } else {
                    self.start_art_fetch();
                }
            }
            Bg::ArtFound => {
                // Show new covers as they arrive, without reloading too often.
                if self.last_art_reload.elapsed() > Duration::from_secs(3) {
                    self.last_art_reload = std::time::Instant::now();
                    self.reload_library();
                }
            }
            Bg::ArtistImage { artist, path } => {
                self.artist_image_pending.remove(&artist);
                if let Some(p) = &path {
                    self.on_artist_photos(vec![(artist.clone(), p.clone())]);
                }
                // A local artist.jpg, if the song has one, stays in front of
                // a downloaded photo.
                let current = match self.current_item() {
                    Some(Item::Track(t)) => {
                        t.grouping_artist() == artist
                            && (t.is_remote() || scanner::local_artist_image(&t.path).is_none())
                    }
                    _ => false,
                };
                if current {
                    self.show_backdrop(path);
                }
            }
            Bg::ArtDone(found) => {
                self.art_running = None;
                if found > 0 {
                    self.reload_library();
                    log(format!("art: downloaded {found} covers"));
                }
            }
            Bg::FixResults { request, result } => self.on_fix_results(request, result),
            Bg::ServerConnected(result) => self.on_server_connected(result),
            Bg::PlexWaiting { login, code, opened } => self.on_plex_waiting(login, code, opened),
            Bg::PlexApproved { login } => {
                if login == self.plex_login.load(Ordering::SeqCst) {
                    if let Some(ui) = self.ui() {
                        ui.set_server_dialog_status("Signed in to Plex. Connecting to your server…".into());
                    }
                }
            }
            Bg::ServerSearch { request, tracks } => self.on_server_search(request, tracks),
            Bg::Lyrics { track, lyrics, failed } => self.on_lyrics(track, lyrics, failed),
            Bg::PodcastResults { request, result } => self.on_podcast_results(request, result),
            Bg::PodcastSubscribed(result) => self.on_podcast_subscribed(result),
            Bg::PodcastRefreshed { feed, result } => self.on_podcast_refreshed(feed, result),
            Bg::ServerAlbums { request, key, result } => {
                if request != self.server_request.load(Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok(list) => {
                        log(format!("servers: {} albums loaded", list.len()));
                        self.server.cache.insert(key, list);
                        self.show_server_albums();
                    }
                    Err(e) => {
                        if let Some(ui) = self.ui() {
                            ui.set_server_loading(false);
                            ui.set_server_status(e.into());
                        }
                    }
                }
            }
            Bg::ServerCover { request, id, path } => {
                if request != self.server_request.load(Ordering::Relaxed) {
                    return;
                }
                let (image, has) = self.cover(&Some(path));
                if let (Some(model), Some(i)) = (&self.server.model, self.server.shown.iter().position(|a| a.id == id)) {
                    if let Some(mut row) = model.row_data(i) {
                        row.cover = image;
                        row.has_cover = has;
                        model.set_row_data(i, row);
                    }
                }
            }
            Bg::ServerTracks { request, album, result } => {
                if request != self.server_request.load(Ordering::Relaxed) || !self.album_from_server {
                    return;
                }
                match result {
                    Ok(tracks) => {
                        let shown = self.server_album(&album, &tracks);
                        self.show_album_tracks(shown, tracks);
                    }
                    Err(e) => {
                        if let Some(ui) = self.ui() {
                            ui.set_album_info(format!("Could not load the songs: {e}").into());
                        }
                    }
                }
            }
            Bg::FixThumb { request, index, path } => {
                if request != self.fix_request.load(Ordering::Relaxed) {
                    return;
                }
                let (thumb, has) = self.cover(&Some(path.clone()));
                if let Some(row) = self.fix.rows.get_mut(index) {
                    row.thumb = thumb;
                    row.has_thumb = has;
                }
                if self.fix.selected == Some(index) && self.fix.local_cover.is_none() {
                    self.set_fix_preview(Some(path));
                }
                self.push_fix_rows();
            }
            Bg::FixSaved(result) => {
                self.fix.saving = false;
                match result {
                    Ok((artist, album, has_cover)) => {
                        self.close_fix_dialog();
                        self.reload_library();
                        if let Some(a) = self.albums.iter().find(|a| a.artist == artist && a.title == album).cloned() {
                            self.show_album(a);
                        }
                        self.show_toast(if has_cover { "Album info and cover updated" } else { "Album info updated" });
                    }
                    Err(e) => {
                        if let Some(ui) = self.ui() {
                            ui.set_fix_busy(false);
                            ui.set_fix_status(format!("Could not save: {e}").into());
                        }
                    }
                }
            }
            Bg::RadioFacets { kind, result } => self.radio_facets_loaded(kind, result),
            Bg::Stations { request, result } => {
                if request != self.radio_request {
                    return; // a newer request superseded this one
                }
                match result {
                    Ok(list) => {
                        let status = if list.is_empty() { "No stations found".to_string() } else { plural(list.len(), "station", "stations") };
                        self.set_stations(list, status);
                    }
                    Err(e) => self.set_stations(Vec::new(), e),
                }
            }
        }
    }

    fn on_media_key(&mut self, cmd: MediaCmd) {
        match cmd {
            MediaCmd::Play => match self.state {
                PlayState::Paused => self.player.play(),
                PlayState::Stopped => self.toggle_play(),
                _ => {}
            },
            MediaCmd::Pause => {
                if matches!(self.state, PlayState::Playing | PlayState::Buffering) {
                    self.toggle_play();
                }
            }
            MediaCmd::Toggle => self.toggle_play(),
            MediaCmd::Next => self.next(false),
            MediaCmd::Previous => self.previous(),
            MediaCmd::Stop => self.player.stop(),
            MediaCmd::SeekTo(secs) => {
                if matches!(self.current_item(), Some(Item::Track(_))) {
                    self.player.seek(secs);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Library
    // ------------------------------------------------------------------

    /// A full-size image (cover or artist photo). Only the last
    /// [`COVER_CACHE`] are kept, so memory doesn't grow with every artist played.
    fn cover(&mut self, path: &Option<PathBuf>) -> (Image, bool) {
        let Some(path) = path else { return (Image::default(), false) };
        if let Some(i) = self.cover_order.iter().position(|p| p == path) {
            let p = self.cover_order.remove(i).unwrap_or_else(|| path.clone());
            self.cover_order.push_back(p);
        } else {
            let img = decode_cover(path).map(Image::from_rgba8);
            self.covers.insert(path.clone(), img);
            self.cover_order.push_back(path.clone());
            while self.cover_order.len() > COVER_CACHE {
                if let Some(old) = self.cover_order.pop_front() {
                    self.covers.remove(&old);
                }
            }
        }
        match self.covers.get(path) {
            Some(Some(img)) => (img.clone(), true),
            _ => (Image::default(), false),
        }
    }

    /// An album cover for lists. Decoding big covers takes long, so ones not
    /// loaded yet are handed to the cover thread and show up when ready.
    /// A grid picture if it's decoded already. Which ones are decoded is
    /// decided by [`Self::sync_thumbs`] (what's on screen).
    fn thumb(&mut self, path: &Option<PathBuf>) -> (Image, bool) {
        match path.as_ref().and_then(|p| self.thumbs.get(p)) {
            Some(Some(img)) => (img.clone(), true),
            _ => (Image::default(), false),
        }
    }

    /// The pictures that should be in memory, most important first: the
    /// open album's cover, then the cards on screen (and a screen's margin)
    /// of the Albums or Artists grid.
    fn wanted_thumbs(&self) -> Vec<PathBuf> {
        let mut wanted: Vec<PathBuf> = Vec::new();
        if let Some(p) = self.open_album.as_ref().and_then(|a| a.cover.clone()) {
            wanted.push(p);
        }
        if let (0, Some((a, b))) = (self.page, self.album_range) {
            wanted.extend(self.albums.iter().take(b + 1).skip(a).filter_map(|al| al.cover.clone()));
        }
        if let (1, Some((a, b))) = (self.page, self.artist_range) {
            wanted.extend(self.artists.iter().take(b + 1).skip(a).filter_map(|ar| self.artist_picture(ar)));
        }
        wanted
    }

    /// Keeps only the wanted pictures in memory: drops the others (also from
    /// the grids) and starts decoding the missing ones.
    fn sync_thumbs(&mut self) {
        let wanted_list = self.wanted_thumbs();
        let wanted: HashSet<PathBuf> = wanted_list.iter().cloned().collect();
        let dropped: HashSet<PathBuf> = self.thumbs.keys().filter(|p| !wanted.contains(*p)).cloned().collect();
        if !dropped.is_empty() {
            for p in &dropped {
                self.thumbs.remove(p);
            }
            if let Some(ui) = self.ui() {
                let albums = ui.get_albums();
                for (i, a) in self.albums.iter().enumerate() {
                    if a.cover.as_ref().is_some_and(|p| dropped.contains(p)) {
                        if let Some(mut row) = albums.row_data(i).filter(|r| r.has_cover) {
                            row.cover = Image::default();
                            row.has_cover = false;
                            albums.set_row_data(i, row);
                        }
                    }
                }
                let artists = ui.get_artists();
                for (i, a) in self.artists.iter().enumerate() {
                    if self.artist_picture(a).is_some_and(|p| dropped.contains(&p)) {
                        if let Some(mut row) = artists.row_data(i).filter(|r| r.has_image) {
                            row.image = Image::default();
                            row.has_image = false;
                            artists.set_row_data(i, row);
                        }
                    }
                }
            }
        }
        // Decode what's missing (on screen first); forget queued pictures
        // that scrolled away before their turn.
        self.thumb_queue.retain(|p| wanted.contains(p));
        // (Pictures being decoded right now are dropped when they arrive.)
        self.thumb_queued.retain(|p| wanted.contains(p));
        for p in wanted_list {
            if !self.thumbs.contains_key(&p) && self.thumb_queued.insert(p.clone()) {
                self.thumb_queue.push(p);
            }
        }
        self.load_thumbs();
    }

    /// Albums `first..=last` are on screen in the Albums grid.
    pub fn set_album_range(&mut self, first: i32, last: i32) {
        // Only the Albums page shows this grid.
        self.page = 0;
        self.album_range = Self::with_margin(first, last);
        self.sync_thumbs();
    }

    /// Artists `first..=last` are on screen in the Artists grid.
    pub fn set_artist_range(&mut self, first: i32, last: i32) {
        // Only the Artists page shows this grid.
        if self.page != 1 {
            self.page_changed(1);
        }
        self.artist_range = Self::with_margin(first, last);
        self.sync_thumbs();
    }

    /// A visible range plus one screenful above and below it.
    fn with_margin(first: i32, last: i32) -> Option<(usize, usize)> {
        if last < first || last < 0 {
            return None;
        }
        let (first, last) = (first.max(0) as usize, last as usize);
        let span = last - first + 1;
        Some((first.saturating_sub(span), last + span))
    }

    /// Starts the cover thread for the covers waiting, if it isn't running.
    fn load_thumbs(&mut self) {
        if self.thumbs_loading || self.thumb_queue.is_empty() {
            return;
        }
        self.thumbs_loading = true;
        let paths = std::mem::take(&mut self.thumb_queue);
        let (tx, dir) = (self.bg_tx.clone(), self.paths.thumbs.clone());
        thread::spawn(move || {
            // Small chunks, so the cards on screen fill in quickly.
            for chunk in paths.chunks(6) {
                let done: Vec<_> = chunk.iter().map(|p| (p.clone(), decode_thumb(p, &dir))).collect();
                if tx.send(Bg::Thumbs(done)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Bg::ThumbsDone);
        });
    }

    /// Puts covers from the cover thread into the album grid and album page.
    fn on_thumbs(&mut self, done: Vec<(PathBuf, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>) {
        let mut ready: HashSet<PathBuf> = HashSet::new();
        // Pictures that scrolled away (or whose page was left) meanwhile are dropped.
        let wanted: HashSet<PathBuf> = self.wanted_thumbs().into_iter().collect();
        for (path, pixels) in done {
            self.thumb_queued.remove(&path);
            if !wanted.contains(&path) {
                continue;
            }
            let img = pixels.map(Image::from_rgba8);
            if img.is_some() {
                ready.insert(path.clone());
            }
            self.thumbs.insert(path, img);
        }
        if ready.is_empty() {
            return;
        }
        let Some(ui) = self.ui() else { return };
        let model = ui.get_albums();
        for (i, a) in self.albums.iter().enumerate() {
            let Some(path) = a.cover.as_ref().filter(|p| ready.contains(*p)) else { continue };
            if let (Some(mut row), Some(Some(img))) = (model.row_data(i), self.thumbs.get(path)) {
                row.cover = img.clone();
                row.has_cover = true;
                model.set_row_data(i, row);
            }
        }
        let artists = ui.get_artists();
        for i in 0..if self.artist_pictures { self.artists.len() } else { 0 } {
            let Some(path) = self.artist_picture(&self.artists[i]).filter(|p| ready.contains(p)) else { continue };
            if let (Some(mut row), Some(Some(img))) = (artists.row_data(i), self.thumbs.get(&path)) {
                row.image = img.clone();
                row.has_image = true;
                artists.set_row_data(i, row);
            }
        }
        if let Some(path) = self.open_album.as_ref().and_then(|a| a.cover.clone()).filter(|p| ready.contains(p)) {
            if let Some(Some(img)) = self.thumbs.get(&path) {
                let mut item = ui.get_album();
                item.cover = img.clone();
                item.has_cover = true;
                ui.set_album(item);
            }
        }
    }

    fn album_item(&mut self, a: &Album) -> AlbumItem {
        let (cover, has_cover) = self.thumb(&a.cover);
        AlbumItem {
            key: album_key(&a.artist, &a.title).into(),
            title: a.title.as_str().into(),
            artist: a.artist.as_str().into(),
            year: if a.year > 0 { a.year.to_string().into() } else { SharedString::new() },
            cover,
            has_cover,
        }
    }

    fn track_item(t: &Track, number: String) -> TrackItem {
        TrackItem {
            key: track_key(t).into(),
            number: number.into(),
            title: t.title.as_str().into(),
            artist: t.artist.as_str().into(),
            album: t.album.as_str().into(),
            duration: if t.duration_ms > 0 { format_time(t.duration_secs()).into() } else { SharedString::new() },
        }
    }

    fn reload_library(&mut self) {
        self.albums = self.library.albums(self.artist_filter.as_deref()).unwrap_or_default();
        if self.albums.is_empty() && self.artist_filter.is_some() {
            self.artist_filter = None;
            self.albums = self.library.albums(None).unwrap_or_default();
        }
        self.artists = self.library.artists().unwrap_or_default();
        self.songs = self.library.tracks(Some(&self.query)).unwrap_or_default();
        let count = self.library.track_count().unwrap_or(0);
        self.sort_albums();
        self.sort_artists();
        self.sort_songs();

        let albums = self.albums.clone();
        let album_items: Vec<AlbumItem> = albums.iter().map(|a| self.album_item(a)).collect();
        let artist_items = self.artist_items();
        let song_items: Vec<TrackItem> =
            self.songs.iter().enumerate().map(|(i, t)| Self::track_item(t, (i + 1).to_string())).collect();

        if let Some(ui) = self.ui() {
            ui.set_albums(model(album_items));
            ui.set_artists(model(artist_items));
            ui.set_songs(model(song_items));
            ui.set_library_empty(count == 0);
            ui.set_albums_filtered(self.artist_filter.is_some());
            ui.set_albums_title(self.artist_filter.clone().unwrap_or_else(|| "Albums".into()).into());
        }
        self.sync_thumbs();
        self.find_artist_photos();
    }

    pub fn start_scan(&mut self) {
        if self.scanning {
            self.rescan_pending = true;
            return;
        }
        let folders = self.library.folders().unwrap_or_default();
        if folders.is_empty() {
            if let Some(ui) = self.ui() {
                ui.set_library_status("No music folders yet".into());
            }
            return;
        }
        self.scanning = true;
        if let Some(ui) = self.ui() {
            ui.set_scanning(true);
            ui.set_library_status("Scanning…".into());
        }
        let db = self.paths.database.clone();
        let covers = self.paths.covers.clone();
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = Library::open(&db)
                .and_then(|lib| {
                    scanner::scan(&lib, &folders, &covers, move |n| {
                        let _ = progress_tx.send(Bg::ScanProgress(n));
                    })
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::ScanDone(result));
        });
    }

    fn refresh_folders(&self) {
        let folders: Vec<SharedString> = self
            .library
            .folders()
            .unwrap_or_default()
            .iter()
            .map(|f| f.display().to_string().into())
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_folders(model(folders));
        }
    }

    pub fn remove_folder(&mut self, index: usize) {
        let folders = self.library.folders().unwrap_or_default();
        let Some(folder) = folders.get(index) else { return };
        match self.library.remove_folder(folder) {
            Ok(()) => {
                self.show_toast(&format!("Removed {}", folder.display()));
                self.refresh_folders();
                self.reload_library();
            }
            Err(e) => self.show_toast(&format!("Could not remove folder: {e}")),
        }
    }

    /// Turns the macOS frosted-glass background on or off. Turning it on is a
    /// 10-second trial that reverts unless confirmed (like display settings),
    /// so a broken blur can never leave the window unusable.
    pub fn set_glass(&mut self, on: bool) {
        if on && !self.settings.glass {
            self.glass_trial = Some(std::time::Instant::now());
            self.tick_glass_trial();
        } else if !on {
            self.glass_trial = None;
            self.settings.glass = false;
            self.save_settings();
            if let Some(ui) = self.ui() {
                ui.set_glass_countdown(0);
            }
        }
        self.apply_glass();
    }

    /// "Keep" during the trial.
    pub fn keep_glass(&mut self) {
        if self.glass_trial.take().is_some() {
            self.settings.glass = true;
            self.save_settings();
            if let Some(ui) = self.ui() {
                ui.set_glass_countdown(0);
            }
            log("ui: frosted glass kept");
        }
    }

    /// Updates the trial countdown; reverts when time is up.
    fn tick_glass_trial(&mut self) {
        let Some(started) = self.glass_trial else { return };
        let left = 10 - started.elapsed().as_secs() as i32;
        if left <= 0 {
            self.glass_trial = None;
            log("ui: frosted glass not confirmed, reverted");
            if let Some(ui) = self.ui() {
                ui.set_glass_countdown(0);
            }
            self.apply_glass();
            self.show_toast("Frosted glass turned off again (not confirmed)");
        } else if let Some(ui) = self.ui() {
            if ui.get_glass_countdown() != left {
                ui.set_glass_countdown(left);
            }
        }
    }

    pub fn apply_glass(&self) {
        let on = (self.settings.glass || self.glass_trial.is_some()) && crate::glass::AVAILABLE;
        if let Some(ui) = self.ui() {
            ui.global::<Theme>().set_glass(on);
            ui.set_glass(on);
            crate::glass::apply(&ui, on);
        }
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.settings.opacity = opacity.clamp(0.5, 1.0);
        self.save_settings();
    }

    /// Downloads covers for albums without art, in the background.
    pub fn start_art_fetch(&mut self) {
        if !self.settings.download_art || self.art_running.is_some() {
            return;
        }
        let running = Arc::new(AtomicBool::new(true));
        self.art_running = Some(running.clone());
        let db = self.paths.database.clone();
        let covers = self.paths.covers.clone();
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let mut found = 0;
            let result = (|| -> anyhow::Result<()> {
                let lib = Library::open(&db)?;
                let missing = lib.albums_missing_art()?;
                if !missing.is_empty() {
                    log(format!("art: looking up {} albums online", missing.len()));
                }
                let mut fetcher = ArtFetcher::new();
                let mut failures = 0;
                for (artist, album) in missing {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    match fetcher.find_cover(&artist, &album) {
                        Ok(Some(data)) => {
                            failures = 0;
                            let key = scanner::album_key(&artist, &album);
                            if let Some(path) = scanner::save_cover(&data, &covers, &key) {
                                lib.set_album_cover(&artist, &album, &path)?;
                                lib.record_art_lookup(&artist, &album, true)?;
                                found += 1;
                                let _ = tx.send(Bg::ArtFound);
                            }
                        }
                        Ok(None) => {
                            failures = 0;
                            lib.record_art_lookup(&artist, &album, false)?;
                        }
                        Err(e) => {
                            // Probably offline: try again on the next scan.
                            log(format!("art: lookup failed for {artist} / {album}: {e:#}"));
                            failures += 1;
                            if failures >= 3 {
                                break;
                            }
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                log(format!("art: {e:#}"));
            }
            let _ = tx.send(Bg::ArtDone(found));
        });
    }

    fn show_backdrop(&mut self, path: Option<PathBuf>) {
        let (image, has) = self.cover(&path);
        if let Some(ui) = self.ui() {
            ui.set_np_backdrop(image);
            ui.set_np_has_backdrop(has);
        }
    }

    /// Shows the artist's photo behind Now Playing: an `artist.jpg` kept
    /// with the music if there is one, else one downloaded once.
    fn update_backdrop(&mut self, artist: &str, song: Option<&Path>) {
        if let Some(local) = song.and_then(scanner::local_artist_image) {
            return self.show_backdrop(Some(local));
        }
        match self.library.artist_image(artist) {
            Ok(Some(Some(path))) => {
                // Photos from before 0.5.17 sit in the covers folder under a
                // hashed name: move them to the artists folder.
                let path = match scanner::move_artist_photo(&path, &self.paths.artists, artist) {
                    Some(moved) => {
                        let _ = self.library.set_artist_image(artist, Some(&moved));
                        moved
                    }
                    None => path,
                };
                return self.show_backdrop(Some(path));
            }
            Ok(Some(None)) => return self.show_backdrop(None),
            Ok(None) => self.show_backdrop(None),
            Err(_) => return self.show_backdrop(None),
        }
        if !self.settings.download_art || artist == "Unknown Artist" || !self.artist_image_pending.insert(artist.to_string()) {
            return;
        }
        let (artist, db, artists, tx) =
            (artist.to_string(), self.paths.database.clone(), self.paths.artists.clone(), self.bg_tx.clone());
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<Option<PathBuf>> {
                let data = ArtFetcher::new().find_artist_image(&artist)?;
                let path = data.and_then(|d| scanner::save_artist_photo(&d, &artists, &artist));
                Library::open(&db)?.set_artist_image(&artist, path.as_deref())?;
                Ok(path)
            })();
            let path = result.unwrap_or_else(|e| {
                log(format!("art: artist photo for {artist} failed: {e:#}"));
                None
            });
            let _ = tx.send(Bg::ArtistImage { artist, path });
        });
    }

    pub fn set_download_art(&mut self, enabled: bool) {
        self.settings.download_art = enabled;
        self.save_settings();
        if enabled {
            self.start_art_fetch();
        } else if let Some(running) = self.art_running.take() {
            running.store(false, Ordering::Relaxed);
        }
    }

    pub fn add_folder(&mut self, folder: PathBuf) {
        match self.library.add_folder(&folder) {
            Ok(()) => {
                self.refresh_folders();
                self.show_toast(&format!("Added {}", folder.display()));
                self.start_scan();
            }
            Err(e) => self.show_toast(&format!("Could not add folder: {e}")),
        }
    }

    pub fn search_library(&mut self, query: &str) {
        self.query = query.trim().to_string();
        self.songs = self.library.tracks(Some(&self.query)).unwrap_or_default();
        self.sort_songs();
        self.show_songs();
        if let Some(ui) = self.ui() {
            ui.set_library_query(self.query.as_str().into());
        }
        // Also search the connected music servers; their songs are added
        // below the local ones when they arrive.
        let request = self.search_request.fetch_add(1, Ordering::SeqCst) + 1;
        if self.query.chars().count() < 2 || self.settings.servers.is_empty() {
            return;
        }
        let (servers, query, tx, current) =
            (self.settings.servers.clone(), self.query.clone(), self.bg_tx.clone(), self.search_request.clone());
        thread::spawn(move || {
            let mut tracks = Vec::new();
            for cfg in &servers {
                if current.load(Ordering::SeqCst) != request {
                    return;
                }
                match servers::search_tracks(cfg, &query) {
                    Ok(found) => tracks.extend(found),
                    Err(e) => log(format!("servers: searching {} failed: {e:#}", cfg.name)),
                }
            }
            let _ = tx.send(Bg::ServerSearch { request, tracks });
        });
    }

    fn sort_albums(&mut self) {
        let s = &self.settings;
        unitn_core::sort::sort_albums(&mut self.albums, s.album_sort, s.album_sort_reversed, s.sort_ignore_articles);
    }

    fn sort_artists(&mut self) {
        let s = &self.settings;
        unitn_core::sort::sort_artists(&mut self.artists, s.artist_sort, s.artist_sort_reversed, s.sort_ignore_articles);
    }

    fn sort_songs(&mut self) {
        // Dates added are only needed for "Recently added".
        let added = if self.settings.song_sort == 6 { self.library.added_times().unwrap_or_default() } else { HashMap::new() };
        let s = &self.settings;
        unitn_core::sort::sort_songs(&mut self.songs, s.song_sort, s.song_sort_reversed, &added, s.sort_ignore_articles);
    }

    /// The picture for an artist: their photo, else one of their album covers.
    fn artist_picture(&self, a: &Artist) -> Option<PathBuf> {
        self.artist_photos.get(&a.name).cloned().or_else(|| a.cover.clone())
    }

    fn artist_item(&mut self, a: &Artist) -> ArtistItem {
        let (image, has_image) = self.thumb(&self.artist_picture(a));
        ArtistItem {
            name: a.name.as_str().into(),
            info: format!(
                "{} • {}",
                plural(a.album_count as usize, "album", "albums"),
                plural(a.track_count as usize, "song", "songs")
            )
            .into(),
            initial: initial(&a.name),
            image,
            has_image,
        }
    }

    fn artist_items(&mut self) -> Vec<ArtistItem> {
        let artists = self.artists.clone();
        artists.iter().map(|a| self.artist_item(a)).collect()
    }

    /// Updates one artist on the Artists page (after their photo arrived).
    fn refresh_artist(&mut self, name: &str) {
        let Some(i) = self.artists.iter().position(|a| a.name == name) else { return };
        let artist = self.artists[i].clone();
        let item = self.artist_item(&artist);
        if let Some(ui) = self.ui() {
            let model = ui.get_artists();
            if i < model.row_count() {
                model.set_row_data(i, item);
            }
        }
    }

    /// Finds photos for the Artists page in the background: an artist.jpg in
    /// the music folders, else one downloaded earlier; then, if album art
    /// downloads are on, downloads the missing ones slowly (the photo
    /// services ask for no more than about one request a second).
    /// Only the shown grid's pictures are kept; leaving a grid frees them.
    pub fn page_changed(&mut self, page: i32) {
        self.page = page;
        if page == PAGE_STATS {
            self.refresh_stats();
        }
        if page != 0 {
            self.album_range = None;
        }
        if page != 1 {
            self.artist_range = None;
        }
        self.artist_pictures = page == 1;
        if page == 1 {
            self.find_artist_photos();
        }
        self.sync_thumbs();
    }

    fn find_artist_photos(&mut self) {
        if !self.artist_pictures || self.artist_photos_running || self.artists.is_empty() {
            return;
        }
        self.artist_photos_running = true;
        let artists: Vec<(String, Option<PathBuf>)> =
            self.artists.iter().filter(|a| !self.artist_photos.contains_key(&a.name)).map(|a| (a.name.clone(), a.sample.clone())).collect();
        let pending = self.artist_image_pending.clone();
        let (db, dir, tx, download) =
            (self.paths.database.clone(), self.paths.artists.clone(), self.bg_tx.clone(), self.settings.download_art);
        thread::spawn(move || {
            let Ok(library) = Library::open(&db) else {
                let _ = tx.send(Bg::ArtistPhotosDone);
                return;
            };
            let mut found = Vec::new();
            let mut missing = Vec::new();
            for (name, sample) in artists {
                if let Some(photo) = sample.as_deref().and_then(scanner::local_artist_image) {
                    found.push((name, photo));
                    continue;
                }
                match library.artist_image(&name) {
                    Ok(Some(Some(photo))) if photo.is_file() => found.push((name, photo)),
                    Ok(Some(_)) => {} // looked up before: there is none
                    _ => missing.push(name),
                }
            }
            if tx.send(Bg::ArtistPhotos(found)).is_err() {
                return;
            }
            if download {
                let skip = ["unknown artist", "various artists", "various", "va"];
                for name in missing.into_iter().filter(|n| !skip.contains(&n.to_lowercase().as_str()) && !pending.contains(n)) {
                    std::thread::sleep(Duration::from_millis(1500));
                    let photo = (|| -> anyhow::Result<Option<PathBuf>> {
                        let data = ArtFetcher::new().find_artist_image(&name)?;
                        let path = data.and_then(|d| scanner::save_artist_photo(&d, &dir, &name));
                        library.set_artist_image(&name, path.as_deref())?;
                        Ok(path)
                    })();
                    match photo {
                        Ok(Some(path)) => {
                            if tx.send(Bg::ArtistPhotos(vec![(name, path)])).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => log(format!("art: artist photo for {name} failed: {e:#}")),
                    }
                }
            }
            let _ = tx.send(Bg::ArtistPhotosDone);
        });
    }

    fn on_artist_photos(&mut self, photos: Vec<(String, PathBuf)>) {
        let names: Vec<String> = photos.iter().map(|(n, _)| n.clone()).collect();
        self.artist_photos.extend(photos);
        for name in &names {
            self.refresh_artist(name);
        }
        self.sync_thumbs();
    }

    /// Shows the sort menus' labels and the chosen orders.
    fn show_sort(&self) {
        use unitn_core::sort::{Order, ALBUM_ORDERS, ARTIST_ORDERS, SONG_ORDERS};
        let Some(ui) = self.ui() else { return };
        let labels = |orders: &[Order]| model(orders.iter().map(|o| SharedString::from(o.label)).collect::<Vec<_>>());
        let desc = |orders: &[Order], i: usize, reversed: bool| orders.get(i).map(|o| o.descending).unwrap_or(false) != reversed;
        let s = &self.settings;
        ui.set_album_sort_labels(labels(ALBUM_ORDERS));
        ui.set_artist_sort_labels(labels(ARTIST_ORDERS));
        ui.set_song_sort_labels(labels(SONG_ORDERS));
        ui.set_album_sort(s.album_sort as i32);
        ui.set_artist_sort(s.artist_sort as i32);
        ui.set_song_sort(s.song_sort as i32);
        ui.set_album_sort_desc(desc(ALBUM_ORDERS, s.album_sort, s.album_sort_reversed));
        ui.set_artist_sort_desc(desc(ARTIST_ORDERS, s.artist_sort, s.artist_sort_reversed));
        ui.set_song_sort_desc(desc(SONG_ORDERS, s.song_sort, s.song_sort_reversed));
    }

    /// Picks the sort order of a page (0 Albums, 1 Artists, 2 Songs), in its
    /// natural direction.
    pub fn set_sort(&mut self, page: i32, index: usize) {
        use unitn_core::sort::{ALBUM_ORDERS, ARTIST_ORDERS, SONG_ORDERS};
        let s = &mut self.settings;
        match page {
            0 if index < ALBUM_ORDERS.len() => (s.album_sort, s.album_sort_reversed) = (index, false),
            1 if index < ARTIST_ORDERS.len() => (s.artist_sort, s.artist_sort_reversed) = (index, false),
            2 if index < SONG_ORDERS.len() => (s.song_sort, s.song_sort_reversed) = (index, false),
            _ => return,
        }
        self.apply_sort(page);
    }

    /// Flips the direction of a page's sort order.
    pub fn flip_sort(&mut self, page: i32) {
        let s = &mut self.settings;
        match page {
            0 => s.album_sort_reversed = !s.album_sort_reversed,
            1 => s.artist_sort_reversed = !s.artist_sort_reversed,
            2 => s.song_sort_reversed = !s.song_sort_reversed,
            _ => return,
        }
        self.apply_sort(page);
    }

    /// Whether sorting skips a leading "The", "A" or "An"; re-sorts all pages.
    pub fn set_sort_ignore_articles(&mut self, on: bool) {
        self.settings.sort_ignore_articles = on;
        for page in 0..3 {
            self.apply_sort(page);
        }
    }

    fn apply_sort(&mut self, page: i32) {
        self.save_settings();
        self.show_sort();
        match page {
            0 => {
                self.sort_albums();
                let albums = self.albums.clone();
                let items: Vec<AlbumItem> = albums.iter().map(|a| self.album_item(a)).collect();
                if let Some(ui) = self.ui() {
                    ui.set_albums(model(items));
                }
                self.sync_thumbs();
            }
            1 => {
                self.sort_artists();
                let items = self.artist_items();
                if let Some(ui) = self.ui() {
                    ui.set_artists(model(items));
                }
                self.sync_thumbs();
            }
            _ => {
                self.sort_songs();
                self.show_songs();
            }
        }
    }

    fn show_songs(&mut self) {
        let items: Vec<TrackItem> =
            self.songs.iter().enumerate().map(|(i, t)| Self::track_item(t, (i + 1).to_string())).collect();
        if let Some(ui) = self.ui() {
            ui.set_songs(model(items));
        }
    }

    fn on_server_search(&mut self, request: u64, tracks: Vec<Track>) {
        if request != self.search_request.load(Ordering::SeqCst) || tracks.is_empty() {
            return;
        }
        self.songs = self.library.tracks(Some(&self.query)).unwrap_or_default();
        self.songs.extend(tracks);
        self.sort_songs();
        self.show_songs();
    }

    pub fn open_album(&mut self, index: usize) {
        let Some(album) = self.albums.get(index).cloned() else { return };
        self.show_album(album);
    }

    fn show_album(&mut self, album: Album) {
        let tracks = self.library.album_tracks(&album.artist, &album.title).unwrap_or_default();
        self.set_album_from_server(false);
        self.show_album_tracks(album, tracks);
    }

    fn set_album_from_server(&mut self, remote: bool) {
        self.album_from_server = remote;
        if let Some(ui) = self.ui() {
            ui.set_album_from_server(remote);
        }
    }

    fn show_album_tracks(&mut self, album: Album, tracks: Vec<Track>) {
        self.open_album = Some(album.clone());
        self.album_tracks = tracks;
        let item = self.album_item(&album);
        let total: u64 = self.album_tracks.iter().map(|t| t.duration_ms).sum();
        let info = format!("{} • {}", plural(self.album_tracks.len(), "song", "songs"), long_duration(total));
        let genre = unitn_core::model::album_genre(&self.album_tracks);
        let tracks: Vec<TrackItem> = self
            .album_tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let n = if t.track_no > 0 { t.track_no as usize } else { i + 1 };
                Self::track_item(t, n.to_string())
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_album(item);
            ui.set_album_genre(genre.into());
            ui.set_album_info(info.into());
            ui.set_album_tracks(model(tracks));
        }
        self.sync_thumbs();
    }

    pub fn open_artist(&mut self, index: usize) {
        let Some(artist) = self.artists.get(index) else { return };
        self.artist_filter = Some(artist.name.clone());
        self.reload_library();
    }

    pub fn show_all_albums(&mut self) {
        self.artist_filter = None;
        self.reload_library();
    }

    // ------------------------------------------------------------------
    // Playlists
    // ------------------------------------------------------------------

    fn refresh_playlists(&mut self) {
        self.playlists = self.library.playlists().unwrap_or_default();
        let items: Vec<PlaylistItem> = self
            .playlists
            .iter()
            .map(|p| PlaylistItem {
                name: p.name.as_str().into(),
                info: plural(p.track_count as usize, "song", "songs").into(),
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_playlists(model(items));
        }
        self.update_liked();
    }

    // ------------------------------------------------------------------
    // Likes: the heart on Now Playing adds the song to the "Likes"
    // playlist (made on the first like) and takes it out again.
    // ------------------------------------------------------------------

    /// The playing song, if it is one that can be liked (not a radio
    /// station or a podcast episode).
    fn likeable_track(&self) -> Option<Track> {
        match self.current_item()? {
            Item::Track(t) if !t.path.to_string_lossy().starts_with("podcast://") => Some(t.clone()),
            _ => None,
        }
    }

    fn likes_playlist_id(&self) -> Option<i64> {
        let exact = self.playlists.iter().find(|p| p.name == LIKES_PLAYLIST);
        exact.or_else(|| self.playlists.iter().find(|p| p.name.eq_ignore_ascii_case(LIKES_PLAYLIST))).map(|p| p.id)
    }

    /// Shows whether the playing song is liked.
    fn update_liked(&self) {
        let track = self.likeable_track();
        let liked = match (&track, self.likes_playlist_id()) {
            (Some(t), Some(id)) => self.library.playlist_paths(id).map(|paths| paths.contains(&t.path)).unwrap_or(false),
            _ => false,
        };
        if let Some(ui) = self.ui() {
            ui.set_np_can_like(track.is_some());
            ui.set_np_liked(liked);
        }
    }

    /// Likes the playing song, or takes the like back.
    pub fn toggle_like(&mut self) {
        let Some(track) = self.likeable_track() else { return };
        let result = match self.likes_playlist_id() {
            Some(id) => {
                let paths = self.library.playlist_paths(id).unwrap_or_default();
                if paths.contains(&track.path) {
                    let kept: Vec<PathBuf> = paths.into_iter().filter(|p| *p != track.path).collect();
                    self.library.set_playlist_paths(id, &kept).map(|_| (id, false))
                } else {
                    self.remember_server_songs(std::slice::from_ref(&track));
                    self.library.add_to_playlist(id, std::slice::from_ref(&track.path)).map(|_| (id, true))
                }
            }
            None => {
                self.remember_server_songs(std::slice::from_ref(&track));
                self.library
                    .create_playlist(LIKES_PLAYLIST)
                    .and_then(|id| self.library.add_to_playlist(id, std::slice::from_ref(&track.path)).map(|_| (id, true)))
            }
        };
        match result {
            Ok((id, liked)) => {
                log(format!("likes: {} {}", if liked { "added" } else { "removed" }, track.path.display()));
                self.show_toast(&if liked {
                    format!("Added \"{}\" to {LIKES_PLAYLIST}", track.title)
                } else {
                    format!("Removed \"{}\" from {LIKES_PLAYLIST}", track.title)
                });
                let open = self.current_playlist_id() == Some(id);
                self.refresh_playlists();
                if open {
                    self.open_playlist_id(id);
                }
            }
            Err(e) => {
                log(format!("likes: could not save: {e:#}"));
                self.show_toast(&format!("Could not save the like: {e}"));
            }
        }
    }

    fn current_playlist_id(&self) -> Option<i64> {
        self.current_playlist.and_then(|i| self.playlists.get(i)).map(|p| p.id)
    }

    /// Picks "New Playlist", "New Playlist 2", … whichever is free.
    fn unique_playlist_name(&self, base: &str) -> String {
        let taken = |n: &str| self.playlists.iter().any(|p| p.name.eq_ignore_ascii_case(n));
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
    }

    fn open_playlist_id(&mut self, id: i64) {
        self.refresh_playlists();
        if let Some(i) = self.playlists.iter().position(|p| p.id == id) {
            self.open_playlist(i);
        }
    }

    /// Saves what's needed to show and play server songs from a playlist
    /// later (no credentials: those come from the server settings).
    fn remember_server_songs(&self, tracks: &[Track]) {
        let remote: Vec<(Track, String)> = tracks
            .iter()
            .filter(|t| t.is_remote())
            .filter_map(|t| {
                // Podcast episodes: the audio address is public, keep it as is.
                if is_episode(t) {
                    return Some((t.clone(), t.stream_url.clone()?));
                }
                let cfg = self.settings.servers.iter().find(|c| servers::owns_track(c, &t.path))?;
                let stream_ref = servers::stream_ref(cfg, t.stream_url.as_deref()?)?;
                Some((t.clone(), stream_ref))
            })
            .collect();
        if !remote.is_empty() {
            if let Err(e) = self.library.remember_remote_tracks(&remote) {
                log(format!("playlists: could not save server songs: {e:#}"));
            }
        }
    }

    /// Gives playlist songs from music servers their stream address.
    fn resolve_server_songs(&self, tracks: &mut [Track]) {
        for t in tracks.iter_mut().filter(|t| t.stream_url.is_none()) {
            if is_episode(t) {
                t.stream_url = self.library.remote_stream_ref(&t.path).ok().flatten();
                continue;
            }
            let Some(cfg) = self.settings.servers.iter().find(|c| servers::owns_track(c, &t.path)) else { continue };
            if let Ok(Some(stream_ref)) = self.library.remote_stream_ref(&t.path) {
                t.stream_url = Some(servers::stream_url(cfg, &stream_ref));
            }
        }
    }

    pub fn open_playlist(&mut self, index: usize) {
        let Some(pl) = self.playlists.get(index).cloned() else { return };
        self.current_playlist = Some(index);
        let mut tracks = self.library.playlist_tracks(pl.id).unwrap_or_default();
        self.resolve_server_songs(&mut tracks);
        self.playlist_tracks = tracks;
        let total: u64 = self.playlist_tracks.iter().map(|t| t.duration_ms).sum();
        let info = format!("{} • {}", plural(self.playlist_tracks.len(), "song", "songs"), long_duration(total));
        let items: Vec<TrackItem> = self
            .playlist_tracks
            .iter()
            .enumerate()
            .map(|(i, t)| Self::track_item(t, (i + 1).to_string()))
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_current_playlist(index as i32);
            ui.set_playlist_name(pl.name.as_str().into());
            ui.set_playlist_info(info.into());
            ui.set_playlist_tracks(model(items));
        }
    }

    /// Moves a song of the open playlist to another position and saves it.
    pub fn move_playlist_track(&mut self, from: usize, to: usize) {
        let Some(id) = self.current_playlist_id() else { return };
        if from == to || from >= self.playlist_tracks.len() || to >= self.playlist_tracks.len() {
            return;
        }
        if let Err(e) = self.library.move_in_playlist(id, from, to) {
            self.show_toast(&format!("Could not move the song: {e}"));
        }
        self.open_playlist_id(id);
    }

    fn create_playlist_with(&mut self, base: &str, paths: &[PathBuf]) -> Option<i64> {
        let name = self.unique_playlist_name(base);
        match self.library.create_playlist(&name).and_then(|id| self.library.add_to_playlist(id, paths).map(|_| id)) {
            Ok(id) => {
                self.refresh_playlists();
                Some(id)
            }
            Err(e) => {
                self.show_toast(&format!("Could not create the playlist: {e}"));
                None
            }
        }
    }

    pub fn new_playlist(&mut self) {
        if let Some(id) = self.create_playlist_with("New Playlist", &[]) {
            self.open_playlist_id(id);
        }
    }

    pub fn rename_playlist(&mut self, name: &str) {
        let (Some(id), name) = (self.current_playlist_id(), name.trim()) else { return };
        if name.is_empty() {
            return;
        }
        if let Err(e) = self.library.rename_playlist(id, name) {
            self.show_toast(&format!("Could not rename: {e}"));
        }
        self.open_playlist_id(id);
    }

    pub fn delete_playlist(&mut self) {
        let Some(id) = self.current_playlist_id() else { return };
        let name = self.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
        if let Err(e) = self.library.delete_playlist(id) {
            self.show_toast(&format!("Could not delete: {e}"));
            return;
        }
        self.current_playlist = None;
        self.refresh_playlists();
        if let Some(ui) = self.ui() {
            ui.set_current_playlist(-1);
            ui.set_page(0);
        }
        self.show_toast(&format!("Deleted \"{name}\""));
    }

    pub fn export_playlist(&mut self, path: PathBuf) {
        match m3u::write(&path, &self.playlist_tracks) {
            Ok(()) => self.show_toast(&format!("Exported to {}", path.display())),
            Err(e) => self.show_toast(&format!("Export failed: {e}")),
        }
    }

    pub fn import_playlist(&mut self, path: PathBuf) {
        let paths: Vec<PathBuf> = match m3u::read(&path) {
            Ok(p) => p.into_iter().filter(|p| p.is_file()).collect(),
            Err(e) => {
                self.show_toast(&format!("Could not read the playlist: {e}"));
                return;
            }
        };
        if paths.is_empty() {
            self.show_toast("No playable files found in that playlist");
            return;
        }
        let base = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into());
        if let Some(id) = self.create_playlist_with(&base, &paths) {
            self.open_playlist_id(id);
            if let Some(ui) = self.ui() {
                ui.set_page(8);
            }
            self.show_toast(&format!("Imported {}", plural(paths.len(), "song", "songs")));
        }
    }

    pub fn play_playlist(&mut self, shuffle: bool) {
        self.play_tracks(self.playlist_tracks.clone(), 0, shuffle);
    }

    pub fn play_playlist_track(&mut self, index: usize) {
        self.play_tracks(self.playlist_tracks.clone(), index, false);
    }

    pub fn save_queue_as_playlist(&mut self) {
        let tracks: Vec<Track> = self
            .queue
            .iter()
            .filter_map(|i| match i {
                Item::Track(t) => Some(t.clone()),
                Item::Station(_) => None,
            })
            .collect();
        self.remember_server_songs(&tracks);
        let paths: Vec<PathBuf> = tracks.into_iter().map(|t| t.path).collect();
        if paths.is_empty() {
            self.show_toast("The queue has no songs to save");
            return;
        }
        if let Some(id) = self.create_playlist_with("Saved Queue", &paths) {
            self.open_playlist_id(id);
            if let Some(ui) = self.ui() {
                ui.set_page(8);
            }
        }
    }

    // ------------------------------------------------------------------
    // Music servers (Jellyfin, Plex, OpenSubsonic)
    // ------------------------------------------------------------------

    fn refresh_servers(&mut self) {
        let items: Vec<ServerItem> = self
            .settings
            .servers
            .iter()
            .map(|s| ServerItem {
                name: s.name.as_str().into(),
                info: format!("{} • {}{}", s.kind.label(), s.url, if s.user.is_empty() { String::new() } else { format!(" • {}", s.user) }).into(),
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_servers(model(items));
            ui.set_current_server(self.server.index.map(|i| i as i32).unwrap_or(-1));
        }
    }

    pub fn connect_server(&mut self, kind: i32, url: String, user: String, secret: String) {
        if kind == 1 {
            self.sign_in_with_plex(url);
            return;
        }
        if url.trim().is_empty() {
            if let Some(ui) = self.ui() {
                ui.set_server_dialog_status("Enter the server's address.".into());
            }
            return;
        }
        let kind = if kind == 2 { ServerKind::Subsonic } else { ServerKind::Jellyfin };
        if let Some(ui) = self.ui() {
            ui.set_server_connecting(true);
            ui.set_server_dialog_status(format!("Connecting to {}…", kind.label()).into());
        }
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let result = servers::connect(kind, &url, &user, &secret).map_err(|e| {
                log(format!("servers: connecting to {} failed: {e:#}", kind.label()));
                format!("{e:#}")
            });
            let _ = tx.send(Bg::ServerConnected(result));
        });
    }

    /// Plex: sign in on plex.tv in the browser (works with two-factor
    /// sign-in), then connect with the token. The address is optional.
    fn sign_in_with_plex(&mut self, url: String) {
        let login = self.plex_login.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ui) = self.ui() {
            ui.set_server_connecting(true);
            ui.set_server_dialog_status("Opening the Plex sign-in page…".into());
        }
        let (tx, current) = (self.bg_tx.clone(), self.plex_login.clone());
        thread::spawn(move || {
            let alive = || current.load(Ordering::SeqCst) == login;
            let result = (|| -> anyhow::Result<Option<ServerConfig>> {
                let pin = servers::plex_pin_start()?;
                let opened = open_in_browser(&pin.auth_url);
                let _ = tx.send(Bg::PlexWaiting { login, code: pin.link_code.clone(), opened });
                let deadline = std::time::Instant::now() + Duration::from_secs(15 * 60);
                let token = loop {
                    thread::sleep(Duration::from_secs(2));
                    if !alive() {
                        return Ok(None);
                    }
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("The Plex sign-in timed out. Please try again.");
                    }
                    if let Some(token) = servers::plex_pin_token(&pin)? {
                        break token;
                    }
                };
                log("servers: Plex sign-in approved, connecting");
                let _ = tx.send(Bg::PlexApproved { login });
                Ok(Some(servers::plex_connect_account(&url, &token, pin.device_id)?))
            })();
            if !alive() {
                return;
            }
            let result = match result {
                Ok(Some(cfg)) => Ok(cfg),
                Ok(None) => return,
                Err(e) => {
                    log(format!("servers: Plex sign-in failed: {e:#}"));
                    Err(format!("{e:#}"))
                }
            };
            let _ = tx.send(Bg::ServerConnected(result));
        });
    }

    fn on_plex_waiting(&mut self, login: u64, code: Option<String>, opened: bool) {
        if login != self.plex_login.load(Ordering::SeqCst) {
            return;
        }
        let text = match (opened, code) {
            (true, Some(code)) => format!(
                "Finish signing in to Plex in your browser. If no page opened, go to plex.tv/link and enter the code {code}."
            ),
            (true, None) => "Finish signing in to Plex in your browser.".to_string(),
            (false, Some(code)) => format!("Go to plex.tv/link in your browser and enter the code {code}."),
            (false, None) => "Couldn't open your browser. Please try again.".to_string(),
        };
        if let Some(ui) = self.ui() {
            ui.set_server_dialog_status(text.into());
        }
    }

    /// The "Add server" dialog was closed: stop waiting for a Plex sign-in.
    pub fn close_server_dialog(&mut self) {
        self.plex_login.fetch_add(1, Ordering::SeqCst);
        if let Some(ui) = self.ui() {
            ui.set_server_dialog(false);
            ui.set_server_connecting(false);
            ui.set_server_dialog_status(SharedString::new());
        }
    }

    fn on_server_connected(&mut self, result: Result<ServerConfig, String>) {
        if let Some(ui) = self.ui() {
            ui.set_server_connecting(false);
        }
        match result {
            Ok(cfg) => {
                log(format!("servers: connected to {} ({})", cfg.name, cfg.kind.label()));
                let key = server_key(&cfg);
                let index = match self.settings.servers.iter().position(|s| server_key(s) == key) {
                    Some(i) => {
                        self.settings.servers[i] = cfg.clone();
                        i
                    }
                    None => {
                        self.settings.servers.push(cfg.clone());
                        self.settings.servers.len() - 1
                    }
                };
                self.server.cache.remove(&key);
                self.save_settings();
                self.refresh_servers();
                self.show_toast(&format!("Connected to {}", cfg.name));
                if let Some(ui) = self.ui() {
                    ui.set_server_dialog(false);
                    ui.set_server_dialog_status(SharedString::new());
                    ui.set_page(9);
                }
                self.open_server(index);
            }
            Err(e) => {
                if let Some(ui) = self.ui() {
                    ui.set_server_dialog_status(e.into());
                }
            }
        }
    }

    pub fn remove_server(&mut self, index: usize) {
        if index >= self.settings.servers.len() {
            return;
        }
        let removed = self.settings.servers.remove(index);
        self.server.cache.remove(&server_key(&removed));
        match self.server.index {
            Some(i) if i == index => {
                self.server.index = None;
                if let Some(ui) = self.ui() {
                    if ui.get_page() == 9 || (ui.get_page() == 5 && self.album_from_server) {
                        ui.set_page(0);
                    }
                }
            }
            Some(i) if i > index => self.server.index = Some(i - 1),
            _ => {}
        }
        self.save_settings();
        self.refresh_servers();
        self.show_toast(&format!("Removed {}", removed.name));
    }

    pub fn open_server(&mut self, index: usize) {
        let Some(cfg) = self.settings.servers.get(index).cloned() else { return };
        self.server.index = Some(index);
        if let Some(ui) = self.ui() {
            ui.set_server_name(cfg.name.as_str().into());
            ui.set_current_server(index as i32);
        }
        if self.server.cache.contains_key(&server_key(&cfg)) {
            self.show_server_albums();
        } else {
            self.load_server_albums(cfg);
        }
    }

    pub fn refresh_server(&mut self) {
        let Some(cfg) = self.server.index.and_then(|i| self.settings.servers.get(i)).cloned() else { return };
        self.server.cache.remove(&server_key(&cfg));
        self.load_server_albums(cfg);
    }

    fn load_server_albums(&mut self, cfg: ServerConfig) {
        let request = self.server_request.fetch_add(1, Ordering::Relaxed) + 1;
        self.server.shown.clear();
        self.server.model = None;
        if let Some(ui) = self.ui() {
            ui.set_server_albums(ModelRc::default());
            ui.set_server_loading(true);
            ui.set_server_status(format!("Loading albums from {}…", cfg.name).into());
        }
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let result = servers::albums(&cfg).map_err(|e| {
                log(format!("servers: albums from {} failed: {e:#}", cfg.name));
                format!("{e:#}")
            });
            let _ = tx.send(Bg::ServerAlbums { request, key: server_key(&cfg), result });
        });
    }

    /// Shows the albums of the open server and fetches missing covers.
    fn show_server_albums(&mut self) {
        let Some(cfg) = self.server.index.and_then(|i| self.settings.servers.get(i)).cloned() else { return };
        let key = server_key(&cfg);
        let all = self.server.cache.get(&key).cloned().unwrap_or_default();
        self.server.shown = all.clone();
        let covers_dir = self.paths.covers.join("servers");
        let mut missing = Vec::new();
        let items: Vec<AlbumItem> = self
            .server
            .shown
            .clone()
            .iter()
            .map(|a| {
                let cached = scanner::cached_cover(&covers_dir, &server_cover_key(&key, &a.id));
                if cached.is_none() {
                    if let Some(url) = &a.image_url {
                        missing.push((a.id.clone(), url.clone()));
                    }
                }
                let (cover, has_cover) = self.cover(&cached);
                AlbumItem {
                    key: album_key(&a.artist, &a.title).into(),
                    title: a.title.as_str().into(),
                    artist: a.artist.as_str().into(),
                    year: if a.year > 0 { a.year.to_string().into() } else { SharedString::new() },
                    cover,
                    has_cover,
                }
            })
            .collect();
        let model = Rc::new(VecModel::from(items));
        self.server.model = Some(model.clone());
        let status = plural(all.len(), "album", "albums");
        if let Some(ui) = self.ui() {
            ui.set_server_albums(ModelRc::from(model));
            ui.set_server_loading(false);
            ui.set_server_status(status.into());
        }
        if missing.is_empty() {
            return;
        }
        // Covers, one at a time; a newer request (refresh, other server) stops this.
        let request = self.server_request.load(Ordering::Relaxed);
        let (current, tx) = (self.server_request.clone(), self.bg_tx.clone());
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(&covers_dir);
            let mut failures = 0;
            for (id, url) in missing {
                if current.load(Ordering::Relaxed) != request || failures >= 5 {
                    return;
                }
                match servers::fetch_image(&url) {
                    Ok(data) => {
                        if let Some(path) = scanner::save_cover(&data, &covers_dir, &server_cover_key(&key, &id)) {
                            let _ = tx.send(Bg::ServerCover { request, id, path });
                        }
                    }
                    Err(_) => failures += 1,
                }
            }
        });
    }

    /// The album header for a server album (with its cached cover).
    fn server_album(&self, album: &ServerAlbum, tracks: &[Track]) -> Album {
        Album {
            title: album.title.clone(),
            artist: album.artist.clone(),
            year: album.year,
            track_count: tracks.len() as u32,
            duration_ms: tracks.iter().map(|t| t.duration_ms).sum(),
            cover: tracks.first().and_then(|t| t.cover.clone()).or_else(|| self.server_cover_of(album)),
            added: 0,
        }
    }

    fn server_cover_of(&self, album: &ServerAlbum) -> Option<PathBuf> {
        let cfg = self.server.index.and_then(|i| self.settings.servers.get(i))?;
        scanner::cached_cover(&self.paths.covers.join("servers"), &server_cover_key(&server_key(cfg), &album.id))
    }

    pub fn open_server_album(&mut self, index: usize) {
        let Some(album) = self.server.shown.get(index).cloned() else { return };
        let Some(cfg) = self.server.index.and_then(|i| self.settings.servers.get(i)).cloned() else { return };
        self.set_album_from_server(true);
        let header = self.server_album(&album, &[]);
        self.show_album_tracks(header, Vec::new());
        if let Some(ui) = self.ui() {
            ui.set_album_info("Loading songs…".into());
        }
        let request = self.server_request.load(Ordering::Relaxed);
        let cover = self.server_cover_of(&album);
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let result = servers::album_tracks(&cfg, &album)
                .map(|mut tracks| {
                    for t in &mut tracks {
                        t.cover = cover.clone();
                    }
                    tracks
                })
                .map_err(|e| {
                    log(format!("servers: songs of {} failed: {e:#}", album.title));
                    format!("{e:#}")
                });
            let _ = tx.send(Bg::ServerTracks { request, album, result });
        });
    }

    // ------------------------------------------------------------------
    // "Fix album info"
    // ------------------------------------------------------------------

    fn open_fix_dialog(&mut self) {
        if self.album_from_server {
            return;
        }
        let Some(album) = self.open_album.clone() else { return };
        self.fix = FixState { target: Some((album.artist.clone(), album.title.clone())), ..Default::default() };
        let (image, has) = self.cover(&album.cover);
        if let Some(ui) = self.ui() {
            ui.set_fix_artist(album.artist.as_str().into());
            ui.set_fix_album(album.title.as_str().into());
            ui.set_fix_selected(-1);
            ui.set_fix_busy(false);
            ui.set_fix_status(SharedString::new());
            ui.set_fix_preview(image);
            ui.set_fix_has_preview(has);
            ui.set_fix_visible(true);
        }
        self.push_fix_rows();
        let artist = if album.artist == "Unknown Artist" { String::new() } else { album.artist };
        self.fix_search(artist, album.title);
    }

    pub fn close_fix_dialog(&mut self) {
        self.fix_request.fetch_add(1, Ordering::Relaxed); // stop thumbnail downloads
        self.fix = FixState::default();
        if let Some(ui) = self.ui() {
            ui.set_fix_visible(false);
        }
    }

    fn push_fix_rows(&mut self) {
        let rows = self.fix.rows.clone();
        if let Some(ui) = self.ui() {
            ui.set_fix_candidates(model(rows));
        }
    }

    fn set_fix_preview(&mut self, path: Option<PathBuf>) {
        let (image, has) = self.cover(&path);
        if let Some(ui) = self.ui() {
            ui.set_fix_preview(image);
            ui.set_fix_has_preview(has);
        }
    }

    pub fn fix_search(&mut self, artist: String, album: String) {
        if artist.trim().is_empty() && album.trim().is_empty() {
            return;
        }
        let request = self.fix_request.fetch_add(1, Ordering::Relaxed) + 1;
        self.fix.candidates.clear();
        self.fix.rows.clear();
        self.fix.selected = None;
        self.push_fix_rows();
        if let Some(ui) = self.ui() {
            ui.set_fix_selected(-1);
            ui.set_fix_busy(true);
            ui.set_fix_status("Searching MusicBrainz…".into());
        }
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let result = ArtFetcher::new().search_albums(&artist, &album).map_err(|e| {
                log(format!("fix: search failed: {e:#}"));
                "Search failed — are you online?".to_string()
            });
            let _ = tx.send(Bg::FixResults { request, result });
        });
    }

    fn on_fix_results(&mut self, request: u64, result: Result<Vec<AlbumCandidate>, String>) {
        if request != self.fix_request.load(Ordering::Relaxed) || self.fix.target.is_none() {
            return;
        }
        let status = match &result {
            Ok(list) if list.is_empty() => "Nothing found. Try fewer or different words.".to_string(),
            Ok(list) => format!("{} — pick the right one, then Save.", plural(list.len(), "match", "matches")),
            Err(e) => e.clone(),
        };
        let list = result.unwrap_or_default();
        self.fix.rows = list
            .iter()
            .map(|c| {
                let mut info = vec![c.artist.clone()];
                if c.year > 0 {
                    info.push(c.year.to_string());
                }
                if !c.kind.is_empty() {
                    info.push(c.kind.clone());
                }
                FixCandidate {
                    title: c.title.as_str().into(),
                    info: info.join(" • ").into(),
                    thumb: Image::default(),
                    has_thumb: false,
                }
            })
            .collect();
        self.fix.candidates = list.clone();
        self.push_fix_rows();
        if let Some(ui) = self.ui() {
            ui.set_fix_busy(false);
            ui.set_fix_status(status.into());
        }
        // Thumbnails, one after another; stops when a new search starts.
        let (current, tx, dir) = (self.fix_request.clone(), self.bg_tx.clone(), self.paths.covers.join("thumbs"));
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let fetcher = ArtFetcher::new();
            for (index, c) in list.iter().enumerate() {
                if current.load(Ordering::Relaxed) != request {
                    return;
                }
                if let Ok(Some(data)) = fetcher.cover_for(&c.id, 250) {
                    if let Some(path) = scanner::save_cover(&data, &dir, &format!("thumb\u{1f}{}", c.id)) {
                        let _ = tx.send(Bg::FixThumb { request, index, path });
                    }
                }
            }
        });
    }

    pub fn fix_select(&mut self, index: usize) {
        let Some(c) = self.fix.candidates.get(index).cloned() else { return };
        self.fix.selected = Some(index);
        self.fix.local_cover = None;
        let has_thumb = self.fix.rows.get(index).map(|r| r.has_thumb).unwrap_or(false);
        if let Some(ui) = self.ui() {
            ui.set_fix_selected(index as i32);
            ui.set_fix_artist(c.artist.as_str().into());
            ui.set_fix_album(c.title.as_str().into());
            ui.set_fix_status(if has_thumb { "The cover shown will be downloaded when you save." } else { "No cover found for this one (yet) — you can choose your own image." }.into());
            if has_thumb {
                let image = self.fix.rows[index].thumb.clone();
                ui.set_fix_preview(image);
                ui.set_fix_has_preview(true);
            }
        }
    }

    pub fn fix_set_local_image(&mut self, path: PathBuf) {
        if Image::load_from_path(&path).is_err() {
            self.show_toast("That image could not be opened");
            return;
        }
        self.fix.local_cover = Some(path.clone());
        self.set_fix_preview(Some(path));
        if let Some(ui) = self.ui() {
            ui.set_fix_status("Your image will be used as the cover.".into());
        }
    }

    pub fn fix_save(&mut self, artist: String, album: String) {
        let Some((old_artist, old_album)) = self.fix.target.clone() else { return };
        if self.fix.saving {
            return;
        }
        let (artist, album) = (artist.trim().to_string(), album.trim().to_string());
        if artist.is_empty() || album.is_empty() {
            if let Some(ui) = self.ui() {
                ui.set_fix_status("Artist and album can't be empty.".into());
            }
            return;
        }
        let selected = self.fix.selected.and_then(|i| self.fix.candidates.get(i).cloned());
        let year = selected.as_ref().map(|c| c.year).unwrap_or(0);
        let download = selected.filter(|_| self.fix.local_cover.is_none()).map(|c| c.id);
        let local = self.fix.local_cover.clone();
        self.fix.saving = true;
        if let Some(ui) = self.ui() {
            ui.set_fix_busy(true);
            ui.set_fix_status(if download.is_some() { "Downloading the cover…" } else { "Saving…" }.into());
        }
        let (db, covers, tx) = (self.paths.database.clone(), self.paths.covers.clone(), self.bg_tx.clone());
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<(String, String, bool)> {
                let key = format!("fixed\u{1f}{}", scanner::album_key(&artist, &album));
                let data = match (&local, &download) {
                    (Some(file), _) => Some(std::fs::read(file)?),
                    (None, Some(id)) => ArtFetcher::new().cover_for(id, 500)?,
                    _ => None,
                };
                let cover = match data {
                    Some(d) => {
                        // Keyed by content, so choosing a different image takes effect.
                        let key = format!("{key}\u{1f}{}", d.len());
                        Some(scanner::save_cover(&d, &covers, &key).ok_or_else(|| anyhow::anyhow!("the image must be a JPEG or PNG"))?)
                    }
                    None => None,
                };
                Library::open(&db)?.fix_album(&old_artist, &old_album, &artist, &album, year, cover.as_deref())?;
                log(format!("fix: {old_artist} / {old_album} -> {artist} / {album} (cover: {})", cover.is_some()));
                Ok((artist, album, cover.is_some()))
            })();
            let _ = tx.send(Bg::FixSaved(result.map_err(|e| format!("{e:#}"))));
        });
    }

    pub fn track_menu_requested(&mut self, context: i32, index: i32) {
        self.menu_target = (context, index);
    }

    /// Songs the open menu refers to.
    fn menu_tracks(&self) -> Vec<Track> {
        let (context, index) = self.menu_target;
        self.list_tracks(context, index)
    }

    /// Song `index` of a song list (0 album, 1 songs, 2 playlist), or the
    /// whole open album (3).
    fn list_tracks(&self, context: i32, index: i32) -> Vec<Track> {
        let pick = |list: &Vec<Track>| list.get(index.max(0) as usize).cloned().into_iter().collect::<Vec<_>>();
        match context {
            0 => pick(&self.album_tracks),
            1 => pick(&self.songs),
            2 => pick(&self.playlist_tracks),
            3 => self.album_tracks.clone(),
            _ => Vec::new(),
        }
    }

    /// 0 play next, 1 add to queue, 2 add to playlist `playlist`,
    /// 3 new playlist, 4 remove from the open playlist.
    pub fn track_menu_action(&mut self, action: i32, playlist: i32) {
        if action == 5 {
            return self.open_fix_dialog();
        }
        let tracks = self.menu_tracks();
        if action == 6 || action == 7 {
            let index = self.menu_target.1.max(0) as usize;
            let to = if action == 6 { 0 } else { self.playlist_tracks.len().saturating_sub(1) };
            return self.move_playlist_track(index, to);
        }
        if action == 4 {
            let (Some(id), index) = (self.current_playlist_id(), self.menu_target.1) else { return };
            if let Err(e) = self.library.remove_from_playlist(id, index.max(0) as usize) {
                self.show_toast(&format!("Could not remove: {e}"));
            }
            self.open_playlist_id(id);
            return;
        }
        self.add_tracks(tracks, action, playlist);
    }

    /// 0 play next, 1 add to the queue, 2 add to playlist `playlist`,
    /// 3 make a new playlist of them.
    fn add_tracks(&mut self, tracks: Vec<Track>, action: i32, playlist: i32) {
        if tracks.is_empty() {
            return;
        }
        if matches!(action, 2 | 3) {
            self.remember_server_songs(&tracks);
        }
        let what = if tracks.len() == 1 {
            format!("\"{}\"", tracks[0].title)
        } else if tracks.iter().all(is_episode) {
            plural(tracks.len(), "episode", "episodes")
        } else {
            plural(tracks.len(), "song", "songs")
        };
        let paths: Vec<PathBuf> = tracks.iter().map(|t| t.path.clone()).collect();
        match action {
            0 | 1 => {
                let was_empty = self.queue.is_empty();
                let items: Vec<Item> = tracks.into_iter().map(Item::Track).collect();
                let at = if action == 0 { self.current.map(|c| c + 1).unwrap_or(0) } else { self.queue.len() };
                for (k, item) in items.into_iter().enumerate() {
                    self.queue.insert(at + k, item);
                }
                if let Some(c) = self.current {
                    if at <= c {
                        self.current = Some(c + paths.len());
                    }
                }
                self.history.clear();
                self.rebuild_queue_model();
                self.update_next();
                if was_empty || self.state == PlayState::Stopped && self.current.is_none() {
                    self.play_index(at);
                } else {
                    self.show_toast(&if action == 0 { format!("{what} will play next") } else { format!("Added {what} to the queue") });
                }
            }
            2 => {
                let Some(pl) = self.playlists.get(playlist.max(0) as usize).cloned() else { return };
                match self.library.add_to_playlist(pl.id, &paths) {
                    Ok(()) => {
                        self.show_toast(&format!("Added {what} to \"{}\"", pl.name));
                        let open = self.current_playlist_id() == Some(pl.id);
                        self.refresh_playlists();
                        if open {
                            self.open_playlist_id(pl.id);
                        }
                    }
                    Err(e) => self.show_toast(&format!("Could not add: {e}")),
                }
            }
            3 => {
                if let Some(id) = self.create_playlist_with("New Playlist", &paths) {
                    let name = self.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                    self.show_toast(&format!("Created \"{name}\" with {what}"));
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Drag and drop (desktop): songs, albums, artists and podcasts dragged
    // onto a sidebar playlist or the queue. A drag carries
    // "unitn:<what>", where <what> is set in the UI (see Dnd in widgets.slint).
    // ------------------------------------------------------------------

    /// The songs (or podcast episodes) a drag carries.
    fn dragged_tracks(&mut self, what: &str) -> Vec<Track> {
        let parts: Vec<&str> = what.split(':').collect();
        let num = |i: usize| parts.get(i).and_then(|p| p.parse::<i32>().ok()).unwrap_or(-1);
        match parts.first().copied().unwrap_or("") {
            "song" => self.list_tracks(num(1), num(2)),
            "album" => {
                let Some(album) = self.albums.get(num(1).max(0) as usize) else { return Vec::new() };
                self.library.album_tracks(&album.artist, &album.title).unwrap_or_default()
            }
            "artist" => {
                let Some(artist) = self.artists.get(num(1).max(0) as usize) else { return Vec::new() };
                let albums = self.library.albums(Some(&artist.name)).unwrap_or_default();
                albums.iter().flat_map(|a| self.library.album_tracks(&a.artist, &a.title).unwrap_or_default()).collect()
            }
            "show" => {
                // A whole show: its unplayed episodes, newest first (as listed).
                let Some(feed) = self.podcasts.get(num(1).max(0) as usize).map(|p| p.show.feed_url.clone()) else {
                    return Vec::new();
                };
                let episodes = self.library.episodes(&feed).unwrap_or_default();
                episodes.iter().filter(|e| !e.played).take(50).filter_map(|e| self.episode_track(&feed, e)).collect()
            }
            "episode" => {
                let Some(feed) = self.podcast_feed.clone() else { return Vec::new() };
                let Some(state) = self.podcast_episodes.get(num(1).max(0) as usize).cloned() else { return Vec::new() };
                self.episode_track(&feed, &state).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Something was dropped on the queue (-1), the "new playlist" button
    /// (-2) or sidebar playlist `target`.
    pub fn drop_items(&mut self, payload: &str, target: i32) {
        let Some(what) = payload.strip_prefix(DRAG_PREFIX) else { return };
        let tracks = self.dragged_tracks(what);
        log(format!("ui: dragged {what} ({} songs) to {target}", tracks.len()));
        if tracks.is_empty() {
            self.show_toast("Nothing to add");
            return;
        }
        match target {
            -1 => self.add_tracks(tracks, 1, -1),
            -2 => self.add_tracks(tracks, 3, -1),
            i => self.add_tracks(tracks, 2, i),
        }
    }

    // ------------------------------------------------------------------
    // Radio
    // ------------------------------------------------------------------

    fn station_items(&self) -> Vec<StationItem> {
        self.stations
            .iter()
            .map(|s| StationItem {
                key: station_key(s).into(),
                name: s.name.as_str().into(),
                info: s.info().into(),
                initial: initial(&s.name),
                favorite: self.favorites.contains(&s.uuid),
            })
            .collect()
    }

    fn set_stations(&mut self, list: Vec<Station>, status: String) {
        self.stations = list;
        let items = self.station_items();
        if let Some(ui) = self.ui() {
            ui.set_stations(model(items));
            ui.set_radio_loading(false);
            ui.set_radio_status(status.into());
        }
    }

    /// The Popular and Favorites chips. Popular clears every filter;
    /// Favorites switches on and off.
    pub fn load_radio_tab(&mut self, tab: i32) {
        match tab {
            RADIO_TAB_FAVORITES => self.radio_favorites = !self.radio_favorites,
            _ => {
                self.radio_favorites = false;
                self.radio_genres = [false; RADIO_GENRES.len()];
                self.radio_country = None;
                self.radio_language = None;
                self.radio_tag = None;
                self.radio_codec = None;
                self.radio_bitrate = 0;
                self.radio_query.clear();
            }
        }
        self.refresh_radio();
    }

    /// The filters of the Radio page as a directory search.
    fn radio_filter(&self) -> unitn_core::radio::Filter {
        let mut tags = self.selected_genres();
        if let Some((tag, _)) = &self.radio_tag {
            // A picked tag narrows the genre chips: with both, look for the tag.
            tags = vec![tag.clone()];
        }
        unitn_core::radio::Filter {
            name: self.radio_query.clone(),
            tags,
            country: self.radio_country.as_ref().map(|(v, _)| v.clone()),
            language: self.radio_language.as_ref().map(|(v, _)| v.clone()),
            codec: self.radio_codec.as_ref().map(|(v, _)| v.clone()),
            bitrate_min: self.radio_bitrate,
        }
    }

    /// Labels of the browse buttons ("Country" or the picked one) and which
    /// of them have something picked.
    fn radio_filter_labels(&self) -> (Vec<SharedString>, Vec<bool>) {
        let picked = [&self.radio_country, &self.radio_language, &self.radio_tag, &self.radio_codec];
        let mut labels = Vec::new();
        let mut active = Vec::new();
        for (i, name) in RADIO_FILTER_NAMES.iter().enumerate() {
            let label = match picked.get(i) {
                Some(Some((_, shown))) => Some(shown.clone()),
                Some(None) => None,
                None if self.radio_bitrate > 0 => Some(format!("{}+ kbps", self.radio_bitrate)),
                None => None,
            };
            active.push(label.is_some());
            labels.push(label.unwrap_or_else(|| name.to_string()).into());
        }
        (labels, active)
    }

    /// Opens a browse list: loads it from the directory the first time.
    pub fn radio_open_picker(&mut self, which: i32) {
        self.radio_picker = which;
        self.radio_picker_query.clear();
        if let Some(ui) = self.ui() {
            ui.set_radio_picker_title(RADIO_FILTER_NAMES.get(which.max(0) as usize).copied().unwrap_or("").into());
            ui.set_radio_picker_query("".into());
        }
        if let Some(kind) = facet_kind(which) {
            let slot = which as usize;
            if self.radio_facets[slot].is_none() {
                if let Some(ui) = self.ui() {
                    ui.set_radio_picker_loading(true);
                    ui.set_radio_picker_items(model(Vec::new()));
                }
                let radio = self.radio.clone();
                let tx = self.bg_tx.clone();
                thread::spawn(move || {
                    let result = radio.facets(kind).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Bg::RadioFacets { kind, result });
                });
                return;
            }
        }
        self.fill_radio_picker();
    }

    /// Narrows the open browse list to what's typed in its search box.
    pub fn radio_picker_search(&mut self, text: &str) {
        self.radio_picker_query = text.trim().to_lowercase();
        self.fill_radio_picker();
    }

    /// Shows the rows of the open browse list: "Any" first, then the
    /// countries (or languages…) with the most stations.
    fn fill_radio_picker(&mut self) {
        let which = self.radio_picker;
        let mut rows: Vec<Option<(String, String)>> = vec![None];
        let mut items: Vec<SharedString> = vec!["Any".into()];
        if which == RADIO_PICK_BITRATE {
            for kbps in RADIO_BITRATES.iter().skip(1) {
                rows.push(Some((kbps.to_string(), format!("{kbps}+ kbps"))));
                items.push(format!("{kbps} kbps or better").into());
            }
        } else if let Some(Some(facets)) = self.radio_facets.get(which.max(0) as usize) {
            let query = &self.radio_picker_query;
            for f in facets.iter().filter(|f| query.is_empty() || f.name.to_lowercase().contains(query.as_str())).take(RADIO_PICKER_ROWS) {
                rows.push(Some((f.value.clone(), f.name.clone())));
                items.push(format!("{}  ·  {}", f.name, f.stations).into());
            }
        }
        self.radio_picker_rows = rows;
        if let Some(ui) = self.ui() {
            ui.set_radio_picker_loading(false);
            ui.set_radio_picker_items(model(items));
        }
    }

    /// A browse list arrived from the directory.
    fn radio_facets_loaded(&mut self, kind: unitn_core::radio::FacetKind, result: Result<Vec<unitn_core::radio::Facet>, String>) {
        let slot = facet_slot(kind);
        match result {
            Ok(list) => self.radio_facets[slot] = Some(list),
            Err(e) => {
                self.show_toast(&format!("Could not load the list: {e}"));
                if let Some(ui) = self.ui() {
                    ui.set_radio_picker_loading(false);
                }
                return;
            }
        }
        if self.radio_picker == slot as i32 {
            self.fill_radio_picker();
        }
    }

    /// A row of the open browse list was picked.
    pub fn radio_picker_pick(&mut self, index: usize) {
        let Some(row) = self.radio_picker_rows.get(index).cloned() else { return };
        match self.radio_picker {
            RADIO_PICK_COUNTRY => self.radio_country = row,
            RADIO_PICK_LANGUAGE => self.radio_language = row,
            RADIO_PICK_TAG => self.radio_tag = row,
            RADIO_PICK_CODEC => self.radio_codec = row,
            RADIO_PICK_BITRATE => self.radio_bitrate = row.and_then(|(v, _)| v.parse().ok()).unwrap_or(0),
            _ => return,
        }
        self.radio_picker = -1;
        self.refresh_radio();
    }

    fn selected_genres(&self) -> Vec<String> {
        RADIO_GENRES.iter().zip(self.radio_genres).filter(|(_, on)| *on).map(|(g, _)| g.to_string()).collect()
    }

    /// Shows the stations for the current filters. Selected genres combine
    /// with OR (jazz or rock), the other filters with AND (German and jazz
    /// or rock, among the favorites, matching the search).
    fn refresh_radio(&mut self) {
        let mut filter = self.radio_filter();
        filter.name.clear();
        let filtered = !filter.is_empty();
        let (labels, active) = self.radio_filter_labels();
        self.radio_tab = if self.radio_favorites {
            RADIO_TAB_FAVORITES
        } else if filtered || !self.radio_query.is_empty() {
            RADIO_TAB_OTHER
        } else {
            RADIO_TAB_POPULAR
        };
        if let Some(ui) = self.ui() {
            ui.set_radio_tab(self.radio_tab);
            ui.set_radio_filter_labels(model(labels));
            ui.set_radio_filter_active(model(active));
            ui.set_radio_filtered(filtered);
            ui.set_radio_genres(model(self.radio_genres.to_vec()));
        }
        if self.radio_favorites {
            self.radio_request += 1; // cancel pending network results
            // Saved favorites know their country by name, not by code, and
            // don't know their language.
            let country = self.radio_country.as_ref().map(|(_, name)| name.clone());
            let favorites: Vec<Station> = self
                .library
                .favorite_stations()
                .unwrap_or_default()
                .into_iter()
                .filter(|s| country.as_ref().map_or(true, |c| s.country.eq_ignore_ascii_case(c)))
                .filter(|s| filter.tags.is_empty() || filter.tags.iter().any(|g| s.has_tag(g)))
                .filter(|s| filter.codec.as_ref().map_or(true, |c| s.codec.eq_ignore_ascii_case(c)))
                .filter(|s| s.bitrate >= filter.bitrate_min)
                .collect();
            let status = if favorites.is_empty() && filtered {
                "No favorites match these filters".to_string()
            } else {
                plural(favorites.len(), "favorite", "favorites")
            };
            self.set_stations(favorites, status);
        } else if self.radio_tab == RADIO_TAB_POPULAR {
            self.fetch_stations(StationQuery::Top);
        } else {
            self.fetch_stations(StationQuery::Filtered(self.radio_filter()));
        }
    }

    fn fetch_stations(&mut self, query: StationQuery) {
        self.radio_request += 1;
        let request = self.radio_request;
        let radio = self.radio.clone();
        let tx = self.bg_tx.clone();
        if let Some(ui) = self.ui() {
            ui.set_radio_loading(true);
            ui.set_radio_status("Loading stations…".into());
        }
        thread::spawn(move || {
            let result = match query {
                StationQuery::Top => radio.top(RADIO_LIMIT),
                StationQuery::Filtered(filter) => {
                    // A whole country or language fits in one longer list.
                    let limit = if filter.tags.is_empty() && filter.name.is_empty() { 300 } else { RADIO_LIMIT };
                    radio.filtered(&filter, limit)
                }
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Stations { request, result });
        });
    }

    /// Searches the directory by name, within the selected filters.
    pub fn radio_search(&mut self, query: &str) {
        self.radio_query = query.trim().to_string();
        if !self.radio_query.is_empty() {
            self.radio_favorites = false;
        }
        self.refresh_radio();
    }

    /// Switches a genre chip on or off.
    pub fn radio_genre(&mut self, index: usize) {
        if let Some(on) = self.radio_genres.get_mut(index) {
            *on = !*on;
            self.refresh_radio();
        }
    }

    pub fn toggle_favorite(&mut self, index: usize) {
        let Some(station) = self.stations.get(index).cloned() else { return };
        let result = if self.favorites.remove(&station.uuid) {
            self.library.remove_favorite_station(&station.uuid)
        } else {
            self.favorites.insert(station.uuid.clone());
            self.library.add_favorite_station(&station)
        };
        if let Err(e) = result {
            self.show_toast(&format!("Could not update favorites: {e}"));
        }
        if self.radio_favorites {
            self.refresh_radio();
        } else {
            let items = self.station_items();
            if let Some(ui) = self.ui() {
                ui.set_stations(model(items));
            }
        }
    }

    /// Saves a station from a URL the user entered (shown under Favorites).
    pub fn add_custom_stream(&mut self, name: &str, url: &str) {
        let url = url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.show_toast("Please enter a stream address starting with http:// or https://");
            return;
        }
        let name = if name.trim().is_empty() { url.to_string() } else { name.trim().to_string() };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&url, &mut h);
        let station = Station {
            uuid: format!("custom-{:016x}", std::hash::Hasher::finish(&h)),
            name: name.clone(),
            url: url.to_string(),
            tags: "custom".into(),
            ..Default::default()
        };
        match self.library.add_favorite_station(&station) {
            Ok(()) => {
                self.favorites.insert(station.uuid.clone());
                self.show_toast(&format!("Added \"{name}\" to Favorites"));
                self.radio_favorites = true;
                self.refresh_radio();
            }
            Err(e) => self.show_toast(&format!("Could not add the stream: {e}")),
        }
    }

    pub fn play_station(&mut self, index: usize) {
        if index >= self.stations.len() {
            return;
        }
        self.set_queue(self.stations.iter().cloned().map(Item::Station).collect());
        self.play_index(index);
    }

    // ------------------------------------------------------------------
    // Queue & transport
    // ------------------------------------------------------------------

    fn current_item(&self) -> Option<&Item> {
        self.current.and_then(|i| self.queue.get(i))
    }

    fn queue_entry(&self, index: usize, item: &Item) -> QueueEntry {
        let current = self.current == Some(index);
        match item {
            Item::Track(t) => QueueEntry {
                title: t.title.as_str().into(),
                subtitle: format!("{} — {}", t.artist, t.album).into(),
                duration: if t.duration_ms > 0 { format_time(t.duration_secs()).into() } else { SharedString::new() },
                current,
                radio: false,
            },
            Item::Station(s) => QueueEntry {
                title: s.name.as_str().into(),
                subtitle: s.info().into(),
                duration: "LIVE".into(),
                current,
                radio: true,
            },
        }
    }

    fn set_queue(&mut self, items: Vec<Item>) {
        self.queue = items;
        self.current = None;
        self.history.clear();
        self.rebuild_queue_model();
    }

    fn rebuild_queue_model(&self) {
        let entries: Vec<QueueEntry> = self.queue.iter().enumerate().map(|(i, it)| self.queue_entry(i, it)).collect();
        self.queue_model.set_vec(entries);
    }

    /// Updates the `current` flag of two queue rows without rebuilding the model.
    fn mark_current(&mut self, new: Option<usize>) {
        let old = self.current;
        self.current = new;
        for i in [old, new].into_iter().flatten() {
            if i < self.queue_model.row_count() {
                let entry = self.queue_entry(i, &self.queue[i]);
                self.queue_model.set_row_data(i, entry);
            }
        }
    }

    fn play_tracks(&mut self, tracks: Vec<Track>, start: usize, shuffle: bool) {
        if tracks.is_empty() {
            return;
        }
        let n = tracks.len();
        self.set_queue(tracks.into_iter().map(Item::Track).collect());
        let start = if shuffle {
            self.settings.shuffle = true;
            if let Some(ui) = self.ui() {
                ui.set_shuffle(true);
            }
            (self.rand() % n as u64) as usize
        } else {
            start.min(n - 1)
        };
        self.play_index(start);
    }

    pub fn play_album(&mut self, shuffle: bool) {
        self.play_tracks(self.album_tracks.clone(), 0, shuffle);
    }

    pub fn play_album_track(&mut self, index: usize) {
        self.play_tracks(self.album_tracks.clone(), index, false);
    }

    pub fn play_song(&mut self, index: usize) {
        self.play_tracks(self.songs.clone(), index, false);
    }

    /// Plays a file path or a stream URL immediately (used by the test hook).
    pub fn autoplay(&mut self, target: &str) {
        log(format!("ui: autoplay {target}"));
        if target.starts_with("http://") || target.starts_with("https://") {
            let station = Station { uuid: "autoplay".into(), name: "Test stream".into(), url: target.into(), ..Default::default() };
            self.set_queue(vec![Item::Station(station)]);
        } else {
            // "a.wav,b.wav" queues several files (used to test gapless playback).
            let tracks: Vec<Item> = target
                .split(',')
                .map(|p| Item::Track(scanner::read_track(&PathBuf::from(p.trim()), &self.paths.covers)))
                .collect();
            self.set_queue(tracks);
        }
        self.play_index(0);
    }

    fn play_index(&mut self, index: usize) {
        let Some(item) = self.queue.get(index).cloned() else { return };
        self.logged_second = -1;
        self.start_listen(&item);
        self.mark_current(Some(index));
        self.position = 0.0;
        self.stream_title.clear();
        match &item {
            Item::Track(t) => {
                self.duration = Some(t.duration_secs()).filter(|d| *d > 0.0);
                self.player.load(t.source());
            }
            Item::Station(s) => {
                self.duration = None;
                self.player.load(Source::Url(s.url.clone()));
                let radio = self.radio.clone();
                let uuid = s.uuid.clone();
                thread::spawn(move || radio.count_click(&uuid));
            }
        }
        self.refresh_now_playing();
        self.refresh_position();
        self.update_next();
        self.save_session();
    }

    /// Files or folders dropped on the window: audio files play right away,
    /// folders are added to the library (and played), M3U files are imported.
    pub fn handle_drop(&mut self, paths: Vec<PathBuf>) {
        log(format!("ui: dropped {} item(s)", paths.len()));
        let mut files: Vec<PathBuf> = Vec::new();
        let mut added_folders = 0;
        for path in paths {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if path.is_dir() {
                files.extend(scanner::audio_files_in(&path));
                if self.library.add_folder(&path).is_ok() {
                    added_folders += 1;
                }
            } else if ext == "m3u" || ext == "m3u8" {
                self.import_playlist(path);
            } else if scanner::is_audio(&path) {
                files.push(path);
            }
        }
        if added_folders > 0 {
            self.refresh_folders();
            self.start_scan();
        }
        if files.is_empty() {
            if added_folders == 0 {
                self.show_toast("No music files in what was dropped");
            }
            return;
        }
        files.truncate(2000);
        let tracks: Vec<Track> = files.iter().map(|f| scanner::read_track(f, &self.paths.covers)).collect();
        let n = tracks.len();
        self.play_tracks(tracks, 0, false);
        self.show_toast(&format!("Playing {}", plural(n, "dropped song", "dropped songs")));
    }

    /// Drag & drop reordering in the queue.
    pub fn move_queue_item(&mut self, from: usize, to: usize) {
        if from >= self.queue.len() || to >= self.queue.len() || from == to {
            return;
        }
        let item = self.queue.remove(from);
        self.queue.insert(to, item);
        if let Some(c) = self.current {
            self.current = Some(if c == from {
                to
            } else if from < c && c <= to {
                c - 1
            } else if to <= c && c < from {
                c + 1
            } else {
                c
            });
        }
        self.history.clear();
        self.rebuild_queue_model();
        self.update_next();
    }

    /// Decides which queue entry follows the current one and lets the engine
    /// prepare it, so the switch happens without a gap.
    fn update_next(&mut self) {
        let next = match self.current {
            Some(cur) if matches!(self.queue.get(cur), Some(Item::Track(_))) => {
                let n = self.queue.len();
                if self.settings.repeat == 2 {
                    Some(cur)
                } else if self.settings.shuffle && n > 1 {
                    let mut r = (self.rand() % n as u64) as usize;
                    if r == cur {
                        r = (r + 1) % n;
                    }
                    Some(r)
                } else if cur + 1 < n {
                    Some(cur + 1)
                } else if self.settings.repeat == 1 {
                    Some(0)
                } else {
                    None
                }
            }
            _ => None,
        };
        let source = next.and_then(|i| match self.queue.get(i) {
            Some(Item::Track(t)) => Some(t.source()),
            _ => None,
        });
        self.planned_next = if source.is_some() { next } else { None };
        // Optionally keep songs of the same album gapless (live albums, DJ mixes).
        let same_album = self.settings.gapless_albums && match (self.current.and_then(|c| self.queue.get(c)), next.and_then(|i| self.queue.get(i))) {
            (Some(Item::Track(a)), Some(Item::Track(b))) => {
                !a.album.is_empty() && a.album == b.album && a.grouping_artist() == b.grouping_artist()
            }
            _ => false,
        };
        let fade = if same_album { 0.0 } else { self.settings.crossfade.max(0) as f64 };
        if self.settings.crossfade > 0 && source.is_some() {
            log(format!("crossfade into the next song: {fade} s{}", if same_album { " (same album: gapless)" } else { "" }));
        }
        self.player.set_crossfade(fade);
        self.player.set_next(source);
    }

    pub fn play_queue_index(&mut self, index: usize) {
        if let Some(cur) = self.current {
            self.history.push(cur);
        }
        self.play_index(index);
    }

    pub fn remove_queue_index(&mut self, index: usize) {
        if index >= self.queue.len() {
            return;
        }
        self.queue.remove(index);
        self.history.clear();
        match self.current {
            Some(c) if c == index => {
                self.current = None;
                self.rebuild_queue_model();
                if index < self.queue.len() && self.state != PlayState::Stopped {
                    self.play_index(index);
                } else {
                    self.player.stop();
                    self.refresh_now_playing();
                }
                return;
            }
            Some(c) if c > index => self.current = Some(c - 1),
            _ => {}
        }
        self.rebuild_queue_model();
        self.update_next();
    }

    pub fn clear_queue(&mut self) {
        self.player.stop();
        self.set_queue(Vec::new());
        self.duration = None;
        self.position = 0.0;
        self.refresh_now_playing();
        self.refresh_position();
    }

    pub fn toggle_play(&mut self) {
        let is_radio = matches!(self.current_item(), Some(Item::Station(_)));
        match self.state {
            PlayState::Playing | PlayState::Buffering => {
                if is_radio {
                    self.player.stop(); // live streams resume from "now"
                } else {
                    self.player.pause();
                }
            }
            PlayState::Paused => self.player.play(),
            PlayState::Stopped => {
                if let Some(i) = self.current {
                    self.play_index(i);
                } else if !self.queue.is_empty() {
                    self.play_index(0);
                } else if !self.songs.is_empty() {
                    self.play_tracks(self.songs.clone(), 0, self.settings.shuffle);
                }
            }
        }
    }

    pub fn next(&mut self, automatic: bool) {
        let n = self.queue.len();
        if n == 0 {
            return;
        }
        let Some(cur) = self.current else {
            self.play_index(0);
            return;
        };
        if automatic && self.settings.repeat == 2 {
            self.play_index(cur);
            return;
        }
        let next = if self.settings.shuffle && n > 1 {
            let mut r = (self.rand() % n as u64) as usize;
            if r == cur {
                r = (r + 1) % n;
            }
            Some(r)
        } else if cur + 1 < n {
            Some(cur + 1)
        } else if self.settings.repeat == 1 {
            Some(0)
        } else {
            None
        };
        match next {
            Some(i) => {
                self.history.push(cur);
                self.play_index(i);
            }
            None if automatic => {
                // End of the queue.
                self.state = PlayState::Stopped;
                self.position = 0.0;
                self.refresh_play_state();
                self.refresh_position();
            }
            None => {}
        }
    }

    pub fn previous(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let is_track = matches!(self.current_item(), Some(Item::Track(_)));
        if is_track && self.position > 3.0 {
            self.player.seek(0.0);
            return;
        }
        let Some(cur) = self.current else {
            self.play_index(0);
            return;
        };
        let prev = if self.settings.shuffle {
            self.history.pop()
        } else if cur > 0 {
            Some(cur - 1)
        } else {
            None
        };
        match prev {
            Some(i) => self.play_index(i),
            None if is_track => self.player.seek(0.0),
            None => self.play_index(cur),
        }
    }

    pub fn seek_fraction(&mut self, fraction: f32) {
        if !matches!(self.current_item(), Some(Item::Track(_))) {
            return;
        }
        if let Some(d) = self.duration {
            let target = fraction.clamp(0.0, 1.0) as f64 * d;
            self.player.seek(target);
            self.position = target;
            self.refresh_position();
        }
    }

    pub fn seek_relative(&mut self, delta: f64) {
        if !matches!(self.current_item(), Some(Item::Track(_))) {
            return;
        }
        let duration = self.duration.unwrap_or(f64::MAX);
        let target = (self.position + delta).clamp(0.0, (duration - 0.5).max(0.0));
        self.player.seek(target);
        self.position = target;
        self.refresh_position();
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.settings.volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(self.settings.volume);
    }

    pub fn toggle_shuffle(&mut self) {
        self.settings.shuffle = !self.settings.shuffle;
        self.history.clear();
        self.update_next();
        if let Some(ui) = self.ui() {
            ui.set_shuffle(self.settings.shuffle);
        }
    }

    pub fn cycle_repeat(&mut self) {
        self.settings.repeat = (self.settings.repeat + 1) % 3;
        self.update_next();
        if let Some(ui) = self.ui() {
            ui.set_repeat_mode(self.settings.repeat);
        }
        let label = ["Repeat off", "Repeat all", "Repeat one"][self.settings.repeat as usize];
        self.show_toast(label);
    }

    pub fn set_theme(&mut self, mode: i32) {
        self.settings.theme = mode;
        self.save_settings();
    }

    // ------------------------------------------------------------------
    // UI refresh helpers
    // ------------------------------------------------------------------

    fn refresh_play_state(&mut self) {
        let playing = matches!(self.state, PlayState::Playing | PlayState::Buffering);
        if let Some(ui) = self.ui() {
            ui.set_playing(playing);
            ui.set_buffering(self.state == PlayState::Buffering);
        }
        self.media.set_state(playing, self.state == PlayState::Paused, self.position);
    }

    fn refresh_position(&mut self) {
        let (fraction, pos, dur, seekable) = match (self.current_item(), self.duration) {
            (Some(Item::Station(_)), _) => (0.0, format_time(self.position), "LIVE".to_string(), false),
            (Some(Item::Track(_)), Some(d)) if d > 0.0 => {
                ((self.position / d).clamp(0.0, 1.0) as f32, format_time(self.position), format_time(d), true)
            }
            (Some(Item::Track(_)), _) => (0.0, format_time(self.position), "--:--".to_string(), false),
            (None, _) => (0.0, "0:00".to_string(), "0:00".to_string(), false),
        };
        if let Some(ui) = self.ui() {
            ui.set_position(fraction);
            ui.set_position_text(pos.into());
            ui.set_duration_text(dur.into());
            ui.set_seekable(seekable);
        }
    }

    fn refresh_now_playing(&mut self) {
        let Some(ui) = self.ui() else { return };
        match self.current_item().cloned() {
            Some(Item::Track(t)) => {
                let (cover, has_cover) = self.cover(&t.cover);
                ui.set_np_title(t.title.as_str().into());
                // Podcast episodes: the show is both "artist" and "album".
                let subtitle = if t.album.is_empty() || t.album == t.artist {
                    t.artist.clone()
                } else {
                    format!("{} — {}", t.artist, t.album)
                };
                ui.set_np_subtitle(subtitle.into());
                ui.set_np_cover(cover);
                ui.set_np_has_cover(has_cover);
                ui.set_is_radio(false);
                ui.set_has_track(true);
                ui.set_current_key(track_key(&t).into());
                // Podcast episodes aren't albums.
                let album = if t.path.to_string_lossy().starts_with("podcast://") {
                    String::new()
                } else {
                    album_key(t.grouping_artist(), &t.album)
                };
                ui.set_playing_album(album.into());
                self.media.set_metadata(&t.title, &t.artist, &t.album, t.cover.as_deref(), self.duration);
                let song = (!t.is_remote()).then(|| t.path.clone());
                self.update_backdrop(t.grouping_artist(), song.as_deref());
            }
            Some(Item::Station(s)) => {
                let (title, subtitle) = if self.stream_title.is_empty() {
                    let info = s.info();
                    (s.name.clone(), if info.is_empty() { "Internet radio".to_string() } else { info })
                } else {
                    (self.stream_title.clone(), s.name.clone())
                };
                ui.set_np_title(title.as_str().into());
                ui.set_np_subtitle(subtitle.into());
                ui.set_np_cover(Image::default());
                ui.set_np_has_cover(false);
                ui.set_is_radio(true);
                ui.set_has_track(true);
                ui.set_current_key(station_key(&s).into());
                ui.set_playing_album(SharedString::new());
                self.media.set_metadata(&title, &s.name, "", None, None);
                self.show_backdrop(None);
            }
            None => {
                ui.set_np_title("Not playing".into());
                ui.set_np_subtitle(SharedString::new());
                ui.set_np_cover(Image::default());
                ui.set_np_has_cover(false);
                ui.set_is_radio(false);
                ui.set_has_track(false);
                ui.set_current_key(SharedString::new());
                ui.set_playing_album(SharedString::new());
            }
        }
        self.update_liked();
        self.load_lyrics();
    }

    // ------------------------------------------------------------------
    // Crossfade and lyrics
    // ------------------------------------------------------------------

    pub fn set_crossfade(&mut self, seconds: i32) {
        self.settings.crossfade = seconds.clamp(0, 12);
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_crossfade(self.settings.crossfade);
        }
        self.update_next();
    }

    pub fn set_gapless_albums(&mut self, on: bool) {
        self.settings.gapless_albums = on;
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_gapless_albums(on);
        }
        self.update_next();
    }

    pub fn set_resume_session(&mut self, on: bool) {
        self.settings.resume_session = on;
        self.save_settings();
        self.save_session();
        if let Some(ui) = self.ui() {
            ui.set_resume_session(on);
        }
    }

    /// Saves the queue, the current song and the position for the next start
    /// (or removes the saved one when "Continue where I left off" is off).
    // ------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------

    /// A new song starts: the one before is written down if it was heard
    /// long enough. Radio and podcast episodes aren't counted.
    fn start_listen(&mut self, item: &Item) {
        self.finish_listen();
        self.listen = match item {
            Item::Track(t) if !is_episode(t) => Some(Listen { track: t.clone(), heard: 0.0, last: 0.0 }),
            _ => None,
        };
    }

    /// The playback position moved: time played on (not jumps) is heard.
    fn note_listening(&mut self, position: f64) {
        let playing = self.state == PlayState::Playing;
        if let Some(l) = self.listen.as_mut() {
            let step = position - l.last;
            if playing && step > 0.0 && step <= 2.5 {
                l.heard += step;
            }
            l.last = position;
        }
    }

    /// Writes down the current song as played if it was heard for 30
    /// seconds (half of a short song). Also called when the app closes.
    pub fn finish_listen(&mut self) {
        let Some(l) = self.listen.take() else { return };
        if l.heard < unitn_core::library::count_threshold(l.track.duration_secs()) {
            return;
        }
        if let Err(e) = self.library.record_play(&l.track, l.heard, unix_now()) {
            log(&format!("statistics: could not record a play: {e:#}"));
        }
        if self.page == PAGE_STATS {
            self.refresh_stats();
        }
    }

    pub fn set_stats_period(&mut self, period: i32) {
        self.stats_period = period;
        self.refresh_stats();
    }

    /// Fills the Statistics page.
    fn refresh_stats(&mut self) {
        let now = unix_now();
        let since = if self.stats_period == 0 { now - i64::from(STATS_DAYS) * 86_400 } else { 0 };
        let stats = self.library.play_stats(since, STATS_TOP).unwrap_or_default();
        let totals = self.library.library_totals().unwrap_or_default();
        let days = self.library.listening_per_day(STATS_DAYS, now).unwrap_or_default();
        let empty = self.library.play_count().unwrap_or(0) == 0;

        let tile = |value: String, label: &str| StatTile { value: value.into(), label: label.into() };
        let first = |rows: &[unitn_core::library::Ranked]| rows.first().map(|r| r.name.clone()).unwrap_or_else(|| "—".into());
        let tiles = vec![
            tile(grouped(stats.plays), "Plays"),
            tile(long_time(stats.listened_secs), "Listening time"),
            tile(first(&stats.artists), "Top artist"),
            tile(first(&stats.genres), "Top genre"),
        ];
        let library = vec![
            tile(grouped(totals.songs), "Songs"),
            tile(grouped(totals.albums), "Albums"),
            tile(grouped(totals.artists), "Artists"),
            tile(grouped(totals.genres), "Genres"),
            tile(long_time(totals.duration_secs), "To listen to it all"),
        ];
        let rows = |list: &[unitn_core::library::Ranked]| -> Vec<StatRow> {
            let most = list.first().map_or(1, |r| r.plays.max(1)) as f32;
            list.iter()
                .map(|r| StatRow {
                    name: r.name.as_str().into(),
                    detail: r.detail.as_str().into(),
                    count: plural(r.plays as usize, "play", "plays").into(),
                    share: r.plays as f32 / most,
                })
                .collect()
        };
        let busiest = days.iter().map(|(_, s)| *s).max().unwrap_or(0).max(1) as f32;
        let days_total: u64 = days.iter().map(|(_, s)| *s).sum();
        let count = days.len();
        let bars: Vec<DayBar> = days
            .iter()
            .enumerate()
            .map(|(i, (day, secs))| {
                // "2026-11-25": label every fifth day and today.
                let day_no = day.get(8..10).unwrap_or("").trim_start_matches('0').to_string();
                let label = if i + 1 == count || (count - 1 - i) % 5 == 0 { day_no.clone() } else { String::new() };
                let tip = format!("{} · {}", short_date(day), if *secs > 0 { long_time(*secs) } else { "nothing".into() });
                DayBar { label: label.into(), share: *secs as f32 / busiest, tip: tip.into() }
            })
            .collect();

        let Some(ui) = self.ui() else { return };
        ui.set_stats_period(self.stats_period);
        ui.set_stats_tiles(model(tiles));
        ui.set_stats_library(model(library));
        ui.set_stats_days(model(bars));
        ui.set_stats_days_total(format!("{} in total", long_time(days_total)).into());
        ui.set_stats_songs(model(rows(&stats.songs)));
        ui.set_stats_artists(model(rows(&stats.artists)));
        ui.set_stats_albums(model(rows(&stats.albums)));
        ui.set_stats_genres(model(rows(&stats.genres)));
        ui.set_stats_empty(empty);
    }

    pub fn save_session(&self) {
        let path = &self.paths.session;
        if !self.settings.resume_session || self.current.is_none() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let session = Session { queue: self.queue.clone(), current: self.current, position: self.position };
        let Ok(json) = serde_json::to_vec(&session) else { return };
        // Write to a temporary file first, so a crash can't leave half a file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Puts the last session's song back on the player, paused; Play
    /// continues where it was.
    pub fn restore_session(&mut self) {
        if !self.settings.resume_session {
            return;
        }
        let Ok(data) = std::fs::read(&self.paths.session) else { return };
        let Ok(session) = serde_json::from_slice::<Session>(&data) else {
            log("session: could not read the saved session");
            return;
        };
        let Some(current) = session.current.filter(|&i| i < session.queue.len()) else { return };
        // A local song that's gone (moved or deleted) can't continue.
        if let Item::Track(t) = &session.queue[current] {
            if !t.is_remote() && !is_episode(t) && !t.path.is_file() {
                log("session: the last song's file is gone; not restoring");
                return;
            }
        }
        log(format!("session: restoring {} song(s), at {:.0} s", session.queue.len(), session.position));
        self.queue = session.queue;
        self.rebuild_queue_model();
        self.mark_current(Some(current));
        self.position = 0.0;
        if let Item::Track(t) = &self.queue[current] {
            self.duration = Some(t.duration_secs()).filter(|d| *d > 0.0);
            if session.position > 3.0 {
                self.position = session.position;
                self.resume_at = Some((t.path.clone(), session.position));
            }
        }
        self.refresh_now_playing();
        self.refresh_position();
        self.update_next();
    }

    pub fn set_startup_sound(&mut self, on: bool) {
        self.settings.startup_sound = on;
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_startup_sound(on);
        }
    }

    pub fn set_lyrics_online(&mut self, on: bool) {
        self.settings.lyrics_online = on;
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_lyrics_online(on);
        }
        // Look again with the new setting.
        self.lyrics_for = None;
        self.load_lyrics();
    }

    pub fn set_lyrics_open(&mut self, open: bool) {
        self.settings.lyrics_open = open;
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_lyrics_open(open);
        }
        self.load_lyrics();
    }

    /// The current song, if it's a file on this computer (not a server
    /// song, a podcast episode or a radio station).
    fn current_local_song(&self) -> Option<PathBuf> {
        match self.current_item() {
            Some(Item::Track(t)) if !t.is_remote() && !is_episode(t) && t.path.is_file() => Some(t.path.clone()),
            _ => None,
        }
    }

    /// Saves the shown lyrics with the song: `kind` 0 as a file next to it
    /// (.lrc or .txt), 1 in the song's tags. `auto`: from the "Save lyrics
    /// found online" setting (only a short note when it worked).
    pub fn save_lyrics(&mut self, kind: i32, auto: bool) {
        let (Some(song), Some(found)) = (self.current_local_song(), self.lyrics.clone()) else { return };
        if found.instrumental || found.lines.is_empty() {
            return;
        }
        let result = if kind == 0 {
            lyrics::save_sidecar(&song, &found).map(|f| {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                format!("Lyrics saved as {name}")
            })
        } else {
            lyrics::save_to_tags(&song, &found).map(|_| "Lyrics saved in the song".to_string())
        };
        match result {
            Ok(msg) => {
                log(format!("lyrics: {msg} ({})", song.display()));
                if !auto {
                    self.show_toast(&msg);
                }
            }
            Err(e) => {
                log(format!("lyrics: saving failed for {}: {e:#}", song.display()));
                self.show_toast(&format!("Couldn't save the lyrics: {e}"));
            }
        }
    }

    pub fn set_lyrics_autosave(&mut self, value: i32) {
        self.settings.lyrics_autosave = value.clamp(0, 3) as u8;
        self.save_settings();
        if let Some(ui) = self.ui() {
            ui.set_lyrics_autosave(self.settings.lyrics_autosave as i32);
        }
    }

    fn show_lyrics(&mut self, lines: Vec<String>, synced: bool, source: &str, status: &str) {
        self.lyrics_line = None;
        if let Some(ui) = self.ui() {
            ui.set_lyrics_can_save(false);
            ui.set_lyrics_lines(model(lines.into_iter().map(SharedString::from).collect()));
            ui.set_lyrics_synced(synced);
            ui.set_lyrics_source(source.into());
            ui.set_lyrics_status(status.into());
            ui.set_lyrics_current(-1);
        }
        self.update_lyrics_line();
    }

    /// Finds lyrics for the current song when the lyrics panel is open.
    fn load_lyrics(&mut self) {
        if !self.lyrics_wanted() {
            return;
        }
        let track = match self.current_item().cloned() {
            Some(Item::Track(t)) => t,
            Some(Item::Station(_)) => {
                self.lyrics_for = None;
                self.lyrics = None;
                return self.show_lyrics(Vec::new(), false, "", "Lyrics aren't available for radio.");
            }
            None => {
                self.lyrics_for = None;
                self.lyrics = None;
                return self.show_lyrics(Vec::new(), false, "", "Play a song to see its lyrics here.");
            }
        };
        if self.lyrics_for.as_ref() == Some(&track.path) {
            return; // already shown or being looked up
        }
        self.lyrics_for = Some(track.path.clone());
        self.lyrics = None;
        if self.episode_keys.contains_key(&track.path) {
            return self.show_lyrics(Vec::new(), false, "", "Podcasts don't have lyrics.");
        }
        self.show_lyrics(Vec::new(), false, "", "Looking for lyrics…");
        let (tx, db, online) = (self.bg_tx.clone(), self.paths.database.clone(), self.settings.lyrics_online);
        thread::spawn(move || {
            let (lyrics, failed) = find_lyrics(&track, &db, online);
            let _ = tx.send(Bg::Lyrics { track: track.path, lyrics, failed });
        });
    }

    fn on_lyrics(&mut self, track: PathBuf, found: Option<Lyrics>, failed: bool) {
        if self.lyrics_for.as_ref() != Some(&track) {
            return;
        }
        match found {
            Some(l) if l.instrumental => {
                self.show_lyrics(Vec::new(), false, "", "♪  Instrumental  ♪");
            }
            Some(l) => {
                let lines: Vec<String> = l
                    .lines
                    .iter()
                    .map(|x| if x.text.is_empty() && l.synced { "♪".to_string() } else { x.text.clone() })
                    .collect();
                let (synced, source) = (l.synced, l.source.clone());
                self.lyrics = Some(l);
                self.show_lyrics(lines, synced, &source, "");
                // Lyrics from LRCLIB for a song on this computer can be kept with it.
                let savable = source == "LRCLIB" && self.current_local_song().is_some();
                if let Some(ui) = self.ui() {
                    ui.set_lyrics_can_save(savable);
                }
                if savable {
                    match self.settings.lyrics_autosave {
                        1 => self.save_lyrics(0, true),
                        2 => self.save_lyrics(1, true),
                        3 => {
                            self.save_lyrics(0, true);
                            self.save_lyrics(1, true);
                        }
                        _ => {}
                    }
                }
            }
            None => {
                let status = if failed {
                    "Couldn't reach LRCLIB to look up lyrics. Check your internet connection."
                } else if self.settings.lyrics_online {
                    "No lyrics found for this song."
                } else {
                    "No lyrics found. Turn on \"Find lyrics online\" in Settings to look them up on LRCLIB."
                };
                self.show_lyrics(Vec::new(), false, "", status);
            }
        }
    }

    /// Highlights the line being sung.
    fn update_lyrics_line(&mut self) {
        let Some(l) = &self.lyrics else { return };
        // A little ahead, so the highlight doesn't lag behind the voice.
        let line = l.current_line(self.position + 0.2);
        if line != self.lyrics_line {
            self.lyrics_line = line;
            if let Some(ui) = self.ui() {
                ui.set_lyrics_current(line.map(|i| i as i32).unwrap_or(-1));
            }
        }
    }

    // ------------------------------------------------------------------
    // Podcasts
    // ------------------------------------------------------------------

    fn podcast_item(&mut self, show: &Show, cover: &Option<PathBuf>, info: String, subscribed: bool, favorite: bool) -> PodcastItem {
        let (image, has_image) = self.cover(cover);
        PodcastItem {
            title: show.title.as_str().into(),
            author: show.author.as_str().into(),
            info: info.into(),
            image,
            has_image,
            subscribed,
            favorite,
        }
    }

    fn refresh_podcasts(&mut self) {
        self.podcasts = self.library.podcasts().unwrap_or_default();
        let list = self.podcasts.clone();
        let items: Vec<PodcastItem> = list
            .iter()
            .map(|p| {
                let info = match p.unplayed {
                    0 => String::new(),
                    n => plural(n, "unplayed episode", "unplayed episodes"),
                };
                self.podcast_item(&p.show, &p.cover, info, true, p.favorite)
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_podcast_shows(model(items));
        }
        self.show_podcast_results();
    }

    fn show_podcast_results(&mut self) {
        let results = self.podcast_results.clone();
        let items: Vec<PodcastItem> = results
            .iter()
            .map(|s| {
                let subscribed = self.podcasts.iter().any(|p| p.show.feed_url == s.feed_url);
                self.podcast_item(s, &None, s.description.clone(), subscribed, false)
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_podcast_results(model(items));
        }
    }

    fn set_podcast_status(&self, loading: bool, status: &str) {
        if let Some(ui) = self.ui() {
            ui.set_podcast_loading(loading);
            ui.set_podcast_status(status.into());
        }
    }

    /// Searches the podcast directory, or reads a pasted RSS link.
    pub fn podcast_search(&mut self, query: &str) {
        let query = query.trim().to_string();
        let request = self.podcast_request.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ui) = self.ui() {
            ui.set_podcast_view(0);
            ui.set_podcast_searching(!query.is_empty());
        }
        self.podcast_feed = None;
        if query.is_empty() {
            self.podcast_results.clear();
            self.show_podcast_results();
            return self.set_podcast_status(false, "");
        }
        let is_link = query.starts_with("http://") || query.starts_with("https://");
        self.set_podcast_status(true, if is_link { "Reading the feed…" } else { "Searching podcasts…" });
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let result = if is_link {
                podcasts::fetch(&query).map(|feed| vec![feed.show])
            } else {
                podcasts::search(&query)
            };
            let _ = tx.send(Bg::PodcastResults { request, result: result.map_err(|e| format!("{e:#}")) });
        });
    }

    fn on_podcast_results(&mut self, request: u64, result: Result<Vec<Show>, String>) {
        if request != self.podcast_request.load(Ordering::SeqCst) {
            return;
        }
        match result {
            Ok(shows) => {
                let status = if shows.is_empty() { String::new() } else { plural(shows.len(), "show", "shows") };
                self.podcast_results = shows;
                self.set_podcast_status(false, &status);
            }
            Err(e) => {
                log(format!("podcasts: search failed: {e}"));
                self.podcast_results.clear();
                self.set_podcast_status(false, &format!("Couldn't search: {e}"));
            }
        }
        self.show_podcast_results();
    }

    pub fn podcast_subscribe(&mut self, index: usize) {
        let Some(show) = self.podcast_results.get(index).cloned() else { return };
        if self.podcasts.iter().any(|p| p.show.feed_url == show.feed_url) {
            if let Some(i) = self.podcasts.iter().position(|p| p.show.feed_url == show.feed_url) {
                self.podcast_open(i);
            }
            return;
        }
        self.set_podcast_status(true, &format!("Subscribing to {}…", show.title));
        let (tx, db, covers) = (self.bg_tx.clone(), self.paths.database.clone(), self.paths.covers.clone());
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<String> {
                let feed = podcasts::fetch(&show.feed_url)?;
                let mut info = feed.show.clone();
                if info.image_url.is_none() {
                    info.image_url = show.image_url.clone();
                }
                let cover = info.image_url.as_deref().and_then(|url| save_podcast_cover(&covers, &info.feed_url, url));
                let lib = Library::open(&db)?;
                lib.subscribe_podcast(&info, cover.as_deref(), unix_now())?;
                lib.save_episodes(&info.feed_url, &feed.episodes)?;
                log(format!("podcasts: subscribed to {} ({} episodes)", info.title, feed.episodes.len()));
                Ok(info.title)
            })();
            let _ = tx.send(Bg::PodcastSubscribed(result.map_err(|e| format!("{e:#}"))));
        });
    }

    fn on_podcast_subscribed(&mut self, result: Result<String, String>) {
        match result {
            Ok(title) => {
                self.set_podcast_status(false, "");
                self.show_toast(&format!("Subscribed to {title}"));
            }
            Err(e) => {
                log(format!("podcasts: subscribe failed: {e}"));
                self.set_podcast_status(false, &format!("Couldn't subscribe: {e}"));
            }
        }
        self.refresh_podcasts();
    }

    pub fn podcast_open(&mut self, index: usize) {
        let Some(p) = self.podcasts.get(index).cloned() else { return };
        self.podcast_feed = Some(p.show.feed_url.clone());
        let (image, has_image) = self.cover(&p.cover);
        if let Some(ui) = self.ui() {
            ui.set_podcast_title(p.show.title.as_str().into());
            ui.set_podcast_author(p.show.author.as_str().into());
            ui.set_podcast_description(p.show.description.as_str().into());
            ui.set_podcast_image(image);
            ui.set_podcast_has_image(has_image);
            ui.set_podcast_favorite(p.favorite);
            ui.set_podcast_view(1);
        }
        self.set_podcast_status(false, "");
        self.reload_episodes();
        self.podcast_refresh();
    }

    /// Marks a show in the list as a favourite (or not); favourites are listed first.
    pub fn podcast_toggle_favorite(&mut self, index: usize) {
        let Some(p) = self.podcasts.get(index).cloned() else { return };
        self.set_podcast_favorite(&p.show.feed_url, !p.favorite);
    }

    /// Same, for the show that is open.
    pub fn podcast_toggle_show_favorite(&mut self) {
        let Some(feed) = self.podcast_feed.clone() else { return };
        let Some(p) = self.podcasts.iter().find(|p| p.show.feed_url == feed) else { return };
        let favorite = !p.favorite;
        self.set_podcast_favorite(&feed, favorite);
    }

    fn set_podcast_favorite(&mut self, feed: &str, favorite: bool) {
        if let Err(e) = self.library.set_podcast_favorite(feed, favorite) {
            return self.show_toast(&format!("Could not update favorites: {e}"));
        }
        self.refresh_podcasts();
        if self.podcast_feed.as_deref() == Some(feed) {
            if let Some(ui) = self.ui() {
                ui.set_podcast_favorite(favorite);
            }
        }
    }

    pub fn podcast_back(&mut self) {
        self.podcast_feed = None;
        if let Some(ui) = self.ui() {
            ui.set_podcast_view(0);
        }
        self.set_podcast_status(false, "");
        self.refresh_podcasts();
    }

    fn reload_episodes(&mut self) {
        let Some(feed) = self.podcast_feed.clone() else { return };
        self.podcast_episodes = self.library.episodes(&feed).unwrap_or_default();
        let items: Vec<EpisodeItem> = self.podcast_episodes.iter().map(|e| self.episode_item(&feed, e)).collect();
        if let Some(ui) = self.ui() {
            ui.set_podcast_episodes(model(items));
        }
    }

    fn episode_key(feed: &str, guid: &str) -> PathBuf {
        PathBuf::from(format!("podcast://{}", unitn_core::util::md5_hex(format!("{feed}\n{guid}").as_bytes())))
    }

    fn episode_item(&self, feed: &str, e: &EpisodeState) -> EpisodeItem {
        let ep = &e.episode;
        let mut parts = Vec::new();
        let date = podcasts::format_date(ep.published);
        if !date.is_empty() {
            parts.push(date);
        }
        if ep.duration > 0 {
            parts.push(long_duration(ep.duration * 1000));
        }
        if e.played {
            parts.push("Played".into());
        } else if e.position > 0.0 && ep.duration > 0 {
            let left = (ep.duration as f64 - e.position).max(0.0);
            parts.push(format!("{} left", long_duration((left * 1000.0) as u64)));
        }
        EpisodeItem {
            key: Self::episode_key(feed, &ep.guid).to_string_lossy().as_ref().into(),
            title: ep.title.as_str().into(),
            info: parts.join(" • ").into(),
            played: e.played,
            progress: if ep.duration > 0 { (e.position / ep.duration as f64) as f32 } else { 0.0 },
        }
    }

    /// Reloads the open show's feed.
    pub fn podcast_refresh(&mut self) {
        let Some(feed) = self.podcast_feed.clone() else { return };
        self.set_podcast_status(true, "");
        let (tx, db) = (self.bg_tx.clone(), self.paths.database.clone());
        thread::spawn(move || {
            let result = refresh_feed(&db, &feed);
            let _ = tx.send(Bg::PodcastRefreshed { feed, result: result.map_err(|e| format!("{e:#}")) });
        });
    }

    /// Checks every subscription for new episodes (at startup).
    fn refresh_all_podcasts(&self) {
        let feeds: Vec<String> = self.podcasts.iter().map(|p| p.show.feed_url.clone()).collect();
        if feeds.is_empty() {
            return;
        }
        let (tx, db) = (self.bg_tx.clone(), self.paths.database.clone());
        thread::spawn(move || {
            for feed in feeds {
                let result = refresh_feed(&db, &feed);
                let _ = tx.send(Bg::PodcastRefreshed { feed, result: result.map_err(|e| format!("{e:#}")) });
            }
        });
    }

    fn on_podcast_refreshed(&mut self, feed: String, result: Result<(), String>) {
        let open = self.podcast_feed.as_deref() == Some(feed.as_str());
        if let Err(e) = &result {
            log(format!("podcasts: refreshing {feed} failed: {e}"));
        }
        if open {
            match result {
                Ok(()) => self.set_podcast_status(false, ""),
                Err(e) => self.set_podcast_status(false, &format!("Couldn't refresh: {e}")),
            }
            self.reload_episodes();
        } else {
            self.refresh_podcasts();
        }
    }

    pub fn podcast_unsubscribe(&mut self) {
        let Some(feed) = self.podcast_feed.clone() else { return };
        let title = self.podcasts.iter().find(|p| p.show.feed_url == feed).map(|p| p.show.title.clone()).unwrap_or_default();
        if let Err(e) = self.library.unsubscribe_podcast(&feed) {
            return self.show_toast(&format!("Could not unsubscribe: {e}"));
        }
        self.show_toast(&format!("Unsubscribed from {title}"));
        self.podcast_back();
    }

    /// A podcast episode as something the player (and playlists) can take.
    fn episode_track(&mut self, feed: &str, state: &EpisodeState) -> Option<Track> {
        let show = self.podcasts.iter().find(|p| p.show.feed_url == feed)?;
        let ep = &state.episode;
        let key = Self::episode_key(feed, &ep.guid);
        let track = Track {
            path: key.clone(),
            title: ep.title.clone(),
            artist: show.show.title.clone(),
            album: show.show.title.clone(),
            album_artist: show.show.title.clone(),
            duration_ms: ep.duration * 1000,
            cover: show.cover.clone(),
            stream_url: Some(ep.audio_url.clone()),
            stream_ext: ep.extension(),
            ..Default::default()
        };
        self.episode_keys.insert(key, (feed.to_string(), ep.guid.clone()));
        Some(track)
    }

    pub fn podcast_play(&mut self, index: usize) {
        let Some(feed) = self.podcast_feed.clone() else { return };
        let Some(state) = self.podcast_episodes.get(index).cloned() else { return };
        let Some(track) = self.episode_track(&feed, &state) else { return };
        let key = track.path.clone();
        // Continue where it was left (unless it was finished).
        self.resume_at = (!state.played && state.position > 10.0).then(|| (key, state.position));
        self.episode_saved_at = if self.resume_at.is_some() { state.position } else { 0.0 };
        self.play_tracks(vec![track], 0, false);
    }

    /// Seeks to the saved position once a resumed episode is playing.
    fn resume_episode(&mut self) {
        let Some((key, position)) = self.resume_at.take() else { return };
        if matches!(self.current_item(), Some(Item::Track(t)) if t.path == key) {
            log(format!("podcasts: resuming at {position:.0} s"));
            self.player.seek(position);
            self.show_toast(&format!("Continuing at {}", format_time(position)));
        }
    }

    /// Remembers how far the current episode got (every 15 s and at the end).
    fn track_episode_progress(&mut self) {
        if self.resume_at.is_some() {
            return; // not at the saved position yet
        }
        let Some(Item::Track(t)) = self.current_item() else { return };
        let Some((feed, guid)) = self.episode_keys.get(&t.path).cloned() else { return };
        let duration = self.duration.unwrap_or(0.0);
        let played = duration > 60.0 && self.position >= duration - 30.0;
        if (self.position - self.episode_saved_at).abs() < 15.0 && !played {
            return;
        }
        if played && self.episode_saved_at >= duration - 30.0 {
            return; // already marked as played
        }
        self.episode_saved_at = self.position;
        if let Err(e) = self.library.set_episode_progress(&feed, &guid, self.position, played) {
            log(format!("podcasts: could not save progress: {e:#}"));
        }
        if self.podcast_feed.as_deref() == Some(feed.as_str()) {
            if let Some(i) = self.podcast_episodes.iter().position(|e| e.episode.guid == guid) {
                self.podcast_episodes[i].position = self.position;
                self.podcast_episodes[i].played = played;
                let item = self.episode_item(&feed, &self.podcast_episodes[i]);
                if let Some(ui) = self.ui() {
                    let m = ui.get_podcast_episodes();
                    if i < m.row_count() {
                        m.set_row_data(i, item);
                    }
                }
            }
        }
    }
}

/// Connects every UI callback to the application state.
pub fn connect(ui: &AppWindow, app: &Rc<RefCell<App>>) {
    // Helper: run `f` with mutable access to the app.
    fn with(app: &Rc<RefCell<App>>, f: impl FnOnce(&mut App)) {
        if let Ok(mut a) = app.try_borrow_mut() {
            f(&mut a);
        }
    }

    macro_rules! on {
        ($setter:ident, |$a:ident $(, $arg:ident)*| $body:expr) => {{
            let app = app.clone();
            ui.$setter(move |$($arg),*| with(&app, |$a| $body));
        }};
    }

    // Drag and drop inside the window (songs etc. onto playlists and the queue).
    let dnd = ui.global::<Dnd>();
    dnd.on_payload(|what| slint::DataTransfer::from(SharedString::from(format!("{DRAG_PREFIX}{what}"))));
    dnd.on_ours(|data| data.plain_text().is_ok_and(|t| t.starts_with(DRAG_PREFIX)));
    {
        let app = app.clone();
        dnd.on_dropped(move |data, target| {
            let Ok(text) = data.plain_text() else { return };
            // Later, so the drag has fully ended before lists are rebuilt.
            let app = app.clone();
            slint::Timer::single_shot(Duration::ZERO, move || with(&app, |a| a.drop_items(&text, target)));
        });
    }

    on!(on_open_album, |a, i| a.open_album(i.max(0) as usize));
    on!(on_open_artist, |a, i| a.open_artist(i.max(0) as usize));
    on!(on_show_all_albums, |a| a.show_all_albums());
    on!(on_play_album, |a, shuffle| a.play_album(shuffle));
    on!(on_play_album_track, |a, i| a.play_album_track(i.max(0) as usize));
    on!(on_play_song, |a, i| a.play_song(i.max(0) as usize));
    on!(on_rescan, |a| a.start_scan());
    on!(on_radio_select_tab, |a, t| a.load_radio_tab(t));
    on!(on_radio_genre, |a, g| a.radio_genre(g.max(0) as usize));
    on!(on_radio_open_picker, |a, k| a.radio_open_picker(k));
    on!(on_radio_picker_pick, |a, i| a.radio_picker_pick(i.max(0) as usize));
    on!(on_radio_picker_search, |a, t| a.radio_picker_search(t.as_str()));
    on!(on_add_custom_stream, |a, name, url| a.add_custom_stream(name.as_str(), url.as_str()));
    on!(on_play_station, |a, i| a.play_station(i.max(0) as usize));
    on!(on_toggle_favorite, |a, i| a.toggle_favorite(i.max(0) as usize));
    on!(on_play_queue_index, |a, i| a.play_queue_index(i.max(0) as usize));
    on!(on_remove_queue_index, |a, i| a.remove_queue_index(i.max(0) as usize));
    on!(on_clear_queue, |a| a.clear_queue());
    on!(on_move_queue_index, |a, from, to| a.move_queue_item(from.max(0) as usize, to.max(0) as usize));
    on!(on_move_playlist_track, |a, from, to| a.move_playlist_track(from.max(0) as usize, to.max(0) as usize));
    on!(on_toggle_play, |a| a.toggle_play());
    on!(on_next, |a| a.next(false));
    on!(on_previous, |a| a.previous());
    on!(on_seek, |a, f| a.seek_fraction(f));
    on!(on_seek_relative, |a, d| a.seek_relative(d as f64));
    on!(on_volume_changed, |a, v| a.set_volume(v));
    on!(on_toggle_shuffle, |a| a.toggle_shuffle());
    on!(on_cycle_repeat, |a| a.cycle_repeat());
    on!(on_theme_selected, |a, m| a.set_theme(m));
    on!(on_opacity_changed, |a, v| a.set_opacity(v));
    on!(on_viz_style_changed, |a, s| a.set_viz_style(s));
    on!(on_density_changed, |a, c| a.set_compact_lists(c));
    on!(on_eq_toggle, |a, on| a.set_eq_enabled(on));
    on!(on_eq_preset_selected, |a, i| a.select_eq_preset(i));
    on!(on_eq_band_changed, |a, i, v| a.set_eq_band(i.max(0) as usize, v));
    on!(on_eq_preamp_changed, |a, v| a.set_eq_preamp(v));
    on!(on_eq_reset, |a| a.reset_eq());
    on!(on_glass_changed, |a, on| a.set_glass(on));
    on!(on_glass_keep, |a| a.keep_glass());
    on!(on_eq_curve_aspect, |a, aspect| a.set_eq_aspect(aspect));
    on!(on_eq_open_changed, |a, open| a.set_eq_open(open));
    on!(on_download_art_changed, |a, on| a.set_download_art(on));
    on!(on_open_playlist, |a, i| a.open_playlist(i.max(0) as usize));
    on!(on_new_playlist, |a| a.new_playlist());
    on!(on_rename_playlist, |a, name| a.rename_playlist(name.as_str()));
    on!(on_delete_playlist, |a| a.delete_playlist());
    on!(on_play_playlist, |a, shuffle| a.play_playlist(shuffle));
    on!(on_play_playlist_track, |a, i| a.play_playlist_track(i.max(0) as usize));
    on!(on_save_queue_as_playlist, |a| a.save_queue_as_playlist());
    on!(on_toggle_like, |a| a.toggle_like());
    on!(on_toggle_mini_player, |a| a.toggle_mini_player());
    on!(on_toggle_mini_lyrics, |a| a.toggle_mini_lyrics());
    on!(on_set_sort, |a, page, index| a.set_sort(page, index.max(0) as usize));
    on!(on_sort_ignore_articles_changed, |a, on| a.set_sort_ignore_articles(on));
    on!(on_page_changed, |a, page| a.page_changed(page));
    on!(on_stats_period_changed, |a, p| a.set_stats_period(p));
    on!(on_album_range, |a, first, last| a.set_album_range(first, last));
    on!(on_artist_range, |a, first, last| a.set_artist_range(first, last));
    on!(on_flip_sort, |a, page| a.flip_sort(page));
    on!(on_track_menu_requested, |a, c, i| a.track_menu_requested(c, i));
    on!(on_track_menu_action, |a, action, pl| a.track_menu_action(action, pl));
    on!(on_open_server, |a, i| a.open_server(i.max(0) as usize));
    on!(on_refresh_server, |a| a.refresh_server());
    on!(on_open_server_album, |a, i| a.open_server_album(i.max(0) as usize));
    on!(on_remove_server, |a, i| a.remove_server(i.max(0) as usize));
    on!(on_connect_server, |a, kind, url, user, secret| a.connect_server(kind, url.to_string(), user.to_string(), secret.to_string()));
    on!(on_close_server_dialog, |a| a.close_server_dialog());
    on!(on_crossfade_changed, |a, secs| a.set_crossfade(secs));
    on!(on_gapless_albums_changed, |a, on| a.set_gapless_albums(on));
    on!(on_lyrics_online_changed, |a, on| a.set_lyrics_online(on));
    on!(on_startup_sound_changed, |a, on| a.set_startup_sound(on));
    on!(on_resume_session_changed, |a, on| a.set_resume_session(on));
    on!(on_save_lyrics, |a, kind| a.save_lyrics(kind, false));
    on!(on_lyrics_autosave_changed, |a, v| a.set_lyrics_autosave(v));
    on!(on_lyrics_open_changed, |a, open| a.set_lyrics_open(open));
    on!(on_podcast_subscribe, |a, i| a.podcast_subscribe(i.max(0) as usize));
    on!(on_podcast_open, |a, i| a.podcast_open(i.max(0) as usize));
    on!(on_podcast_back, |a| a.podcast_back());
    on!(on_podcast_refresh, |a| a.podcast_refresh());
    on!(on_podcast_unsubscribe, |a| a.podcast_unsubscribe());
    on!(on_podcast_play, |a, i| a.podcast_play(i.max(0) as usize));
    on!(on_podcast_toggle_favorite, |a, i| a.podcast_toggle_favorite(i.max(0) as usize));
    on!(on_podcast_toggle_show_favorite, |a| a.podcast_toggle_show_favorite());
    on!(on_fix_search, |a, artist, album| a.fix_search(artist.to_string(), album.to_string()));
    on!(on_fix_select, |a, i| a.fix_select(i.max(0) as usize));
    on!(on_fix_save, |a, artist, album| a.fix_save(artist.to_string(), album.to_string()));
    on!(on_fix_cancel, |a| a.close_fix_dialog());
    {
        let app = app.clone();
        ui.on_fix_choose_image(move || {
            let file = rfd::FileDialog::new()
                .set_title("Choose a cover image")
                .add_filter("Images", &["jpg", "jpeg", "png"])
                .pick_file();
            if let Some(file) = file {
                with(&app, |a| a.fix_set_local_image(file));
            }
        });
    }

    // File dialogs run before borrowing the app state (see add_folder).
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_export_playlist(move || {
            let name = weak.upgrade().map(|u| u.get_playlist_name().to_string()).unwrap_or_default();
            let file = rfd::FileDialog::new()
                .set_title("Export playlist")
                .set_file_name(format!("{}.m3u", if name.is_empty() { "Playlist" } else { &name }))
                .add_filter("M3U playlist", &["m3u", "m3u8"])
                .save_file();
            if let Some(file) = file {
                with(&app, |a| a.export_playlist(file));
            }
        });
    }
    {
        let app = app.clone();
        ui.on_import_playlist(move || {
            let file = rfd::FileDialog::new()
                .set_title("Import playlist")
                .add_filter("M3U playlist", &["m3u", "m3u8"])
                .pick_file();
            if let Some(file) = file {
                with(&app, |a| a.import_playlist(file));
            }
        });
    }
    on!(on_remove_folder, |a, i| a.remove_folder(i.max(0) as usize));

    // The folder dialog runs a nested event loop on some platforms, so open
    // it before borrowing the app state.
    {
        let app = app.clone();
        ui.on_add_folder(move || {
            if let Some(folder) = rfd::FileDialog::new().set_title("Add music folder").pick_folder() {
                with(&app, |a| a.add_folder(folder));
            }
        });
    }

    // Debounced searches: wait until typing pauses.
    {
        let app = app.clone();
        let timer = Rc::new(Timer::default());
        ui.on_library_search(move |text| {
            let app = app.clone();
            timer.start(TimerMode::SingleShot, Duration::from_millis(200), move || {
                with(&app, |a| a.search_library(text.as_str()));
            });
        });
    }
    {
        let app = app.clone();
        let timer = Rc::new(Timer::default());
        ui.on_podcast_search(move |text| {
            let app = app.clone();
            timer.start(TimerMode::SingleShot, Duration::from_millis(500), move || {
                with(&app, |a| a.podcast_search(text.as_str()));
            });
        });
    }
    {
        let app = app.clone();
        let timer = Rc::new(Timer::default());
        ui.on_radio_search(move |text| {
            let app = app.clone();
            timer.start(TimerMode::SingleShot, Duration::from_millis(400), move || {
                with(&app, |a| a.radio_search(text.as_str()));
            });
        });
    }
}

/// Opens a web page in the default browser. Returns false if that failed.
fn open_in_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.spawn() {
        Ok(_) => true,
        Err(e) => {
            log(format!("could not open the browser: {e}"));
            false
        }
    }
}

/// 12345 → "12,345".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Seconds as "3 d 4 h", "5 h 12 min", "12 min" or "40 s".
fn long_time(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{secs} s")
    }
}

/// "2026-11-25" → "Nov 25".
fn short_date(day: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = day.get(5..7).and_then(|m| m.parse::<usize>().ok()).and_then(|m| MONTHS.get(m.wrapping_sub(1)));
    let day_no = day.get(8..10).map(|d| d.trim_start_matches('0')).unwrap_or("");
    match month {
        Some(m) => format!("{m} {day_no}"),
        None => day.to_string(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Cache key for a song's lyrics.
fn lyrics_key(t: &Track) -> String {
    format!("{}\u{1f}{}\u{1f}{}", t.artist.to_lowercase(), t.title.to_lowercase(), t.duration_ms / 1000)
}

/// Looks for lyrics: next to the file or in its tags, then the cache, then
/// LRCLIB. Runs on a worker thread. Returns the lyrics and whether an online
/// lookup failed.
fn find_lyrics(track: &Track, db: &Path, online: bool) -> (Option<Lyrics>, bool) {
    if !track.is_remote() {
        if let Some(l) = lyrics::local(&track.path) {
            return (Some(l), false);
        }
    }
    let key = lyrics_key(track);
    let lib = Library::open(db).ok();
    if let Some(lib) = &lib {
        match lib.cached_lyrics(&key, unix_now()) {
            Ok(Some(Some((_, source)))) if source == "instrumental" => return (Some(Lyrics::instrumental("LRCLIB")), false),
            Ok(Some(Some((body, source)))) => return (lyrics::parse(&body, &source), false),
            Ok(Some(None)) => return (None, false),
            _ => {}
        }
    }
    if !online {
        return (None, false);
    }
    match lyrics::lrclib(&track.artist, &track.title, &track.album, track.duration_secs()) {
        Ok(found) => {
            if let Some(lib) = &lib {
                let _ = match &found {
                    Some(l) if l.instrumental => lib.cache_lyrics(&key, Some(""), "instrumental", unix_now()),
                    Some(l) => lib.cache_lyrics(&key, Some(&l.to_text()), &l.source, unix_now()),
                    None => lib.cache_lyrics(&key, None, "", unix_now()),
                };
            }
            log(format!("lyrics: LRCLIB {} \"{} - {}\"", if found.is_some() { "found" } else { "has nothing for" }, track.artist, track.title));
            (found, false)
        }
        Err(e) => {
            log(format!("lyrics: LRCLIB lookup failed: {e:#}"));
            (None, true)
        }
    }
}

/// Downloads a show's artwork into the covers folder.
fn save_podcast_cover(covers: &Path, feed: &str, url: &str) -> Option<PathBuf> {
    let data = servers::fetch_image(url).map_err(|e| log(format!("podcasts: artwork failed: {e:#}"))).ok()?;
    let ext = if data.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
    std::fs::create_dir_all(covers).ok()?;
    let path = covers.join(format!("podcast-{}.{ext}", unitn_core::util::md5_hex(feed.as_bytes())));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// Reloads a feed and stores its episodes (worker thread).
fn refresh_feed(db: &Path, feed_url: &str) -> anyhow::Result<()> {
    let feed = podcasts::fetch(feed_url)?;
    let lib = Library::open(db)?;
    lib.save_episodes(feed_url, &feed.episodes)?;
    Ok(())
}
