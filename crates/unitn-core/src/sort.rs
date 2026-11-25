//! Sort orders for the Albums, Artists and Songs pages.
//!
//! Each page has a list of orders (`*_ORDERS`: the label shown in the menu
//! and whether it starts newest / biggest first) and a function that sorts
//! by order index plus a "reversed" flag. Names compare without case and,
//! when `ignore_articles` is on (the default setting), without a leading
//! "The", "A" or "An", as most music players do, so The Beatles sort under B.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::model::{Album, Artist, Track};

/// A sort order: its menu label, and whether it runs high-to-low by default.
pub struct Order {
    pub label: &'static str,
    pub descending: bool,
}

const fn order(label: &'static str, descending: bool) -> Order {
    Order { label, descending }
}

pub const ALBUM_ORDERS: &[Order] = &[
    order("Artist", false),
    order("Title", false),
    order("Year", true),
    order("Recently added", true),
];

pub const ARTIST_ORDERS: &[Order] = &[
    order("Name", false),
    order("Most albums", true),
    order("Most songs", true),
    order("Recently added", true),
];

pub const SONG_ORDERS: &[Order] = &[
    order("Artist", false),
    order("Title", false),
    order("Album", false),
    order("Year", true),
    order("Length", false),
    order("Genre", false),
    order("Recently added", true),
];

/// Text as it is compared when sorting: lower case and, with
/// `ignore_articles`, without a leading article ("The Beatles" → "beatles").
pub fn sort_key(s: &str, ignore_articles: bool) -> String {
    let lower = s.trim().to_lowercase();
    if !ignore_articles {
        return lower;
    }
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            let rest = rest.trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    lower
}

fn by_name(a: &str, b: &str, ignore_articles: bool) -> Ordering {
    sort_key(a, ignore_articles).cmp(&sort_key(b, ignore_articles)).then_with(|| a.cmp(b))
}

/// `primary` in the chosen direction, then the tie-breakers (always A–Z).
fn directed(primary: Ordering, descending: bool, then: impl FnOnce() -> Ordering) -> Ordering {
    let p = if descending { primary.reverse() } else { primary };
    p.then_with(then)
}

/// Whether `order` should run high-to-low: its default, flipped by `reversed`.
fn descending(orders: &[Order], index: usize, reversed: bool) -> bool {
    orders.get(index).map(|o| o.descending).unwrap_or(false) != reversed
}

pub fn sort_albums(albums: &mut [Album], index: usize, reversed: bool, ignore_articles: bool) {
    sort_albums_by(albums, |a| a, index, reversed, ignore_articles);
}

/// Like [`sort_albums`], for a list of things that each have an album (so
/// what belongs to an album stays with it).
pub fn sort_albums_by<T>(items: &mut [T], album: impl Fn(&T) -> &Album, index: usize, reversed: bool, ignore_articles: bool) {
    let cmp_name = |a: &str, b: &str| by_name(a, b, ignore_articles);
    let desc = descending(ALBUM_ORDERS, index, reversed);
    // Within one artist, albums go oldest to newest (by year).
    let artist_then_year = |a: &Album, b: &Album| {
        cmp_name(&a.artist, &b.artist).then(a.year.cmp(&b.year)).then_with(|| cmp_name(&a.title, &b.title))
    };
    items.sort_by(|a, b| {
        let (a, b) = (album(a), album(b));
        match index {
        1 => directed(cmp_name(&a.title, &b.title), desc, || cmp_name(&a.artist, &b.artist)),
        // Albums without a year go last either way.
        2 => (a.year == 0)
            .cmp(&(b.year == 0))
            .then_with(|| directed(a.year.cmp(&b.year), desc, || artist_then_year(a, b))),
        3 => directed(a.added.cmp(&b.added), desc, || artist_then_year(a, b)),
        _ => {
            let p = cmp_name(&a.artist, &b.artist);
            (if desc { p.reverse() } else { p }).then(a.year.cmp(&b.year)).then_with(|| cmp_name(&a.title, &b.title))
        }
        }
    });
}

