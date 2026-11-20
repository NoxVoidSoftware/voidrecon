//! Web search and page fetching for the LLM's `web_search` / `fetch_url` tools.
//!
//! Search goes through a configurable provider: DuckDuckGo and a SearXNG
//! instance need no key, while Google Programmable Search, Brave and Tavily use
//! keys saved in config. Other configured providers are tried when one fails.
//! Everything returned here is untrusted web content.

mod html;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

pub use html::html_to_text;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Bytes of a fetched page that are downloaded at most.
const FETCH_MAX_BYTES: usize = 3 * 1024 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; voidrecon)";

/// Search backend used by `web_search`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchProvider {
    #[default]
    DuckDuckGo,
    Google,
    Brave,
    Tavily,
    SearXng,
}

impl SearchProvider {
    pub const ALL: [SearchProvider; 5] = [
        SearchProvider::DuckDuckGo,
        SearchProvider::Brave,
        SearchProvider::Tavily,
        SearchProvider::Google,
        SearchProvider::SearXng,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SearchProvider::DuckDuckGo => "duckduckgo",
            SearchProvider::Google => "google",
            SearchProvider::Brave => "brave",
            SearchProvider::Tavily => "tavily",
            SearchProvider::SearXng => "searxng",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_lowercase();
        match name.as_str() {
            "ddg" => return Some(SearchProvider::DuckDuckGo),
            "searx" => return Some(SearchProvider::SearXng),
            _ => {}
        }
        Self::ALL.into_iter().find(|provider| provider.as_str() == name)
    }

    pub fn requires_api_key(self) -> bool {
        matches!(
            self,
            SearchProvider::Google | SearchProvider::Brave | SearchProvider::Tavily
        )
    }
}

/// Web search settings stored in the voidrecon config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebSearchConfig {
    #[serde(default)]
    pub provider: SearchProvider,
    /// Key of the selected provider in configs written before `api_keys` existed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// API keys for Google / Brave / Tavily, kept when switching providers.
    #[serde(default)]
    pub api_keys: BTreeMap<SearchProvider, String>,
    /// Google Programmable Search engine id (`cx`).
    #[serde(default)]
    pub google_cx: String,
    /// Base URL of a SearXNG instance (JSON output enabled), e.g. http://localhost:8888.
    #[serde(default)]
    pub searxng_url: String,
}

impl WebSearchConfig {
    pub fn key_for(&self, provider: SearchProvider) -> &str {
        self.api_keys
            .get(&provider)
            .map(String::as_str)
            .filter(|key| !key.trim().is_empty())
            .unwrap_or(if provider == self.provider { self.api_key.as_str() } else { "" })
            .trim()
    }

    /// Whether `provider` has everything it needs to run.
    pub fn is_configured(&self, provider: SearchProvider) -> bool {
        match provider {
            SearchProvider::DuckDuckGo => true,
            SearchProvider::SearXng => !self.searxng_url.trim().is_empty(),
            SearchProvider::Google => {
                !self.key_for(provider).is_empty() && !self.google_cx.trim().is_empty()
            }
            SearchProvider::Brave | SearchProvider::Tavily => !self.key_for(provider).is_empty(),
        }
    }

    /// The selected provider first, then every other configured one as a
    /// fallback (DuckDuckGo last, since it blocks automated searches).
    pub fn provider_order(&self) -> Vec<SearchProvider> {
        let mut order = vec![self.provider];
        order.extend(SearchProvider::ALL.into_iter().filter(|provider| {
            *provider != self.provider
                && *provider != SearchProvider::DuckDuckGo
                && self.is_configured(*provider)
        }));
        if self.provider != SearchProvider::DuckDuckGo {
            order.push(SearchProvider::DuckDuckGo);
        }
        order
    }
}

const NO_PROVIDER_HINT: &str = "Configure a search provider with :websearch: brave <api_key> or tavily <api_key> (both have free plans), google <api_key> <cx>, or searxng <url> for your own SearXNG instance (no key).";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Results of a search and the provider that produced them.
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub provider: SearchProvider,
    pub results: Vec<SearchResult>,
    /// Providers tried before it, with their errors.
    pub failed: Vec<String>,
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to create HTTP client")
}

