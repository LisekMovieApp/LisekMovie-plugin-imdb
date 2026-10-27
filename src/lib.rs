//! IMDb metadata provider extension for LisekMovie.
//!
//! IMDb has no free official API (its data is sold through AWS Data
//! Exchange). What this extension uses is the public suggestion endpoint
//! behind the search box of imdb.com:
//!
//! ```text
//! GET https://v3.sg.media-imdb.com/suggestion/{first letter}/{query}.json
//! ```
//!
//! It needs no key and answers both a title (`matrix`) and an IMDb id
//! (`tt0133093`) with a list of `{ id, l: title, y: year, qid: kind,
//! i: { imageUrl } , s: main cast }` entries. That is enough to find a
//! title, show it in Fix match and pin its IMDb id (`fields.imdb_id`), which
//! then links to the IMDb page and lets TMDB / OMDb fill in the rest.
//! There is no plot, genre or rating in it: for those enable OMDb (an
//! IMDb-backed API with a free key) or TMDB.
//!
//! The endpoint is unofficial and can change without notice; every
//! response is parsed leniently and a shape it does not understand is
//! reported as [`ProviderError::UnexpectedResponse`], never a panic.

use std::time::Duration;

use async_trait::async_trait;
use lisek_plugin_api::{
    DirectId, MediaKind, MetadataProvider, MovieQuery, ProviderCandidate, ProviderError,
    ProviderMatch, SeriesQuery, normalize_query_title,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://v3.sg.media-imdb.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const LIMIT: usize = 10;

#[derive(Debug, Deserialize)]
struct SuggestionResponse {
    #[serde(default)]
    d: Vec<RawSuggestion>,
}

#[derive(Debug, Deserialize)]
struct RawImage {
    #[serde(rename = "imageUrl")]
    image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSuggestion {
    id: String,
    #[serde(default)]
    l: String,
    y: Option<i32>,
    qid: Option<String>,
    i: Option<RawImage>,
    /// Main cast, comma separated.
    s: Option<String>,
}

/// One IMDb title from the suggestion endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct ImdbTitle {
    pub imdb_id: String,
    pub title: String,
    pub year: Option<i32>,
    /// `movie`, `tvSeries`, `tvMiniSeries`, `tvMovie`, `short`, `video`...
    pub kind: String,
    pub image_url: Option<String>,
    pub cast: Option<String>,
}

impl ImdbTitle {
    fn is(&self, kind: MediaKind) -> bool {
        match kind {
            MediaKind::Movie => {
                matches!(self.kind.as_str(), "movie" | "tvMovie" | "video" | "short")
            }
            MediaKind::Series => matches!(self.kind.as_str(), "tvSeries" | "tvMiniSeries"),
        }
    }
}

/// `tt` followed by digits.
fn is_title_id(id: &str) -> bool {
    id.len() > 2 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
}

/// Amazon image links resize by their `._V1_` suffix: `._V1_UX185_.jpg`
/// is 185 px wide. Other links stay as they are.
pub fn resized(url: &str, width: u32) -> String {
    match url.rsplit_once("._V1_") {
        Some((base, _)) => format!("{base}._V1_UX{width}_.jpg"),
        None => url.to_string(),
    }
}

/// The path segment the endpoint files a query under: its first letter or
/// digit, lowercase.
fn bucket(query: &str) -> char {
    query
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map_or('x', |c| c.to_ascii_lowercase())
}

/// Plain HTTP client of the suggestion endpoint. `base_url` is injectable
/// for tests.
pub struct ImdbClient {
    http: reqwest::Client,
    base_url: String,
}

impl ImdbClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .user_agent("LisekMovie (+https://github.com/LisekMovieApp/LisekMovie-app)")
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
        }
    }

    /// Titles for a query (a title or an IMDb id), people left out.
    pub async fn suggest(&self, query: &str) -> Result<Vec<ImdbTitle>, ProviderError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|err| ProviderError::Request(err.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::Request("base URL cannot have a path".into()))?
            .pop_if_empty()
            .push("suggestion")
            .push(&bucket(query).to_string())
            .push(&format!("{query}.json"));
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|err| ProviderError::Request(err.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let body: SuggestionResponse = response
            .error_for_status()
            .map_err(|err| ProviderError::Request(err.to_string()))?
            .json()
            .await
            .map_err(|err| ProviderError::UnexpectedResponse(err.to_string()))?;
        Ok(body
            .d
            .into_iter()
            .filter(|raw| is_title_id(&raw.id) && !raw.l.trim().is_empty())
            .map(|raw| ImdbTitle {
                imdb_id: raw.id,
                title: raw.l.trim().to_string(),
                year: raw.y,
                kind: raw.qid.unwrap_or_default(),
                image_url: raw
                    .i
                    .and_then(|i| i.image_url)
                    .filter(|u| u.starts_with("http")),
                cast: raw.s.filter(|s| !s.trim().is_empty()),
            })
            .collect())
    }
}

