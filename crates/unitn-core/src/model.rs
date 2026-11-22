use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A local audio file with its tags.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: u32,
    pub genre: String,
    pub duration_ms: u64,
    pub cover: Option<PathBuf>,
    /// For songs on a music server: where to stream them from. `path` then
    /// holds a stable key like `jellyfin://server/item`.
    pub stream_url: Option<String>,
    /// File type of the stream (e.g. "flac"), if known.
    pub stream_ext: Option<String>,
}

impl Track {
    /// What to give the player to play this song.
    pub fn source(&self) -> crate::audio::Source {
        match &self.stream_url {
            Some(url) => crate::audio::Source::Remote { url: url.clone(), ext: self.stream_ext.clone() },
            None => crate::audio::Source::File(self.path.clone()),
        }
    }

    /// True for songs streamed from a music server.
    pub fn is_remote(&self) -> bool {
        self.stream_url.is_some()
    }

    /// The artist an album is grouped under (album artist if present).
    pub fn grouping_artist(&self) -> &str {
        if self.album_artist.is_empty() {
            &self.artist
        } else {
            &self.album_artist
        }
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_ms as f64 / 1000.0
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Album {
    pub title: String,
    pub artist: String,
    pub year: u32,
    pub track_count: u32,
    pub duration_ms: u64,
    pub cover: Option<PathBuf>,
    /// When the album's newest song was added to the library (Unix seconds).
    pub added: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Artist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
    /// When the artist's newest song was added to the library (Unix seconds).
    pub added: i64,
    /// One of the artist's album covers, for when there is no artist photo.
    pub cover: Option<PathBuf>,
    /// One of the artist's songs, to look for an `artist.jpg` next to it.
    pub sample: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub track_count: u32,
}

/// An internet radio station (from radio-browser.info or a favorite).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub uuid: String,
    pub name: String,
    pub url: String,
    pub favicon: String,
    pub country: String,
    pub tags: String,
    pub codec: String,
    pub bitrate: u32,
}

impl Station {
    /// True if one of the station's tags contains `tag` (ignoring case), the
    /// way Radio Browser matches a genre: "pop" also finds "pop rock".
    pub fn has_tag(&self, tag: &str) -> bool {
        let tag = tag.to_lowercase();
        self.tags.split(',').any(|t| t.trim().to_lowercase().contains(&tag))
    }

    /// Short human readable description, e.g. "Germany • jazz, swing • MP3 128 kbps".
    pub fn info(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.country.is_empty() {
            parts.push(self.country.clone());
        }
        let tags: Vec<&str> = self
            .tags
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .take(3)
            .collect();
        if !tags.is_empty() {
            parts.push(tags.join(", "));
        }
        let mut fmt = self.codec.to_uppercase();
        if self.bitrate > 0 {
            if !fmt.is_empty() {
                fmt.push(' ');
            }
            fmt.push_str(&format!("{} kbps", self.bitrate));
        }
        if !fmt.is_empty() {
            parts.push(fmt);
        }
        parts.join(" • ")
    }
}

/// The genre of an album, from its songs' genre tags: the most common one,
/// or the two most common joined with " / " when the album mixes genres.
/// Tags holding several genres ("Rock; Pop", "Rock\0Pop") count each one.
pub fn album_genre(tracks: &[Track]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for t in tracks {
        for g in t.genre.split([';', '\0']).map(str::trim).filter(|g| !g.is_empty()) {
            match counts.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(g)) {
                Some((_, n)) => *n += 1,
                None => counts.push((g.to_string(), 1)),
            }
        }
    }
    // Stable sort: ties keep the order of first appearance.
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.into_iter().take(2).map(|(g, _)| g).collect::<Vec<_>>().join(" / ")
}

#[cfg(test)]
mod album_genre_tests {
    use super::*;

    fn song(genre: &str) -> Track {
        Track { genre: genre.to_string(), ..Default::default() }
    }

    #[test]
    fn picks_the_albums_genres() {
        assert_eq!(album_genre(&[]), "");
        assert_eq!(album_genre(&[song(""), song("")]), "");
        assert_eq!(album_genre(&[song("Rock"), song("rock"), song("")]), "Rock");
        assert_eq!(album_genre(&[song("Jazz"), song("Soul"), song("Soul"), song("Funk")]), "Soul / Jazz");
        assert_eq!(album_genre(&[song("Rock; Pop"), song("Pop")]), "Pop / Rock");
    }
}
