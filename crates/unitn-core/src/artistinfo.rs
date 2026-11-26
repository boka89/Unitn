//! Artist biographies and photos from TheAudioDB (https://www.theaudiodb.com),
//! a community music database with a free API. Biographies come in several
//! languages; photos ("artist thumbs") are square.
//!
//! The free key is shared by everyone and limited to about 30 requests a
//! minute, so callers look an artist up only when needed and keep the result
//! (see `Library::artist_info`).

use std::io::Read;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::url_encode;

/// TheAudioDB's free, public API key.
const API: &str = "https://www.theaudiodb.com/api/v1/json/123";
/// Photos larger than this are ignored.
const MAX_IMAGE: u64 = 8 * 1024 * 1024;

/// The biography languages TheAudioDB has: its field suffix, the ISO 639-1
/// codes that pick it, and its name in that language.
pub const LANGUAGES: [(&str, &[&str], &str); 15] = [
    ("EN", &["en"], "English"),
    ("DE", &["de"], "Deutsch"),
    ("FR", &["fr"], "Français"),
    ("ES", &["es"], "Español"),
    ("IT", &["it"], "Italiano"),
    ("PT", &["pt"], "Português"),
    ("NL", &["nl"], "Nederlands"),
    ("SE", &["sv"], "Svenska"),
    ("NO", &["no", "nb", "nn"], "Norsk"),
    ("PL", &["pl"], "Polski"),
    ("HU", &["hu"], "Magyar"),
    ("RU", &["ru"], "Русский"),
    ("IL", &["he", "iw"], "עברית"),
    ("JP", &["ja"], "日本語"),
    ("CN", &["zh"], "中文"),
];

/// What TheAudioDB knows about an artist.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistInfo {
    pub id: String,
    /// The name as TheAudioDB writes it (e.g. "Céline Dion").
    pub name: String,
    /// Biographies: (language suffix such as "EN", text), English first.
    pub bios: Vec<(String, String)>,
    /// Square photo, about 700 px.
    pub thumb: Option<String>,
    pub genre: String,
    pub country: String,
    pub formed: String,
}

impl ArtistInfo {
    fn from_json(a: &Value) -> Option<Self> {
        let text = |k: &str| a[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let name = text("strArtist")?;
        let bios = LANGUAGES
            .iter()
            .filter_map(|(suffix, _, _)| {
                // English is "strBiographyEN" in the docs, plain "strBiography" in some answers.
                let text = text(&format!("strBiography{suffix}")).or_else(|| if *suffix == "EN" { text("strBiography") } else { None });
                text.map(|b| (suffix.to_string(), clean_bio(&b)))
            })
            .collect();
        Some(Self {
            id: text("idArtist").unwrap_or_default(),
            name,
            bios,
            thumb: text("strArtistThumb"),
            genre: text("strGenre").unwrap_or_default(),
            country: text("strCountry").unwrap_or_default(),
            formed: text("intFormedYear").or_else(|| text("intBornYear")).unwrap_or_default(),
        })
    }

    /// The biography in the first of `languages` (suffixes such as "DE")
    /// that has one, else English, else any.
    pub fn bio_index(&self, languages: &[&str]) -> Option<usize> {
        languages
            .iter()
            .chain(std::iter::once(&"EN"))
            .find_map(|l| self.bios.iter().position(|(s, _)| s == l))
            .or(if self.bios.is_empty() { None } else { Some(0) })
    }
}

/// The TheAudioDB language for an ISO 639-1 code or locale ("hu", "hu_HU",
/// "pt-BR"), if it has biographies in it.
pub fn language_for(locale: &str) -> Option<&'static str> {
    let code = locale.split(['_', '-', '.', '@']).next()?.to_lowercase();
    LANGUAGES.iter().find(|(_, codes, _)| codes.contains(&code.as_str())).map(|(s, _, _)| *s)
}

/// The name of a TheAudioDB language suffix in that language.
pub fn language_name(suffix: &str) -> &'static str {
    LANGUAGES.iter().find(|(s, _, _)| *s == suffix).map(|(_, _, n)| *n).unwrap_or("")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(crate::USER_AGENT)
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
}

/// Looks `name` up: the name itself, then (for "A & B", "A feat. B"…) the
/// first artist, then the spelling MusicBrainz knows (it finds "Céline Dion"
/// for "Celine Dion"). `Ok(None)`: not found.
pub fn lookup(name: &str, musicbrainz: &mut crate::artwork::ArtFetcher) -> Result<Option<ArtistInfo>> {
    let candidates = candidates(name);
    if candidates.is_empty() {
        return Ok(None);
    }
    for c in &candidates {
        if let Some(info) = search(c)? {
            return Ok(Some(info));
        }
    }
    // MusicBrainz: its ID finds the artist on TheAudioDB whatever the spelling.
    if let Some((mbid, canonical)) = musicbrainz.find_artist(&candidates[0])? {
        if let Some(info) = by_musicbrainz_id(&mbid)? {
            return Ok(Some(info));
        }
        if canonical != candidates[0] {
            if let Some(info) = search(&canonical)? {
                return Ok(Some(info));
            }
        }
    }
    Ok(None)
}

