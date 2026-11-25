//! Listening statistics: every song listened to long enough is written to
//! the `plays` table with its tags, so the numbers stay right after a song
//! is renamed, removed or came from a music server.

use anyhow::Result;
use rusqlite::params;

use super::Library;
use crate::model::Track;

/// A song counts as played once this much of it was heard, or half of it
/// for songs shorter than a minute.
pub const COUNT_AFTER_SECS: f64 = 30.0;

/// Seconds of a song that must be heard for it to count as played.
pub fn count_threshold(duration_secs: f64) -> f64 {
    if duration_secs > 0.0 {
        COUNT_AFTER_SECS.min(duration_secs / 2.0)
    } else {
        COUNT_AFTER_SECS
    }
}

/// One row of a top list.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// Song title, artist, album or genre.
    pub name: String,
    /// The artist for songs and albums, else empty.
    pub detail: String,
    pub plays: u64,
    pub listened_secs: u64,
}

/// Plays over a period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayStats {
    pub plays: u64,
    pub listened_secs: u64,
    pub songs: Vec<Ranked>,
    pub artists: Vec<Ranked>,
    pub albums: Vec<Ranked>,
    pub genres: Vec<Ranked>,
}

/// What's in the library.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryTotals {
    pub songs: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub duration_secs: u64,
}

const ARTIST: &str = "(CASE WHEN artist <> '' THEN artist ELSE 'Unknown artist' END)";

