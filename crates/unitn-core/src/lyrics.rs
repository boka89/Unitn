//! Song lyrics: from a `.lrc` / `.txt` file next to the song or in the album's
//! `Lyrics` subfolder, from the song's own
//! tags, or online from LRCLIB (lrclib.net, free and open). Synced (LRC)
//! lyrics carry a time per line, so the current line can be highlighted.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::util::url_encode;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Start time in seconds (synced lyrics only).
    pub time: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Every line has a time.
    pub synced: bool,
    /// The song has no words (LRCLIB marks these).
    pub instrumental: bool,
    /// Where they came from, for the UI ("LRCLIB", "Lyrics file", "Song tags").
    pub source: String,
}

impl Lyrics {
    pub fn instrumental(source: &str) -> Self {
        Self { lines: Vec::new(), synced: false, instrumental: true, source: source.into() }
    }

    /// Index of the line being sung at `position` seconds.
    pub fn current_line(&self, position: f64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let n = self.lines.partition_point(|l| l.time.unwrap_or(0.0) <= position);
        n.checked_sub(1)
    }

    /// The lyrics as text again (LRC for synced lyrics), for the cache.
    pub fn to_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| match l.time {
                Some(t) if self.synced => {
                    let cs = (t * 100.0).round().max(0.0) as u64;
                    format!("[{:02}:{:02}.{:02}]{}", cs / 6000, cs / 100 % 60, cs % 100, l.text)
                }
                _ => l.text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Parses LRC (`[mm:ss.xx]text`, several stamps per line, `[offset:±ms]`)
/// or plain text. Returns `None` for empty lyrics.
pub fn parse(text: &str, source: &str) -> Option<Lyrics> {
    let mut timed: Vec<Line> = Vec::new();
    let mut plain: Vec<Line> = Vec::new();
    let mut offset = 0.0;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let mut rest = line.trim_start();
        let mut stamps = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let tag = &body[..end];
            if let Some(t) = parse_stamp(tag) {
                stamps.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                // Positive offset: lyrics appear earlier.
                offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            } else if stamps.is_empty()
                && tag
                    .split_once(':')
                    .is_some_and(|(k, _)| !k.is_empty() && k.len() <= 8 && k.chars().all(|c| c.is_ascii_alphabetic()))
            {
                // ID tags like [ar:…], [ti:…]: not lyrics.
                rest = "";
                break;
            } else {
                break;
            }
            rest = &body[end + 1..];
        }
        let words = rest.trim().to_string();
        if stamps.is_empty() {
            if !(words.is_empty() && plain.is_empty()) {
                plain.push(Line { time: None, text: words });
            }
        } else {
            for t in stamps {
                timed.push(Line { time: Some(t), text: words.clone() });
            }
        }
    }
    if !timed.is_empty() {
        for l in &mut timed {
            l.time = l.time.map(|t| (t - offset).max(0.0));
        }
        timed.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        return Some(Lyrics { lines: timed, synced: true, instrumental: false, source: source.into() });
    }
    while plain.last().is_some_and(|l| l.text.is_empty()) {
        plain.pop();
    }
    (!plain.is_empty()).then(|| Lyrics { lines: plain, synced: false, instrumental: false, source: source.into() })
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss:xx` → seconds.
fn parse_stamp(tag: &str) -> Option<f64> {
    let (min, rest) = tag.split_once(':')?;
    let min: u64 = min.trim().parse().ok()?;
    let rest = rest.replacen(':', ".", 1);
    let secs: f64 = rest.trim().parse().ok()?;
    (secs < 60.0 && secs >= 0.0).then_some(min as f64 * 60.0 + secs)
}

/// Lyrics stored with a local song: a lyrics file named like the song
/// (`Song.lrc` or `Song.txt`), next to it or in a `Lyrics` subfolder of the
/// album folder (any capitalisation), or a lyrics tag inside the file.
/// `.lrc` files win over `.txt`, and a file next to the song over one in
/// the subfolder.
pub fn local(path: &Path) -> Option<Lyrics> {
    for file in lyrics_files(path) {
        let Ok(bytes) = std::fs::read(&file) else { continue };
        if let Some(l) = parse(&decode_text(&bytes), "Lyrics file") {
            return Some(l);
        }
    }
    use lofty::prelude::*;
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let text = tag.get_string(&lofty::tag::ItemKey::Lyrics)?;
    parse(text, "Song tags")
}

/// Lyrics files that may belong to the song at `path`, best first.
fn lyrics_files(path: &Path) -> Vec<std::path::PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { return Vec::new() };
    let mut dirs = vec![dir.to_path_buf()];
    // A "Lyrics" / "lyrics" subfolder; matched without regard to case, so it
    // also works on Linux, where file names are case-sensitive.
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut subs: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("lyrics"))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        dirs.extend(subs);
    }
    let mut files = Vec::new();
    for ext in ["lrc", "txt"] {
        for d in &dirs {
            let wanted = format!("{}.{ext}", stem.to_string_lossy());
            let exact = d.join(&wanted);
            if exact.is_file() {
                files.push(exact);
                continue;
            }
            // Same name with another capitalisation (Song.LRC, song.lrc).
            if let Ok(entries) = std::fs::read_dir(d) {
                if let Some(e) = entries.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&wanted)) {
                    files.push(e.path());
                }
            }
        }
    }
    files
}

/// Text of a lyrics file: UTF-8 (with or without a byte-order mark), UTF-16
/// with a byte-order mark (Windows Notepad's "Unicode"), or else Windows-1252
/// / Latin-1, which older lyrics files often use.
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// One Windows-1252 byte as a character (Latin-1, plus curly quotes, dashes,
/// € and friends in 0x80–0x9F).
fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Saves lyrics next to the song, named like it: `Song.lrc` for synced
/// lyrics, `Song.txt` for plain ones. Returns the file written.
pub fn save_sidecar(song: &Path, lyrics: &Lyrics) -> anyhow::Result<std::path::PathBuf> {
    let ext = if lyrics.synced { "lrc" } else { "txt" };
    let file = song.with_extension(ext);
    std::fs::write(&file, lyrics.to_text())?;
    Ok(file)
}

/// Writes lyrics into the song file's own tags (synced lyrics as LRC text,
/// as most players expect), creating a tag if the file has none.
pub fn save_to_tags(song: &Path, lyrics: &Lyrics) -> anyhow::Result<()> {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::Tag;
    let mut tagged = lofty::read_from_path(song)?;
    if tagged.primary_tag().is_none() {
        let kind = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(kind));
    }
    let tag = tagged.primary_tag_mut().ok_or_else(|| anyhow::anyhow!("this file type can't hold tags"))?;
    tag.insert_text(ItemKey::Lyrics, lyrics.to_text());
    tag.save_to_path(song, WriteOptions::default())?;
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(crate::USER_AGENT)
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
}

/// Looks the song up on LRCLIB. `Ok(None)`: LRCLIB doesn't know it.
pub fn lrclib(artist: &str, title: &str, album: &str, duration_secs: f64) -> Result<Option<Lyrics>> {
    if artist.trim().is_empty() || title.trim().is_empty() {
        return Ok(None);
    }
    let mut url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}",
        url_encode(artist.trim()),
        url_encode(title.trim())
    );
    if !album.trim().is_empty() {
        url.push_str(&format!("&album_name={}", url_encode(album.trim())));
    }
    if duration_secs > 0.0 {
        url.push_str(&format!("&duration={}", duration_secs.round() as u64));
    }
    match agent().get(&url).call() {
        Ok(resp) => {
            let json: Value = resp.into_json()?;
            return Ok(from_lrclib(&json));
        }
        Err(ureq::Error::Status(404, _)) => {}
        Err(e) => return Err(e.into()),
    }
    // Not found exactly (album or length differ): search and take the best match.
    let url = format!(
        "https://lrclib.net/api/search?artist_name={}&track_name={}",
        url_encode(artist.trim()),
        url_encode(title.trim())
    );
    let json: Value = agent().get(&url).call()?.into_json()?;
    Ok(best_match(&json, duration_secs).and_then(from_lrclib))
}

/// From search results: closest length (within 10 s when known), synced first.
pub(crate) fn best_match(results: &Value, duration_secs: f64) -> Option<&Value> {
    let list = results.as_array()?;
    list.iter()
        .filter(|r| {
            duration_secs <= 0.0
                || r["duration"].as_f64().map(|d| (d - duration_secs).abs() <= 10.0).unwrap_or(true)
        })
        .min_by_key(|r| {
            let synced = r["syncedLyrics"].as_str().is_some_and(|s| !s.is_empty());
            let diff = r["duration"].as_f64().map(|d| (d - duration_secs).abs()).unwrap_or(30.0);
            (!synced, (diff * 10.0) as i64)
        })
}

pub(crate) fn from_lrclib(json: &Value) -> Option<Lyrics> {
    if let Some(synced) = json["syncedLyrics"].as_str().and_then(|t| parse(t, "LRCLIB")) {
        return Some(synced);
    }
    if let Some(plain) = json["plainLyrics"].as_str().and_then(|t| parse(t, "LRCLIB")) {
        return Some(plain);
    }
    json["instrumental"].as_bool().filter(|i| *i).map(|_| Lyrics::instrumental("LRCLIB"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synced_lrc() {
        let text = "[ar:Band]\n[ti:Song]\n[offset:+500]\n[00:01.00]First line\n[00:05.50][01:05.50]Chorus\n[00:03.20]Second\n\n";
        let l = parse(text, "test").unwrap();
        assert!(l.synced);
        let times: Vec<f64> = l.lines.iter().map(|x| x.time.unwrap()).collect();
        assert_eq!(times, vec![0.5, 2.7, 5.0, 65.0]);
        assert_eq!(l.lines[1].text, "Second");
        assert_eq!(l.lines[3].text, "Chorus");
        assert_eq!(l.current_line(0.0), None);
        assert_eq!(l.current_line(3.0), Some(1));
        assert_eq!(l.current_line(100.0), Some(3));
        // Round trip through the cache format.
        assert_eq!(parse(&l.to_text(), "cache").unwrap().lines, l.lines);
    }

    #[test]
    fn parses_plain_lyrics() {
        let l = parse("\nHello\n\nWorld\n\n", "test").unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["Hello", "", "World"]);
        assert_eq!(l.current_line(10.0), None);
        assert!(parse("  \n\n", "test").is_none());
        assert_eq!(parse_stamp("02:03:50"), Some(123.5));
        assert_eq!(parse_stamp("ar:x"), None);
    }

    #[test]
    fn picks_lrclib_results() {
        let json: Value = serde_json::from_str(
            r#"[{"duration":200,"plainLyrics":"plain far"},
                {"duration":181,"plainLyrics":"plain close"},
                {"duration":185,"syncedLyrics":"[00:01.00]synced"},
                {"duration":400,"syncedLyrics":"[00:01.00]wrong song"}]"#,
        )
        .unwrap();
        let best = from_lrclib(best_match(&json, 180.0).unwrap()).unwrap();
        assert!(best.synced);
        assert_eq!(best.lines[0].text, "synced");
        let inst: Value = serde_json::from_str(r#"{"instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#).unwrap();
        assert!(from_lrclib(&inst).unwrap().instrumental);
    }

    #[test]
    fn reads_lrc_next_to_the_song() {
        let dir = std::env::temp_dir().join(format!("unitn-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("Song.mp3");
        std::fs::write(&song, b"not really audio").unwrap();
        std::fs::write(dir.join("Song.lrc"), "[00:10.00]Hi").unwrap();
        let l = local(&song).unwrap();
        assert_eq!(l.source, "Lyrics file");
        assert_eq!(l.lines[0].time, Some(10.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_lyrics_from_a_lyrics_subfolder() {
        let dir = std::env::temp_dir().join(format!("unitn-lyrics-sub-{}", std::process::id()));
        let sub = dir.join("Lyrics");
        std::fs::create_dir_all(&sub).unwrap();
        let (a, b, c) = (dir.join("01 One.flac"), dir.join("02 Two.flac"), dir.join("03 Three.flac"));
        for song in [&a, &b, &c] {
            std::fs::write(song, b"not really audio").unwrap();
        }
        std::fs::write(sub.join("01 One.lrc"), "[00:01.00]one").unwrap();
        // Plain text in Windows-1252 ("café"), and a capitalised extension.
        std::fs::write(sub.join("02 Two.TXT"), b"caf\xe9\nsecond line").unwrap();
        // A .lrc next to the song wins over the subfolder.
        std::fs::write(sub.join("03 Three.lrc"), "[00:01.00]from subfolder").unwrap();
        std::fs::write(dir.join("03 Three.lrc"), "[00:01.00]next to song").unwrap();

        let one = local(&a).unwrap();
        assert!(one.synced);
        assert_eq!(one.lines[0].text, "one");
        let two = local(&b).unwrap();
        assert!(!two.synced);
        assert_eq!(two.lines[0].text, "café");
        assert_eq!(local(&c).unwrap().lines[0].text, "next to song");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saves_lyrics_next_to_the_song() {
        let dir = std::env::temp_dir().join(format!("unitn-lyrics-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("01 Song.flac");
        std::fs::write(&song, b"not really audio").unwrap();
        let synced = parse("[00:01.50]one\n[00:03.00]two", "LRCLIB").unwrap();
        assert_eq!(save_sidecar(&song, &synced).unwrap(), dir.join("01 Song.lrc"));
        // It is found again, as the same lyrics.
        let again = local(&song).unwrap();
        assert!(again.synced);
        assert_eq!(again.lines, synced.lines);
        let plain = parse("just words", "LRCLIB").unwrap();
        assert_eq!(save_sidecar(&song, &plain).unwrap(), dir.join("01 Song.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decodes_lyrics_text() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode_text("héllo".as_bytes()), "héllo");
        assert_eq!(decode_text(b"h\xe9llo"), "héllo");
        assert_eq!(decode_text(b"\x93hi\x94 \x96 ok"), "“hi” – ok");
    }
}