impl Default for ImdbClient {
    fn default() -> Self {
        Self::new()
    }
}

fn match_from(title: &ImdbTitle, with_title: bool) -> ProviderMatch {
    let mut fields = serde_json::json!({ "imdb_id": title.imdb_id });
    if with_title {
        fields["title"] = serde_json::json!(title.title);
        fields["year"] = serde_json::json!(title.year);
    }
    ProviderMatch {
        poster_url: title.image_url.as_deref().map(|u| resized(u, 500)),
        fields,
        ..ProviderMatch::default()
    }
}

/// The [`MetadataProvider`] for `"imdb"`. Needs no API key.
pub struct ImdbProvider {
    base_url: String,
}

impl ImdbProvider {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn client(&self) -> ImdbClient {
        ImdbClient::with_base_url(self.base_url.clone())
    }

    /// The first title of `kind` the suggestions have for `title`; a year,
    /// when given, must match within one year (festival vs release year).
    async fn find(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<ImdbTitle>, ProviderError> {
        let query = normalize_query_title(title);
        let found = self.client().suggest(&query).await?;
        Ok(found.into_iter().find(|t| {
            t.is(kind)
                && year.is_none_or(|wanted| t.year.is_some_and(|y| (y - wanted).abs() <= 1))
                && normalize_query_title(&t.title) == query
        }))
    }
}

impl Default for ImdbProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetadataProvider for ImdbProvider {
    fn provider_key(&self) -> &'static str {
        "imdb"
    }

