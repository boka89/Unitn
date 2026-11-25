//! Internet radio directory via the free, open Radio Browser API
//! (<https://www.radio-browser.info>, ~50,000 stations, no API key).

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::model::Station;

/// Public mirrors; tried in order until one answers.
const MIRRORS: &[&str] = &[
    "https://de1.api.radio-browser.info",
    "https://nl1.api.radio-browser.info",
    "https://de2.api.radio-browser.info",
    "https://at1.api.radio-browser.info",
];

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StationDto {
    stationuuid: String,
    name: String,
    url: String,
    url_resolved: String,
    favicon: String,
    country: String,
    tags: String,
    codec: String,
    bitrate: u32,
    clickcount: u64,
}

impl From<StationDto> for Station {
    fn from(d: StationDto) -> Self {
        Station {
            uuid: d.stationuuid,
            name: d.name.trim().to_string(),
            url: if d.url_resolved.trim().is_empty() { d.url.trim().to_string() } else { d.url_resolved.trim().to_string() },
            favicon: d.favicon,
            country: d.country,
            tags: d.tags,
            codec: d.codec,
            bitrate: d.bitrate,
        }
    }
}

#[derive(Clone)]
pub struct RadioBrowser {
    agent: ureq::Agent,
}

impl Default for RadioBrowser {
    fn default() -> Self {
        Self::new()
    }
}

/// What to browse stations by, besides genre chips and name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetKind {
    Country,
    Language,
    Tag,
    Codec,
}

/// One country, language, tag or format, and how many stations have it.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    /// Shown to the user ("Germany", "English", "jazz", "AAC").
    pub name: String,
    /// Sent to the search: the country code for countries, else the name.
    pub value: String,
    pub stations: u32,
}

#[derive(Debug, Deserialize)]
struct FacetDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    iso_3166_1: String,
    #[serde(default)]
    stationcount: u32,
}

fn facets_from(kind: FacetKind, rows: Vec<FacetDto>) -> Vec<Facet> {
    let mut out: Vec<Facet> = rows
        .into_iter()
        .filter(|r| !r.name.trim().is_empty() && r.stationcount > 0)
        .filter(|r| kind != FacetKind::Country || !r.iso_3166_1.is_empty())
        .map(|r| {
            let value = if kind == FacetKind::Country { r.iso_3166_1.to_uppercase() } else { r.name.trim().to_string() };
            // Languages come in lower case ("english").
            let name = if kind == FacetKind::Language { capitalize(r.name.trim()) } else { r.name.trim().to_string() };
            Facet { name, value, stations: r.stationcount }
        })
        .collect();
    out.sort_by(|a, b| b.stations.cmp(&a.stations).then_with(|| a.name.cmp(&b.name)));
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Filters for browsing the directory. Empty / zero = any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub name: String,
    /// Any of these tags (genres).
    pub tags: Vec<String>,
    /// ISO 3166 country code ("HU").
    pub country: Option<String>,
    /// Language name as the directory has it ("english").
    pub language: Option<String>,
    /// Format ("MP3", "AAC").
    pub codec: Option<String>,
    /// At least this bitrate (kbps).
    pub bitrate_min: u32,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty()
            && self.tags.is_empty()
            && self.country.is_none()
            && self.language.is_none()
            && self.codec.is_none()
            && self.bitrate_min == 0
    }

    /// The search parameters (without the tag, which is asked one at a time).
    fn query(&self, limit: u32) -> Vec<(&'static str, String)> {
        let mut q = vec![
            ("limit", limit.to_string()),
            ("hidebroken", "true".to_string()),
            ("order", "clickcount".to_string()),
            ("reverse", "true".to_string()),
        ];
        let name = self.name.trim();
        if !name.is_empty() {
            q.push(("name", name.to_string()));
        }
        if let Some(c) = &self.country {
            q.push(("countrycode", c.to_uppercase()));
        }
        if let Some(l) = &self.language {
            q.push(("language", l.to_lowercase()));
            q.push(("languageExact", "true".to_string()));
        }
        if let Some(c) = &self.codec {
            q.push(("codec", c.clone()));
        }
        if self.bitrate_min > 0 {
            q.push(("bitrateMin", self.bitrate_min.to_string()));
        }
        q
    }
}

