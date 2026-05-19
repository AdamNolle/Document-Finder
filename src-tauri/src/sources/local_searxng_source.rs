//! `searxng` source — calls a SearXNG-compatible JSON endpoint.
//!
//! Defaults to the embedded server (`engine::local_searxng`) so the source
//! works out-of-the-box without Docker. Falls through to any
//! `instance_url` the user supplied in SettingsView when present (lets
//! power-users point at a real SearXNG they're running elsewhere).

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

use super::{Document, Source};

const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

pub struct LocalSearxngSource {
    client: Arc<reqwest::Client>,
    /// Explicit instance URL from the frontend. When `None` we resolve to
    /// the embedded server URL at request time so the source picks up the
    /// running port even on the first invocation after startup.
    explicit_url: Option<String>,
}

impl LocalSearxngSource {
    pub fn new(client: Arc<reqwest::Client>, explicit_url: Option<String>) -> Self {
        let cleaned = explicit_url.and_then(|u| {
            let t = u.trim();
            // Empty / sentinel / legacy default → defer to embedded.
            if t.is_empty()
                || t.eq_ignore_ascii_case("embedded")
                || t.eq_ignore_ascii_case("embedded://local")
                || t.eq_ignore_ascii_case("http://localhost:8080")
            {
                None
            } else {
                Some(t.trim_end_matches('/').to_string())
            }
        });
        Self {
            client,
            explicit_url: cleaned,
        }
    }

    /// Resolve the URL to hit at request time. Embedded address might not
    /// have been claimed by `OnceCell` yet if `start_run` fires
    /// immediately after launch.
    fn resolve_url(&self) -> Option<String> {
        if let Some(u) = &self.explicit_url {
            return Some(u.clone());
        }
        crate::EMBEDDED_SEARXNG_ADDR
            .get()
            .map(|addr| format!("http://{addr}"))
    }
}

#[derive(Deserialize)]
struct SearxResp {
    #[serde(default)]
    results: Vec<SearxResultRow>,
}

#[derive(Deserialize)]
struct SearxResultRow {
    url: String,
    title: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(rename = "publishedDate", default)]
    published_date: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

#[async_trait]
impl Source for LocalSearxngSource {
    fn name(&self) -> &'static str {
        "searxng"
    }

    async fn search(
        &self,
        keywords: Vec<String>,
        limit: usize,
    ) -> BoxStream<'static, anyhow::Result<Document>> {
        let Some(base) = self.resolve_url() else {
            // Server hasn't bound yet — surface as a soft error so the UI
            // shows it in the issues panel instead of hanging silently.
            return stream::iter(vec![Err(anyhow::anyhow!(
                "embedded SearXNG server not yet ready — retry in a moment"
            ))])
            .boxed();
        };
        let query = keywords.join(" ");
        if query.trim().is_empty() {
            return stream::empty().boxed();
        }

        let search_url = format!("{base}/search");
        let client = self.client.clone();
        let take_n = limit.max(1);

        let docs = match tokio::time::timeout(
            QUERY_TIMEOUT,
            client
                .get(&search_url)
                .query(&[
                    ("q", query.as_str()),
                    ("format", "json"),
                    ("limit", &take_n.to_string()),
                ])
                .send(),
        )
        .await
        {
            Err(_) => {
                return stream::iter(vec![Err(anyhow::anyhow!(
                    "SearXNG request timed out after {}s",
                    QUERY_TIMEOUT.as_secs()
                ))])
                .boxed();
            }
            Ok(Err(e)) => {
                return stream::iter(vec![Err(anyhow::anyhow!(
                    "SearXNG request to {base} failed: {e}"
                ))])
                .boxed();
            }
            Ok(Ok(resp)) => match resp.json::<SearxResp>().await {
                Ok(body) => body
                    .results
                    .into_iter()
                    .take(take_n)
                    .map(|r| Document {
                        title: r.title,
                        url: r.url,
                        source: "searxng".to_string(),
                        authors: r
                            .author
                            .map(|a| a.split(',').map(|s| s.trim().to_string()).collect())
                            .unwrap_or_default(),
                        year: r.published_date,
                        abstract_: r.content,
                        identifier: r.engine,
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    return stream::iter(vec![Err(anyhow::anyhow!(
                        "SearXNG returned non-JSON body: {e}"
                    ))])
                    .boxed();
                }
            },
        };

        stream::iter(docs.into_iter().map(Ok)).boxed()
    }
}