    async fn find_movie(
        &self,
        query: &MovieQuery<'_>,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        Ok(self
            .find(MediaKind::Movie, query.title, query.year)
            .await?
            .map(|t| match_from(&t, false)))
    }

    async fn find_series(
        &self,
        query: &SeriesQuery<'_>,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        Ok(self
            .find(MediaKind::Series, query.title, None)
            .await?
            .map(|t| match_from(&t, false)))
    }

    async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
        _lang: &str,
    ) -> Result<Vec<ProviderCandidate>, ProviderError> {
        Ok(self
            .client()
            .suggest(title)
            .await?
            .into_iter()
            .filter(|t| t.is(kind) && year.is_none_or(|wanted| t.year == Some(wanted)))
            .take(LIMIT)
            .map(|t| ProviderCandidate {
                provider_id: t.imdb_id,
                title: t.title,
                year: t.year,
                overview: t.cast.unwrap_or_default(),
                poster_url: t.image_url.as_deref().map(|u| resized(u, 185)),
            })
            .collect())
    }

    async fn lookup(
        &self,
        _kind: MediaKind,
        provider_id: &str,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        let id = provider_id.trim().to_ascii_lowercase();
        if !is_title_id(&id) {
            return Ok(None);
        }
        Ok(self
            .client()
            .suggest(&id)
            .await?
            .into_iter()
            .find(|t| t.imdb_id == id)
            .map(|t| match_from(&t, true)))
    }

    /// IMDb ids only; a plain number is not an IMDb id.
    async fn lookup_by_id(
        &self,
        kind: MediaKind,
        id: &DirectId,
        lang: &str,
    ) -> Result<Option<ProviderCandidate>, ProviderError> {
        let DirectId::Imdb(imdb) = id else {
            return Ok(None);
        };
        Ok(self
            .lookup(kind, imdb, lang)
            .await?
            .and_then(|matched| ProviderCandidate::from_match(imdb, &matched)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn suggestions() -> serde_json::Value {
        serde_json::json!({ "d": [
            { "id": "tt0133093", "l": "The Matrix", "y": 1999, "qid": "movie", "s": "Keanu Reeves, Laurence Fishburne",
              "i": { "imageUrl": "https://m.media-amazon.com/images/M/abc._V1_.jpg", "width": 1, "height": 1 } },
            { "id": "tt0106062", "l": "Matrix", "y": 1993, "qid": "tvSeries" },
            { "id": "nm0000206", "l": "Keanu Reeves" },
            { "id": "tt9999999", "l": " " }
        ], "q": "the matrix", "v": 1 })
    }

    #[test]
    fn images_resize_through_the_v1_suffix() {
        assert_eq!(
            resized("https://m.media-amazon.com/images/M/abc._V1_.jpg", 185),
            "https://m.media-amazon.com/images/M/abc._V1_UX185_.jpg"
        );
        assert_eq!(resized("https://other/x.jpg", 185), "https://other/x.jpg");
        assert_eq!(bucket("  the matrix"), 't');
        assert_eq!(bucket("300"), '3');
    }

    #[tokio::test]
    async fn search_keeps_titles_of_the_kind_and_drops_people() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/suggestion/t/the%20matrix.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(suggestions()))
            .mount(&server)
            .await;
        let provider = ImdbProvider::with_base_url(server.uri());

        let movies = provider
            .search(MediaKind::Movie, "the matrix", None, "pl")
            .await
            .unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(
            (movies[0].provider_id.as_str(), movies[0].year),
            ("tt0133093", Some(1999))
        );
        assert_eq!(movies[0].overview, "Keanu Reeves, Laurence Fishburne");
        assert_eq!(
            movies[0].poster_url.as_deref(),
            Some("https://m.media-amazon.com/images/M/abc._V1_UX185_.jpg")
        );
        let series = provider
            .search(MediaKind::Series, "the matrix", None, "pl")
            .await
            .unwrap();
        assert_eq!(series[0].provider_id, "tt0106062");
        assert!(
            provider
                .search(MediaKind::Movie, "the matrix", Some(2003), "pl")
                .await
                .unwrap()
                .is_empty()
        );

        let found = provider
            .find_movie(
                &MovieQuery {
                    title: "The Matrix",
                    year: Some(1999),
                },
                "pl",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.fields["imdb_id"], "tt0133093");
        assert!(found.fields.get("title").is_none());
    }

    #[tokio::test]
    async fn an_imdb_id_is_looked_up_and_numbers_are_not() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/suggestion/t/tt0133093.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(suggestions()))
            .mount(&server)
            .await;
        let provider = ImdbProvider::with_base_url(server.uri());

        let candidate = provider
            .lookup_by_id(MediaKind::Movie, &DirectId::Imdb("tt0133093".into()), "pl")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (candidate.title.as_str(), candidate.year),
            ("The Matrix", Some(1999))
        );
        let matched = provider
            .lookup(MediaKind::Movie, "TT0133093", "pl")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(matched.fields["title"], "The Matrix");
        assert!(
            provider
                .lookup(MediaKind::Movie, "603", "pl")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            provider
                .lookup_by_id(MediaKind::Movie, &DirectId::Numeric("603".into()), "pl")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_unknown_shape_is_an_unexpected_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let provider = ImdbProvider::with_base_url(server.uri());
        assert!(matches!(
            provider.search(MediaKind::Movie, "x", None, "pl").await,
            Err(ProviderError::UnexpectedResponse(_))
        ));
    }

    #[test]
    fn provider_key_is_imdb() {
        assert_eq!(ImdbProvider::new().provider_key(), "imdb");
    }
}