/// Runs a web search with the selected provider, falling back to the other
/// configured providers when it fails.
pub async fn search(config: &WebSearchConfig, query: &str, max_results: usize) -> Result<SearchOutcome> {
    let query = query.trim();
    if query.is_empty() {
        bail!("Search query is empty");
    }
    let max_results = max_results.clamp(1, 10);
    let http = client(SEARCH_TIMEOUT)?;
    let mut failed = Vec::new();
    for provider in config.provider_order() {
        if !config.is_configured(provider) {
            failed.push(format!("{}: not configured", provider.as_str()));
            continue;
        }
        let attempt = match provider {
            SearchProvider::DuckDuckGo => search_duckduckgo(&http, query).await,
            SearchProvider::Google => search_google(&http, config, query, max_results).await,
            SearchProvider::Brave => search_brave(&http, config, query, max_results).await,
            SearchProvider::Tavily => search_tavily(&http, config, query, max_results).await,
            SearchProvider::SearXng => search_searxng(&http, config, query).await,
        };
        match attempt {
            Ok(mut results) => {
                results.truncate(max_results);
                return Ok(SearchOutcome { provider, results, failed });
            }
            Err(error) => failed.push(format!("{}: {:#}", provider.as_str(), error)),
        }
    }
    bail!("Web search failed ({}). {}", failed.join("; "), NO_PROVIDER_HINT)
}

async fn get_json(request: reqwest::RequestBuilder, provider: &str) -> Result<Value> {
    let response = request
        .send()
        .await
        .with_context(|| format!("{} search request failed", provider))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{} search returned HTTP {}: {}", provider, status, body.chars().take(300).collect::<String>());
    }
    serde_json::from_str(&body).with_context(|| format!("{} returned invalid JSON", provider))
}

