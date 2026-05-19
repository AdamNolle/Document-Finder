//! Embedded SearXNG-compatible local search server.
//!
//! Tauri spins this up once at startup on `127.0.0.1:<ephemeral-port>`. It
//! exposes the slice of SearXNG's HTTP surface that document-finder cares
//! about (`GET /search?q=…&format=json`, `GET /healthz`, `GET /config`) and
//! is internally backed by the existing `crate::sources::meta_search`
//! aggregator. No Python, no Docker, no sidecar binary — the whole server
//! is compiled into the Tauri executable.
//!
//! Response shape matches upstream SearXNG so any SearXNG-compatible client
//! (the app's own `LocalSearxngSource`, curl, third-party tooling) can
//! consume it interchangeably.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::Json,
    routing::get,
    Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::sources::meta_search::MetaSearchSource;
use crate::sources::{make_client, Source};

#[derive(Clone)]
struct ServerState {
    client: Arc<reqwest::Client>,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    /// Accepted but not used — we always return JSON. Present so SearXNG
    /// clients that always send `format=json` work without complaint.
    #[serde(default)]
    #[allow(dead_code)]
    format: Option<String>,
    /// Hard cap on results. Defaults to 30, max 100.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct SearchResultRow {
    url: String,
    title: String,
    /// SearXNG calls this `content` — it's the snippet / abstract.
    content: String,
    /// Originating engine (e.g. "duckduckgo", "brave"). For document-finder
    /// the source string is often `meta_search/<engine>` — strip that prefix
    /// so consumers see the same value SearXNG would emit.
    engine: String,
    /// Best-effort relevance score in [0, 1]. Currently constant per row
    /// because meta_search doesn't expose a per-doc score, but the field is
    /// here so consumers can rank if they want.
    score: f32,
    /// Optional structured fields if the upstream source surfaced them.
    /// `publishedDate` is camelCase intentionally — that's SearXNG's wire
    /// format, and tools that consume SearXNG expect it spelled exactly so.
    #[serde(skip_serializing_if = "Option::is_none")]
    publishedDate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
}

#[derive(Serialize)]
struct SearchResponse {
    query: String,
    number_of_results: usize,
    results: Vec<SearchResultRow>,
}

#[derive(Serialize)]
struct ConfigResponse {
    /// Mirrors SearXNG's `/config` endpoint just enough that clients
    /// probing for a SearXNG install see the right signal.
    instance_name: &'static str,
    version: &'static str,
    /// `embedded` distinguishes us from a real SearXNG; clients that don't
    /// care still get a valid JSON body.
    backend: &'static str,
}

async fn healthz() -> &'static str {
    "ok"
}

async fn config() -> Json<ConfigResponse> {
    Json(ConfigResponse {
        instance_name: "document-finder embedded",
        version: env!("CARGO_PKG_VERSION"),
        backend: "embedded",
    })
}

async fn search(
    State(s): State<ServerState>,
    Query(p): Query<SearchParams>,
) -> Json<SearchResponse> {
    let q = p.q.trim().to_string();
    if q.is_empty() {
        return Json(SearchResponse {
            query: q,
            number_of_results: 0,
            results: Vec::new(),
        });
    }
    let limit = p.limit.unwrap_or(30).clamp(1, 100);
    let keywords = q.split_whitespace().map(String::from).collect::<Vec<_>>();

    // App handle is None — the server does not emit Tauri frontend events
    // (those are reserved for user-driven runs). Backend logging via
    // `tracing` still fires inside meta_search.
    let meta = MetaSearchSource::new(s.client.clone(), None);
    let mut stream = meta.search(keywords, limit).await;

    let mut results = Vec::with_capacity(limit);
    while let Some(item) = stream.next().await {
        if let Ok(doc) = item {
            let engine = doc
                .source
                .strip_prefix("meta_search/")
                .unwrap_or(doc.source.as_str())
                .to_string();
            results.push(SearchResultRow {
                url: doc.url,
                title: doc.title,
                content: doc.abstract_.unwrap_or_default(),
                engine,
                score: 1.0,
                publishedDate: doc.year,
                author: if doc.authors.is_empty() {
                    None
                } else {
                    Some(doc.authors.join(", "))
                },
            });
            if results.len() >= limit {
                break;
            }
        }
    }

    Json(SearchResponse {
        number_of_results: results.len(),
        query: q,
        results,
    })
}

/// Boot the embedded server. Returns the bound address; the actual
/// `Router::serve` future runs forever on a detached task so the caller
/// gets back control immediately. If startup fails (port exhausted, etc.)
/// the returned error short-circuits so we don't claim a URL we can't
/// honor.
pub async fn start() -> std::io::Result<SocketAddr> {
    let client = Arc::new(make_client());
    let state = ServerState { client };

    let app = Router::new()
        .route("/search", get(search))
        .route("/healthz", get(healthz))
        .route("/config", get(config))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tracing::info!("embedded searxng-compat server listening on http://{addr}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("embedded searxng server stopped: {e}");
        }
    });

    Ok(addr)
}