impl Library {
    /// Writes down that `track` was listened to for `listened_secs`, at `at`
    /// (Unix time, seconds).
    pub fn record_play(&self, track: &Track, listened_secs: f64, at: i64) -> Result<()> {
        let album_artist = if track.album_artist.is_empty() { &track.artist } else { &track.album_artist };
        self.conn.execute(
            "INSERT INTO plays (at, listened_ms, path, title, artist, album, album_artist, genre)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                at,
                (listened_secs.max(0.0) * 1000.0) as i64,
                track.path.to_string_lossy(),
                track.title,
                track.artist,
                track.album,
                album_artist,
                track.genre.split([';', '\0']).map(str::trim).find(|g| !g.is_empty()).unwrap_or(""),
            ],
        )?;
        Ok(())
    }

    /// Number of plays recorded so far.
    pub fn play_count(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Plays since `since` (Unix time; 0 = all time), with the `top` most
    /// played songs, artists, albums and genres.
    pub fn play_stats(&self, since: i64, top: usize) -> Result<PlayStats> {
        let (plays, ms): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(listened_ms), 0) FROM plays WHERE at >= ?1",
            params![since],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let ranked = |name: &str, detail: &str, group: &str, filter: &str| -> Result<Vec<Ranked>> {
            let sql = format!(
                "SELECT {name}, {detail}, COUNT(*) AS n, COALESCE(SUM(listened_ms), 0) AS ms FROM plays
                 WHERE at >= ?1 {filter} GROUP BY {group} ORDER BY n DESC, ms DESC, {name} LIMIT ?2"
            );
            let mut st = self.conn.prepare(&sql)?;
            let rows = st.query_map(params![since, top as i64], |r| {
                Ok(Ranked {
                    name: r.get(0)?,
                    detail: r.get(1)?,
                    plays: r.get::<_, i64>(2)? as u64,
                    listened_secs: (r.get::<_, i64>(3)? / 1000) as u64,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        };
        Ok(PlayStats {
            plays: plays as u64,
            listened_secs: (ms / 1000) as u64,
            songs: ranked("MAX(title)", &format!("MAX({ARTIST})"), "lower(title), lower(artist)", "AND title <> ''")?,
            artists: ranked(&format!("MAX({ARTIST})"), "''", "lower(artist)", "")?,
            albums: ranked("MAX(album)", "MAX(album_artist)", "lower(album), lower(album_artist)", "AND album <> ''")?,
            genres: ranked("MAX(genre)", "''", "lower(genre)", "AND genre <> ''")?,
        })
    }

    /// Seconds listened per day for the last `days` days (oldest first,
    /// today last), by the computer's time zone. `now` is Unix time.
    pub fn listening_per_day(&self, days: u32, now: i64) -> Result<Vec<(String, u64)>> {
        let mut st = self.conn.prepare(
            "SELECT date(at, 'unixepoch', 'localtime') AS d, SUM(listened_ms) FROM plays
             WHERE date(at, 'unixepoch', 'localtime') >= date(?1, 'unixepoch', 'localtime', ?2) GROUP BY d",
        )?;
        let back = format!("-{} days", days.saturating_sub(1));
        let by_day: std::collections::HashMap<String, i64> = st
            .query_map(params![now, back], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::with_capacity(days as usize);
        for i in (0..days).rev() {
            let day: String = self.conn.query_row(
                "SELECT date(?1, 'unixepoch', 'localtime', ?2)",
                params![now, format!("-{i} days")],
                |r| r.get(0),
            )?;
            let ms = by_day.get(&day).copied().unwrap_or(0);
            out.push((day, (ms / 1000) as u64));
        }
        Ok(out)
    }

    /// Songs, albums, artists, genres and total length of the library.
    pub fn library_totals(&self) -> Result<LibraryTotals> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT COUNT(*),
                        COUNT(DISTINCT CASE WHEN album <> '' THEN lower({group}) || char(1) || lower(album) END),
                        COUNT(DISTINCT CASE WHEN artist <> '' THEN lower(artist) END),
                        COUNT(DISTINCT CASE WHEN genre <> '' THEN lower(genre) END),
                        COALESCE(SUM(duration_ms), 0) / 1000
                 FROM tracks",
                group = super::GROUP_ARTIST
            ),
            [],
            |r| {
                Ok(LibraryTotals {
                    songs: r.get::<_, i64>(0)? as u64,
                    albums: r.get::<_, i64>(1)? as u64,
                    artists: r.get::<_, i64>(2)? as u64,
                    genres: r.get::<_, i64>(3)? as u64,
                    duration_secs: r.get::<_, i64>(4)? as u64,
                })
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn song(title: &str, artist: &str, album: &str, genre: &str) -> Track {
        Track {
            path: PathBuf::from(format!("/m/{title}.mp3")),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            genre: genre.into(),
            duration_ms: 200_000,
            ..Default::default()
        }
    }

    #[test]
    fn counts_after_thirty_seconds_or_half() {
        assert_eq!(count_threshold(200.0), 30.0);
        assert_eq!(count_threshold(40.0), 20.0);
        assert_eq!(count_threshold(0.0), 30.0);
    }

    #[test]
    fn ranks_plays() {
        let lib = Library::open_in_memory().unwrap();
        let a = song("One", "Band", "First", "Rock");
        let b = song("Two", "Band", "First", "Rock");
        let c = song("Three", "Singer", "Solo", "Jazz");
        lib.record_play(&a, 100.0, 1000).unwrap();
        lib.record_play(&a, 100.0, 2000).unwrap();
        lib.record_play(&b, 60.0, 3000).unwrap();
        lib.record_play(&c, 30.0, 10).unwrap();
        let all = lib.play_stats(0, 10).unwrap();
        assert_eq!(all.plays, 4);
        assert_eq!(all.listened_secs, 290);
        assert_eq!(all.songs[0].name, "One");
        assert_eq!(all.songs[0].plays, 2);
        assert_eq!(all.songs[0].detail, "Band");
        assert_eq!(all.artists[0].name, "Band");
        assert_eq!(all.artists[0].plays, 3);
        assert_eq!(all.albums[0].name, "First");
        assert_eq!(all.genres[0].name, "Rock");
        // Only what was played since then.
        let recent = lib.play_stats(500, 10).unwrap();
        assert_eq!(recent.plays, 3);
        assert!(recent.artists.iter().all(|r| r.name != "Singer"));
        assert_eq!(lib.play_count().unwrap(), 4);
    }

    #[test]
    fn listening_per_day_ends_today() {
        let lib = Library::open_in_memory().unwrap();
        let now = 1_700_000_000;
        lib.record_play(&song("A", "B", "C", ""), 120.0, now).unwrap();
        let days = lib.listening_per_day(7, now).unwrap();
        assert_eq!(days.len(), 7);
        assert_eq!(days[6].1, 120);
        assert!(days[..6].iter().all(|(_, s)| *s == 0));
    }

    #[test]
    fn totals_the_library() {
        let lib = Library::open_in_memory().unwrap();
        lib.upsert_track(&song("One", "Band", "First", "Rock"), 1).unwrap();
        lib.upsert_track(&song("Two", "Band", "First", "Rock"), 1).unwrap();
        lib.upsert_track(&song("Three", "Singer", "Solo", "Jazz"), 1).unwrap();
        let t = lib.library_totals().unwrap();
        assert_eq!((t.songs, t.albums, t.artists, t.genres, t.duration_secs), (3, 2, 2, 2, 600));
    }
}