fn results_from(items: Option<&Value>, title: &str, url: &str, snippet: &str) -> Vec<SearchResult> {
    let field = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    items
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| SearchResult {
                    title: field(item, title),
                    url: field(item, url),
                    snippet: field(item, snippet),
                })
                .filter(|result| !result.url.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn search_google(
    http: &reqwest::Client,
    config: &WebSearchConfig,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>> {
    let num = max_results.to_string();
    let request = http.get("https://www.googleapis.com/customsearch/v1").query(&[
        ("key", config.key_for(SearchProvider::Google)),
        ("cx", config.google_cx.trim()),
        ("q", query),
        ("num", num.as_str()),
    ]);
    let json = get_json(request, "Google").await?;
    Ok(results_from(json.get("items"), "title", "link", "snippet"))
}

async fn search_brave(
    http: &reqwest::Client,
    config: &WebSearchConfig,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>> {
    let count = max_results.to_string();
    let request = http
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("Accept", "application/json")
        .header("X-Subscription-Token", config.key_for(SearchProvider::Brave))
        .query(&[("q", query), ("count", count.as_str())]);
    let json = get_json(request, "Brave").await?;
    let items = json.get("web").and_then(|web| web.get("results"));
    let mut results = results_from(items, "title", "url", "description");
    for result in &mut results {
        result.snippet = html_to_text(&result.snippet);
    }
    Ok(results)
}

async fn search_tavily(
    http: &reqwest::Client,
    config: &WebSearchConfig,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>> {
    let request = http
        .post("https://api.tavily.com/search")
        .bearer_auth(config.key_for(SearchProvider::Tavily))
        .json(&serde_json::json!({ "query": query, "max_results": max_results }));
    let json = get_json(request, "Tavily").await?;
    Ok(results_from(json.get("results"), "title", "url", "content"))
}

async fn search_duckduckgo(http: &reqwest::Client, query: &str) -> Result<Vec<SearchResult>> {
    let response = http
        .post("https://html.duckduckgo.com/html/")
        .form(&[("q", query)])
        .send()
        .await
        .context("DuckDuckGo search request failed")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("DuckDuckGo returned HTTP {}", status);
    }
    let results = html::parse_duckduckgo_results(&body);
    if results.is_empty() && body.contains("anomaly") {
        bail!("DuckDuckGo blocks automated searches from this connection");
    }
    Ok(results)
}

/// A SearXNG instance with `json` enabled in `search.formats` of its settings.yml.
async fn search_searxng(
    http: &reqwest::Client,
    config: &WebSearchConfig,
    query: &str,
) -> Result<Vec<SearchResult>> {
    let base = config.searxng_url.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(&format!("{}/search", base)).context("Invalid SearXNG URL")?;
    let request = http.get(url).query(&[("q", query), ("format", "json")]);
    let json = get_json(request, "SearXNG").await.map_err(|error| {
        if error.to_string().contains("403") {
            error.context("enable the json format under search.formats in SearXNG settings.yml")
        } else {
            error
        }
    })?;
    Ok(results_from(json.get("results"), "title", "url", "content"))
}

/// Text of a fetched web page.
#[derive(Debug, Clone, Serialize)]
pub struct FetchedPage {
    pub url: String,
    pub content_type: String,
    pub total_chars: usize,
    pub start_char: usize,
    pub text: String,
}

/// Downloads a public web page and returns its readable text, starting at
/// `start_char` and limited to `max_chars`.
pub async fn fetch(url: &str, start_char: usize, max_chars: usize) -> Result<FetchedPage> {
    let parsed = reqwest::Url::parse(url.trim()).context("Invalid URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Only http and https URLs can be fetched");
    }
    ensure_public_host(&parsed).await?;

    // Redirects are followed manually so every hop gets the same host check.
    let http = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Failed to create HTTP client")?;
    let mut current = parsed;
    let mut response = None;
    for _ in 0..5 {
        let reply = http.get(current.clone()).send().await.context("Request failed")?;
        if reply.status().is_redirection() {
            let location = reply
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .context("Redirect without a location")?;
            current = current.join(location).context("Invalid redirect location")?;
            if !matches!(current.scheme(), "http" | "https") {
                bail!("Redirect to a non-http URL was blocked");
            }
            ensure_public_host(&current).await?;
            continue;
        }
        response = Some(reply);
        break;
    }
    let mut response = response.context("Too many redirects")?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP {} from {}", status, current);
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let lower_type = content_type.to_lowercase();
    if !(lower_type.is_empty()
        || lower_type.contains("text")
        || lower_type.contains("json")
        || lower_type.contains("xml"))
    {
        bail!("Unsupported content type '{}' (only text pages are read)", content_type);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("Failed to read page")? {
        body.extend_from_slice(&chunk);
        if body.len() >= FETCH_MAX_BYTES {
            body.truncate(FETCH_MAX_BYTES);
            break;
        }
    }
    let raw = String::from_utf8_lossy(&body);
    let text = if lower_type.contains("html") || raw.trim_start().starts_with('<') {
        html_to_text(&raw)
    } else {
        raw.trim().to_string()
    };

    let total_chars = text.chars().count();
    let start_char = start_char.min(total_chars);
    let page_text: String = text.chars().skip(start_char).take(max_chars).collect();
    Ok(FetchedPage {
        url: current.to_string(),
        content_type,
        total_chars,
        start_char,
        text: page_text,
    })
}

/// Rejects local, private and otherwise non-public destinations: target
/// systems are reached through approved commands, not through `fetch_url`.
async fn ensure_public_host(url: &reqwest::Url) -> Result<()> {
    let host = url.host_str().context("URL has no host")?;
    let lower = host.to_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || !lower.contains('.') && lower.parse::<IpAddr>().is_err() && !lower.starts_with('[')
    {
        bail!("Fetching local or internal hosts is not allowed: {}", host);
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<IpAddr> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
        .await
        .with_context(|| format!("Could not resolve {}", host))?
        .map(|address| address.ip())
        .collect();
    if addresses.is_empty() {
        bail!("Could not resolve {}", host);
    }
    if let Some(address) = addresses.iter().find(|address| !is_public_ip(address)) {
        bail!("{} resolves to a non-public address ({}); use an approved command for in-scope hosts", host, address);
    }
    Ok(())
}

fn is_public_ip(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT / VPN ranges
                || a >= 224)
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80 // link local
                || ip.to_ipv4_mapped().is_some_and(|v4| !is_public_ip(&IpAddr::V4(v4))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_local_addresses_are_not_public() {
        for address in ["10.10.11.5", "127.0.0.1", "192.168.1.1", "172.16.0.1", "100.64.0.1", "169.254.1.1", "::1", "fd00::1"] {
            let ip: IpAddr = address.parse().unwrap_or(IpAddr::from([0, 0, 0, 0]));
            assert!(!is_public_ip(&ip), "{address} must not be public");
        }
        for address in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            let ip: IpAddr = address.parse().unwrap_or(IpAddr::from([0, 0, 0, 0]));
            assert!(is_public_ip(&ip), "{address} is public");
        }
    }

    #[tokio::test]
    async fn fetch_rejects_local_hosts_and_other_schemes() {
        for url in ["http://localhost/", "http://127.0.0.1:8080/", "http://10.0.0.5/", "ftp://example.com/", "http://intranet/"] {
            assert!(fetch(url, 0, 100).await.is_err(), "{url} must be rejected");
        }
    }

    #[test]
    fn configured_providers_are_tried_in_order_with_duckduckgo_last() {
        let mut config = WebSearchConfig::default();
        assert_eq!(config.provider_order(), vec![SearchProvider::DuckDuckGo]);

        config.api_keys.insert(SearchProvider::Tavily, "tvly-key".to_string());
        config.searxng_url = "http://localhost:8888".to_string();
        config.provider = SearchProvider::Brave;
        config.api_keys.insert(SearchProvider::Brave, "brave-key".to_string());
        assert_eq!(
            config.provider_order(),
            vec![
                SearchProvider::Brave,
                SearchProvider::Tavily,
                SearchProvider::SearXng,
                SearchProvider::DuckDuckGo
            ]
        );
        assert!(!config.is_configured(SearchProvider::Google), "no key or cx");
    }

    #[test]
    fn legacy_single_key_config_still_works() {
        let config: WebSearchConfig =
            serde_json::from_str(r#"{"provider":"brave","api_key":"old-key","google_cx":""}"#)
                .expect("old config");
        assert_eq!(config.key_for(SearchProvider::Brave), "old-key");
        assert_eq!(config.key_for(SearchProvider::Tavily), "");
        let saved = serde_json::to_string(&config).expect("serialize");
        let reloaded: WebSearchConfig = serde_json::from_str(&saved).expect("reload");
        assert_eq!(reloaded.key_for(SearchProvider::Brave), "old-key");
    }

    #[test]
    fn provider_names_round_trip() {
        for provider in SearchProvider::ALL {
            assert_eq!(SearchProvider::from_name(provider.as_str()), Some(provider));
        }
        assert_eq!(SearchProvider::from_name("DDG"), Some(SearchProvider::DuckDuckGo));
        assert_eq!(SearchProvider::from_name("bing"), None);
    }
}

/// Live network check, run manually: `cargo test --lib live_web -- --ignored --nocapture`.
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_web_search_and_fetch() {
        let outcome = search(&WebSearchConfig::default(), "CVE-2021-41773", 5)
            .await
            .expect("search");
        let results = outcome.results;
        for result in &results {
            println!("{} | {} | {}", result.title, result.url, result.snippet);
        }
        assert!(!results.is_empty());
        let mut readable = 0;
        for result in &results {
            match fetch(&result.url, 0, 600).await {
                Ok(page) => {
                    println!("--- {} ({} chars)\n{}", page.url, page.total_chars, page.text);
                    readable += usize::from(page.total_chars > 0);
                }
                Err(error) => println!("--- {} failed: {:#}", result.url, error),
            }
        }
        assert!(readable > 0);
        let api = fetch("https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2021-41773", 0, 400)
            .await
            .expect("nvd api");
        println!("--- NVD API ({} chars)\n{}", api.total_chars, api.text);
    }
}
