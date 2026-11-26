//! Looks artists up on TheAudioDB and checks the biography, the photo and
//! the name matching (accents via MusicBrainz, "A & B" → A).
//!
//!     cargo run -p unitn-core --example artistinfo_check -- "Artist" [more artists…]

use unitn_core::artistinfo;
use unitn_core::artwork::ArtFetcher;

fn main() {
    let names: Vec<String> = std::env::args().skip(1).collect();
    if names.is_empty() {
        eprintln!("usage: artistinfo_check <artist> [artist…]");
        std::process::exit(2);
    }
    let mut fetcher = ArtFetcher::new();
    let mut failed = false;
    for name in names {
        std::thread::sleep(std::time::Duration::from_millis(2100));
        match artistinfo::lookup(&name, &mut fetcher) {
            Ok(Some(info)) => {
                let langs: Vec<&str> = info.bios.iter().map(|(l, _)| l.as_str()).collect();
                let en = info.bio_index(&["EN"]).map(|i| info.bios[i].1.chars().take(120).collect::<String>()).unwrap_or_default();
                println!("{name} → {} (id {}), {} • {} • {}", info.name, info.id, info.genre, info.country, info.formed);
                println!("  bios: {langs:?}\n  thumb: {:?}\n  {en}…", info.thumb);
                let photo = info.thumb.as_deref().map(|u| artistinfo::download(u).ok().flatten().map(|d| d.len()));
                println!("  photo bytes: {photo:?}");
                if info.bios.is_empty() {
                    println!("RESULT artist info {name}: FAILED: no biography");
                    failed = true;
                } else {
                    println!("RESULT artist info {name}: OK ({} languages)", info.bios.len());
                }
            }
            Ok(None) => {
                println!("RESULT artist info {name}: FAILED: not found");
                failed = true;
            }
            Err(e) => {
                println!("RESULT artist info {name}: FAILED: {e:#}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