pub fn sort_artists(artists: &mut [Artist], index: usize, reversed: bool, ignore_articles: bool) {
    let cmp_name = |a: &str, b: &str| by_name(a, b, ignore_articles);
    let desc = descending(ARTIST_ORDERS, index, reversed);
    artists.sort_by(|a, b| {
        let name = || cmp_name(&a.name, &b.name);
        match index {
            1 => directed(a.album_count.cmp(&b.album_count), desc, name),
            2 => directed(a.track_count.cmp(&b.track_count), desc, name),
            3 => directed(a.added.cmp(&b.added), desc, name),
            _ => directed(name(), desc, || Ordering::Equal),
        }
    });
}

/// `added`: when each song was added, by song id (only used for
/// "Recently added").
pub fn sort_songs(songs: &mut [Track], index: usize, reversed: bool, added: &HashMap<i64, i64>, ignore_articles: bool) {
    let cmp_name = |a: &str, b: &str| by_name(a, b, ignore_articles);
    let desc = descending(SONG_ORDERS, index, reversed);
    // An album's songs in album order.
    let in_album = |a: &Track, b: &Track| {
        cmp_name(&a.album, &b.album)
            .then(a.disc_no.cmp(&b.disc_no))
            .then(a.track_no.cmp(&b.track_no))
            .then_with(|| cmp_name(&a.title, &b.title))
    };
    let artist_order = |a: &Track, b: &Track| cmp_name(a.grouping_artist(), b.grouping_artist()).then_with(|| in_album(a, b));
    songs.sort_by(|a, b| match index {
        1 => directed(cmp_name(&a.title, &b.title), desc, || artist_order(a, b)),
        2 => directed(cmp_name(&a.album, &b.album), desc, || artist_order(a, b)),
        3 => (a.year == 0)
            .cmp(&(b.year == 0))
            .then_with(|| directed(a.year.cmp(&b.year), desc, || artist_order(a, b))),
        4 => directed(a.duration_ms.cmp(&b.duration_ms), desc, || artist_order(a, b)),
        // Songs without a genre go last either way.
        5 => (a.genre.trim().is_empty())
            .cmp(&b.genre.trim().is_empty())
            .then_with(|| directed(cmp_name(&a.genre, &b.genre), desc, || artist_order(a, b))),
        6 => {
            let (x, y) = (added.get(&a.id).copied().unwrap_or(0), added.get(&b.id).copied().unwrap_or(0));
            directed(x.cmp(&y), desc, || artist_order(a, b))
        }
        _ => {
            let p = cmp_name(a.grouping_artist(), b.grouping_artist());
            (if desc { p.reverse() } else { p }).then_with(|| in_album(a, b))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(artist: &str, title: &str, year: u32, added: i64) -> Album {
        Album { artist: artist.into(), title: title.into(), year, added, ..Default::default() }
    }

    fn titles(albums: &[Album]) -> Vec<&str> {
        albums.iter().map(|a| a.title.as_str()).collect()
    }

    #[test]
    fn sort_keys_skip_articles() {
        assert_eq!(sort_key("The Beatles", true), "beatles");
        assert_eq!(sort_key("  An Album ", true), "album");
        assert_eq!(sort_key("The", true), "the");
        assert_eq!(sort_key("Theory", true), "theory");
        assert_eq!(sort_key("ABBA", true), "abba");
        // Setting off: "The Beatles" is the name, sorted under T.
        assert_eq!(sort_key("The Beatles", false), "the beatles");
    }

    #[test]
    fn sorts_albums() {
        let mut v = vec![
            album("The Beatles", "Abbey Road", 1969, 30),
            album("Air", "Moon Safari", 1998, 10),
            album("The Beatles", "Help!", 1965, 20),
            album("Björk", "Debut", 0, 40),
        ];
        // Artist (The Beatles under B), each artist's albums by year.
        sort_albums(&mut v, 0, false, true);
        assert_eq!(titles(&v), ["Moon Safari", "Help!", "Abbey Road", "Debut"]);
        // Reversed: artists Z–A, albums still oldest first.
        sort_albums(&mut v, 0, true, true);
        assert_eq!(titles(&v), ["Debut", "Help!", "Abbey Road", "Moon Safari"]);
        sort_albums(&mut v, 1, false, true);
        assert_eq!(titles(&v), ["Abbey Road", "Debut", "Help!", "Moon Safari"]);
        // Year: newest first, no year last; reversed oldest first, no year still last.
        sort_albums(&mut v, 2, false, true);
        assert_eq!(titles(&v), ["Moon Safari", "Abbey Road", "Help!", "Debut"]);
        sort_albums(&mut v, 2, true, true);
        assert_eq!(titles(&v), ["Help!", "Abbey Road", "Moon Safari", "Debut"]);
        sort_albums(&mut v, 3, false, true);
        assert_eq!(titles(&v), ["Debut", "Abbey Road", "Help!", "Moon Safari"]);
        // With articles counted, The Beatles come after Björk.
        sort_albums(&mut v, 0, false, false);
        assert_eq!(titles(&v), ["Moon Safari", "Debut", "Help!", "Abbey Road"]);
    }

    #[test]
    fn sorts_artists() {
        let a = |name: &str, albums: u32, songs: u32, added: i64| Artist {
            name: name.into(),
            album_count: albums,
            track_count: songs,
            added,
            ..Default::default()
        };
        let mut v = vec![a("The Cure", 1, 30, 5), a("Air", 3, 20, 1), a("Beck", 2, 40, 9)];
        let names = |v: &[Artist]| v.iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        sort_artists(&mut v, 0, false, true);
        assert_eq!(names(&v), ["Air", "Beck", "The Cure"]);
        sort_artists(&mut v, 1, false, true);
        assert_eq!(names(&v), ["Air", "Beck", "The Cure"]);
        sort_artists(&mut v, 2, false, true);
        assert_eq!(names(&v), ["Beck", "The Cure", "Air"]);
        sort_artists(&mut v, 3, false, true);
        assert_eq!(names(&v), ["Beck", "The Cure", "Air"]);
        sort_artists(&mut v, 0, true, true);
        assert_eq!(names(&v), ["The Cure", "Beck", "Air"]);
    }

    #[test]
    fn sorts_songs() {
        let t = |id: i64, title: &str, artist: &str, album: &str, no: u32, secs: u64, genre: &str| Track {
            id,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            track_no: no,
            duration_ms: secs * 1000,
            genre: genre.into(),
            ..Default::default()
        };
        let mut v = vec![
            t(1, "Zebra", "Beck", "Odelay", 2, 200, "Rock"),
            t(2, "Alpha", "Air", "Moon Safari", 1, 300, ""),
            t(3, "Devils Haircut", "Beck", "Odelay", 1, 100, "Alternative"),
        ];
        let ids = |v: &[Track]| v.iter().map(|t| t.id).collect::<Vec<_>>();
        let added = HashMap::from([(1, 30), (2, 10), (3, 20)]);
        sort_songs(&mut v, 0, false, &added, true);
        assert_eq!(ids(&v), [2, 3, 1]);
        sort_songs(&mut v, 1, false, &added, true);
        assert_eq!(ids(&v), [2, 3, 1]);
        sort_songs(&mut v, 4, false, &added, true);
        assert_eq!(ids(&v), [3, 1, 2]);
        sort_songs(&mut v, 4, true, &added, true);
        assert_eq!(ids(&v), [2, 1, 3]);
        // Genre: no genre last.
        sort_songs(&mut v, 5, false, &added, true);
        assert_eq!(ids(&v), [3, 1, 2]);
        sort_songs(&mut v, 6, false, &added, true);
        assert_eq!(ids(&v), [1, 3, 2]);
    }
}
