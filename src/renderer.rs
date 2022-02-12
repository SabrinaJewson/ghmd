use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, ensure, Context as _};
use fn_error_context::context;
use once_cell::sync::Lazy;
use reqwest::header::HeaderValue;
use scraper::{node, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use tokio::runtime;
use tokio::sync::oneshot;
use tokio::sync::Mutex;

pub(crate) struct Renderer {
    client: reqwest::Client,
    token: Box<str>,
    cache: Mutex<HashMap<sha2::digest::Output<Sha512>, Arc<str>>>,
    octicons: Octicons,
}

impl Renderer {
    pub(crate) fn new(client: reqwest::Client, token: impl Into<Box<str>>) -> Self {
        Self {
            client: client.clone(),
            token: token.into(),
            cache: Mutex::new(HashMap::new()),
            octicons: Octicons::new(client),
        }
    }
    #[context("failed to render markdown")]
    pub(crate) async fn render(
        &self,
        markdown: &str,
    ) -> anyhow::Result<Result<Arc<str>, RateLimited>> {
        let hash = Sha512::digest(markdown.as_bytes());

        let mut cache = self.cache.lock().await;

        if let Some(data) = cache.get(&hash) {
            return Ok(Ok(data.clone()));
        }

        #[derive(Serialize)]
        struct Body<'a> {
            text: &'a str,
        }
        let res = self
            .client
            .post("https://api.github.com/markdown")
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", "markdown previewer")
            .bearer_auth(&self.token)
            .json(&Body { text: markdown })
            .send()
            .await?;

        let res = async {
            if res.status() == reqwest::StatusCode::FORBIDDEN {
                let limit: u32 = parse_header_value(
                    res.headers()
                        .get("X-RateLimit-Limit")
                        .context("no ratelimit limit header")?,
                )
                .context("ratelimit limit header was invalid")?;
                let reset: SystemTime = SystemTime::UNIX_EPOCH
                    + Duration::from_secs(
                        parse_header_value(
                            res.headers()
                                .get("X-RateLimit-Reset")
                                .context("no ratelimit reset header")?,
                        )
                        .context("ratelimit reset header was invalid")?,
                    );
                return Ok(Err(RateLimited { limit, reset }));
            }

            #[derive(Deserialize)]
            struct ErrorResponse {
                message: String,
            }
            if res.status().is_client_error() {
                bail!(res.json::<ErrorResponse>().await?.message);
            }

            ensure!(
                res.status().is_success(),
                "GitHub request failed with {}",
                res.status()
            );

            Ok(Ok(res.text().await?))
        }
        .await
        .context("GitHub API response was unexpected")?;

        let rendered = match res {
            Ok(rendered) => rendered,
            Err(e) => return Ok(Err(e)),
        };

        let rendered = self.octicons.populate(rendered).await;

        let rendered = <Arc<str>>::from(rendered);

        if cache.len() > 100 {
            cache.clear();
        }
        cache.insert(hash, rendered.clone());

        Ok(Ok(rendered))
    }
}

fn parse_header_value<T: FromStr>(value: &HeaderValue) -> anyhow::Result<T>
where
    T::Err: Send + Sync + std::error::Error + 'static,
{
    Ok(value.to_str()?.parse()?)
}

pub(crate) struct RateLimited {
    pub(crate) limit: u32,
    pub(crate) reset: SystemTime,
}

struct Octicons {
    client: reqwest::Client,
    cache: Mutex<HashMap<Box<str>, Arc<str>>>,
}

impl Octicons {
    fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn get(&self, name: &str) -> Option<Arc<str>> {
        let mut cache = self.cache.lock().await;

        if let Some(data) = cache.get(name) {
            return Some(data.clone());
        }

        let res = self
            .client
            .get(format!(
                "https://cdn.jsdelivr.net/gh/primer/octicons@14.2.2/icons/{}.svg",
                name
            ))
            .header("User-Agent", "markdown previewer")
            .send()
            .await
            .ok()?;

        if !res.status().is_success() {
            return None;
        }

        let svg = <Arc<str>>::from(res.text().await.ok()?);
        cache.insert(Box::from(name), svg.clone());
        Some(svg)
    }

    async fn populate(&self, html: String) -> String {
        let (required_icons_tx, required_icons_rx) = oneshot::channel::<Vec<String>>();
        let (icons_tx, icons_rx) = oneshot::channel::<Vec<Option<Arc<str>>>>();
        let (result_tx, result_rx) = oneshot::channel();

        tokio::task::spawn_blocking(move || {
            let mut html = Html::parse_fragment(&html);

            static SELECTOR: Lazy<Selector> =
                Lazy::new(|| Selector::parse("span.octicon").unwrap());
            let (octicon_spans, required_icons): (Vec<_>, Vec<_>) = html
                .select(&SELECTOR)
                .filter_map(|e| {
                    let required_icon = e
                        .value()
                        .classes()
                        .find_map(|c| c.strip_prefix("octicon-"))?;
                    Some((e.id(), format!("{}-16", required_icon)))
                })
                .unzip();

            required_icons_tx.send(required_icons).ok()?;

            let icons = runtime::Handle::current().block_on(icons_rx).ok()?;

            for (i, (&octicon_span, svg)) in octicon_spans.iter().zip(&icons).enumerate() {
                if svg.is_none() {
                    continue;
                }
                html.tree
                    .get_mut(octicon_span)
                    .unwrap()
                    .append(Node::Text(node::Text {
                        text: format!("__OCTICON{}__", i).into(),
                    }));
            }

            let html = html.root_element().inner_html();

            let mut parts = html.split("__OCTICON");
            let mut res = parts.next().unwrap().to_owned();

            for part in parts {
                let (num, rest) = part.split_once("__").unwrap();
                res.push_str(icons[num.parse::<usize>().unwrap()].as_deref().unwrap());
                res.push_str(rest);
            }

            result_tx.send(res).ok();

            Some(())
        });

        let required_icons = required_icons_rx.await.unwrap();
        let mut icons = Vec::with_capacity(required_icons.len());
        for required_icon in required_icons {
            icons.push(self.get(&required_icon).await);
        }
        icons_tx.send(icons).unwrap();

        result_rx.await.unwrap()
    }
}