/// The names to try for `name`, most specific first.
pub fn candidates(name: &str) -> Vec<String> {
    let name = name.trim();
    if name.is_empty() || ["unknown artist", "various artists", "various", "va"].contains(&name.to_lowercase().as_str()) {
        return Vec::new();
    }
    let mut out = vec![name.to_string()];
    let lower = name.to_lowercase();
    for sep in [" feat. ", " feat ", " ft. ", " featuring ", " & ", " and ", " x ", " / ", "; ", ", "] {
        if let Some(i) = lower.find(sep) {
            let first = name[..i].trim().to_string();
            if !first.is_empty() && !out.contains(&first) {
                out.push(first);
            }
        }
    }
    out
}

fn search(name: &str) -> Result<Option<ArtistInfo>> {
    let json: Value = agent().get(&format!("{API}/search.php?s={}", url_encode(name))).call()?.into_json()?;
    Ok(best_match(&json, name))
}

fn by_musicbrainz_id(mbid: &str) -> Result<Option<ArtistInfo>> {
    let json: Value = agent().get(&format!("{API}/artist-mb.php?i={}", url_encode(mbid))).call()?.into_json()?;
    Ok(json["artists"].as_array().and_then(|a| a.first()).and_then(ArtistInfo::from_json))
}

/// The result whose name is `name` (ignoring case, accents, punctuation
/// and a leading "The"). Never a near miss: "Bob Marley" must not pick
/// "Bob Marley & The Wailers".
pub(crate) fn best_match(results: &Value, name: &str) -> Option<ArtistInfo> {
    let want = fold(name);
    results["artists"]
        .as_array()?
        .iter()
        .find(|a| a["strArtist"].as_str().is_some_and(|n| fold(n) == want))
        .and_then(ArtistInfo::from_json)
}

/// Downloads a photo (`ArtistInfo::thumb`).
pub fn download(url: &str) -> Result<Option<Vec<u8>>> {
    let mut data = Vec::new();
    agent().get(url).call()?.into_reader().take(MAX_IMAGE).read_to_end(&mut data)?;
    let image = data.starts_with(&[0xFF, 0xD8]) || data.starts_with(b"\x89PNG") || data.get(8..12) == Some(b"WEBP");
    Ok(image.then_some(data))
}

/// For comparing names: lower case, no accents, letters and digits only,
/// without a leading "the".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        let base = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'č' | 'ĉ' => 'c',
            'ď' | 'đ' => 'd',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
            'ğ' | 'ĝ' => 'g',
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
            'ł' | 'ľ' | 'ĺ' => 'l',
            'ñ' | 'ń' | 'ň' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
            'ř' | 'ŕ' => 'r',
            'ś' | 'š' | 'ş' | 'ș' => 's',
            'ť' | 'ţ' | 'ț' => 't',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'ß' => 's',
            'æ' => 'a',
            'œ' => 'o',
            '&' => ' ',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        out.push(base);
    }
    let words: Vec<&str> = out.split_whitespace().collect();
    let words = if words.len() > 1 && words[0] == "the" { &words[1..] } else { &words[..] };
    words.join(" ")
}

/// Biographies sometimes come with Windows line ends and runs of blank lines.
fn clean_bio(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_case_and_the() {
        assert_eq!(fold("Céline Dion"), fold("celine dion"));
        assert_eq!(fold("The Beatles"), "beatles");
        assert_eq!(fold("Mötley Crüe"), "motley crue");
        assert_eq!(fold("The The"), "the");
        assert_ne!(fold("Bob Marley"), fold("Bob Marley & The Wailers"));
    }

    #[test]
    fn tries_the_first_artist_of_a_duo() {
        assert_eq!(candidates("Bob Marley & The Wailers"), vec!["Bob Marley & The Wailers", "Bob Marley"]);
        assert_eq!(candidates("Calvin Harris feat. Rihanna"), vec!["Calvin Harris feat. Rihanna", "Calvin Harris"]);
        assert!(candidates("Various Artists").is_empty());
    }

    #[test]
    fn picks_the_exact_artist_only() {
        let json = serde_json::json!({"artists": [
            {"idArtist": "1", "strArtist": "Bob Marley & The Wailers", "strBiographyEN": "Band."},
            {"idArtist": "2", "strArtist": "Bob Marley", "strBiographyEN": "Singer.\r\n\r\n\r\nMore.",
             "strBiographyHU": "Énekes.", "strArtistThumb": "https://x/thumb.jpg", "intBornYear": "1945"}
        ]});
        let info = best_match(&json, "bob marley").unwrap();
        assert_eq!(info.id, "2");
        assert_eq!(info.bios[0], ("EN".to_string(), "Singer.\n\nMore.".to_string()));
        assert_eq!(info.bio_index(&["HU"]), Some(1));
        assert_eq!(info.bio_index(&["FR"]), Some(0));
        assert_eq!(info.thumb.as_deref(), Some("https://x/thumb.jpg"));
        assert_eq!(info.formed, "1945");
        assert!(best_match(&json, "Bob").is_none());
        assert!(best_match(&serde_json::json!({"artists": null}), "Bob Marley").is_none());
    }

    #[test]
    fn languages_from_locales() {
        assert_eq!(language_for("hu_HU.UTF-8"), Some("HU"));
        assert_eq!(language_for("pt-BR"), Some("PT"));
        assert_eq!(language_for("nb"), Some("NO"));
        assert_eq!(language_for("ko_KR"), None);
        assert_eq!(language_name("DE"), "Deutsch");
    }
}
