//! Local music library: folder scanning, tag reading and the SQLite index.

mod db;
pub mod m3u;
pub mod scanner;

pub use db::stats::{count_threshold, LibraryTotals, PlayStats, Ranked};
pub use db::{EpisodeState, Library, Podcast};