impl RadioBrowser {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build();
        Self { agent }
    }

    fn dtos(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<StationDto>> {
        let mut last_error = String::from("no mirror available");
        for mirror in MIRRORS {
            let mut req = self.agent.get(&format!("{mirror}{path}"));
            for (k, v) in query {
                req = req.query(k, v);
            }
            match req.call() {
                Ok(resp) => return Ok(resp.into_json()?),
                Err(e) => last_error = e.to_string(),
            }
        }
        Err(anyhow!("Radio Browser is unreachable ({last_error})"))
    }

    fn stations(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<Station>> {
        Ok(usable(self.dtos(path, query)?.into_iter().map(Station::from)))
    }

    /// Stations matching several filters at once, most popular first:
    /// any of `filter.tags` (OR; no tags = every genre), and every other
    /// filter that is set (AND).
    pub fn filtered(&self, filter: &Filter, limit: u32) -> Result<Vec<Station>> {
        let base_owned = filter.query(limit);
        let base: Vec<(&str, &str)> = base_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let tags = &filter.tags;
        let mut lists = Vec::new();
        if tags.is_empty() {
            lists.push(self.dtos("/json/stations/search", &base)?);
        } else {
            for tag in tags {
                let mut query = base.clone();
                query.push(("tag", tag));
                lists.push(self.dtos("/json/stations/search", &query)?);
            }
        }
        Ok(merge_ranked(lists, limit as usize))
    }

    /// Every country, language, tag or format in the directory with how
    /// many stations it has, most stations first (tags: the 600 biggest).
    pub fn facets(&self, kind: FacetKind) -> Result<Vec<Facet>> {
        let (path, query): (&str, Vec<(&str, &str)>) = match kind {
            FacetKind::Country => ("/json/countries", vec![("hidebroken", "true")]),
            FacetKind::Language => ("/json/languages", vec![("hidebroken", "true")]),
            FacetKind::Tag => ("/json/tags", vec![("hidebroken", "true"), ("order", "stationcount"), ("reverse", "true"), ("limit", "600")]),
            FacetKind::Codec => ("/json/codecs", vec![("hidebroken", "true")]),
        };
        let mut last_error = String::from("no mirror available");
        for mirror in MIRRORS {
            let mut req = self.agent.get(&format!("{mirror}{path}"));
            for (k, v) in &query {
                req = req.query(k, v);
            }
            match req.call() {
                Ok(resp) => {
                    let rows: Vec<FacetDto> = resp.into_json()?;
                    return Ok(facets_from(kind, rows));
                }
                Err(e) => last_error = e.to_string(),
            }
        }
        Err(anyhow!("Radio Browser is unreachable ({last_error})"))
    }

    /// Most popular stations worldwide.
    pub fn top(&self, limit: u32) -> Result<Vec<Station>> {
        self.stations(&format!("/json/stations/topclick/{limit}"), &[("hidebroken", "true")])
    }

    /// Stations whose name matches `name`, most popular first.
    pub fn search(&self, name: &str, limit: u32) -> Result<Vec<Station>> {
        let limit = limit.to_string();
        self.stations(
            "/json/stations/search",
            &[
                ("name", name),
                ("limit", &limit),
                ("hidebroken", "true"),
                ("order", "clickcount"),
                ("reverse", "true"),
            ],
        )
    }

    /// Stations with a tag (genre), e.g. "jazz".
    pub fn by_tag(&self, tag: &str, limit: u32) -> Result<Vec<Station>> {
        let limit = limit.to_string();
        self.stations(
            "/json/stations/search",
            &[("tag", tag), ("limit", &limit), ("hidebroken", "true"), ("order", "clickcount"), ("reverse", "true")],
        )
    }

    /// Most popular stations of a country (ISO 3166 code, e.g. "HU").
    pub fn by_country(&self, country_code: &str, limit: u32) -> Result<Vec<Station>> {
        let limit = limit.to_string();
        self.stations(
            &format!("/json/stations/bycountrycodeexact/{}", country_code.to_uppercase()),
            &[("limit", &limit), ("hidebroken", "true"), ("order", "clickcount"), ("reverse", "true")],
        )
    }

    /// Tells Radio Browser a station was played (keeps its popularity data useful).
    pub fn count_click(&self, uuid: &str) {
        for mirror in MIRRORS {
            if self.agent.get(&format!("{mirror}/json/url/{uuid}")).call().is_ok() {
                return;
            }
        }
    }
}

/// Drops stations that cannot be played or shown.
fn usable(list: impl Iterator<Item = Station>) -> Vec<Station> {
    list.filter(|s| !s.url.is_empty() && !s.name.is_empty()).collect()
}

/// Joins several result lists into one: each station once, most clicked first.
fn merge_ranked(lists: Vec<Vec<StationDto>>, limit: usize) -> Vec<Station> {
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<StationDto> = lists.into_iter().flatten().filter(|d| seen.insert(d.stationuuid.clone())).collect();
    all.sort_by(|a, b| b.clickcount.cmp(&a.clickcount));
    let mut list = usable(all.into_iter().map(Station::from));
    list.truncate(limit);
    list
}

#[cfg(test)]
mod tests {

    #[test]
    fn filter_builds_the_search() {
        let f = Filter {
            name: " Jazz FM ".into(),
            country: Some("hu".into()),
            language: Some("English".into()),
            codec: Some("AAC".into()),
            bitrate_min: 128,
            ..Default::default()
        };
        let q = f.query(50);
        let get = |k: &str| q.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("name"), Some("Jazz FM"));
        assert_eq!(get("countrycode"), Some("HU"));
        assert_eq!(get("language"), Some("english"));
        assert_eq!(get("languageExact"), Some("true"));
        assert_eq!(get("codec"), Some("AAC"));
        assert_eq!(get("bitrateMin"), Some("128"));
        assert_eq!(get("limit"), Some("50"));
        assert!(!f.is_empty());
        let any = Filter::default();
        assert!(any.is_empty());
        assert_eq!(any.query(10).iter().filter(|(k, _)| matches!(*k, "countrycode" | "language" | "codec" | "bitrateMin")).count(), 0);
    }

    #[test]
    fn facets_are_cleaned_and_sorted() {
        let rows: Vec<FacetDto> = serde_json::from_str(
            r#"[{"name":"Hungary","iso_3166_1":"HU","stationcount":120},
                {"name":"Germany","iso_3166_1":"DE","stationcount":5000},
                {"name":"Nowhere","iso_3166_1":"","stationcount":3},
                {"name":"Empty","iso_3166_1":"XX","stationcount":0}]"#,
        )
        .unwrap();
        let c = facets_from(FacetKind::Country, rows);
        assert_eq!(c.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["DE", "HU"]);
        assert_eq!(c[1].name, "Hungary");
        let rows: Vec<FacetDto> = serde_json::from_str(r#"[{"name":"english","iso_639":"en","stationcount":9}]"#).unwrap();
        let l = facets_from(FacetKind::Language, rows);
        assert_eq!((l[0].name.as_str(), l[0].value.as_str()), ("English", "english"));
    }

    use super::*;

    #[test]
    fn converts_api_records() {
        let json = r#"[{"stationuuid":"abc","name":" Jazz FM ","url":"http://a/pls","url_resolved":"http://a/stream",
                        "favicon":"","country":"Germany","tags":"jazz,swing","codec":"MP3","bitrate":128,"votes":5}]"#;
        let list: Vec<StationDto> = serde_json::from_str(json).unwrap();
        let s: Station = list.into_iter().next().unwrap().into();
        assert_eq!(s.name, "Jazz FM");
        assert_eq!(s.url, "http://a/stream");
        assert_eq!(s.info(), "Germany • jazz, swing • MP3 128 kbps");
    }

    #[test]
    fn merges_results_of_several_genres() {
        let dto = |id: &str, clicks: u64| StationDto {
            stationuuid: id.into(),
            name: id.into(),
            url: format!("http://{id}"),
            clickcount: clicks,
            ..Default::default()
        };
        let jazz = vec![dto("a", 50), dto("b", 10)];
        let rock = vec![dto("c", 30), dto("a", 50)];
        let merged = merge_ranked(vec![jazz, rock], 10);
        let ids: Vec<&str> = merged.iter().map(|s| s.uuid.as_str()).collect();
        assert_eq!(ids, ["a", "c", "b"]);
        assert_eq!(merge_ranked(vec![vec![dto("a", 1), dto("b", 2)]], 1)[0].uuid, "b");
    }
}
