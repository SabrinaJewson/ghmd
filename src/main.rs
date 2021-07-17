use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt::{self, Debug, Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context as _};
use async_stream::try_stream;
use hyper::http;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use serde::Serialize;
use structopt::StructOpt;
use tera::Tera;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;
use tokio::sync::Notify;

mod watcher;

mod renderer;
use renderer::{RateLimited, Renderer};

#[derive(StructOpt)]
#[structopt(name = "ghmd", about = "GitHub Markdown previewer")]
struct Opts {
    /// The markdown file to render.
    #[structopt(parse(from_os_str))]
    input: PathBuf,

    /// The authorization token to use. You can create a personal one at
    /// <https://github.com/settings/tokens>.
    #[structopt(short, long)]
    token: String,

    /// The theme to generate the resulting page using.
    #[structopt(long, possible_values = &["dark", "light"], default_value = "dark", case_insensitive = true)]
    theme: String,

    /// The title of the page. Defaults to the filename.
    #[structopt(long)]
    title: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::env::set_var("RUST_LOG", "INFO");
    pretty_env_logger::init();

    let opts = Opts::from_args();

    let mut template = Tera::default();
    template.autoescape_on(Vec::new());
    template.add_raw_template("html", include_str!("template.html"))?;

    let server = Arc::new(Server {
        renderer: Renderer::new(reqwest::Client::new(), opts.token),
        watcher: watcher::watch_file(&opts.input).await?,
        shutdown: Notify::new(),
        title: match opts.title {
            Some(title) => title.into(),
            None => opts.input.to_string_lossy().into(),
        },
        template,
        theme: Box::from(opts.theme),
    });

    let http = Http::new();
    let listener = TcpListener::bind("0.0.0.0:39131")
        .await
        .context("failed to bind server")?;

    log::info!(
        "Now listening on http://localhost:{}/",
        listener.local_addr()?.port()
    );

    let server_task = tokio::spawn({
        let server = server.clone();
        async move {
            loop {
                let (connection, _address) = match listener.accept().await {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("{:?}", anyhow!(e).context("failed to accept connection"));
                        continue;
                    }
                };
                let connection = http.serve_connection(
                    connection,
                    service_fn({
                        let server = server.clone();
                        move |req| {
                            let server = server.clone();
                            async move { Ok::<_, Infallible>(server.handle_request(req).await) }
                        }
                    }),
                );

                let server = server.clone();
                tokio::spawn(async move {
                    tokio::pin!(connection);
                    let res = tokio::select! {
                        res = &mut connection => { res }
                        _ = server.shutdown.notified() => {
                            connection.as_mut().graceful_shutdown();
                            connection.await
                        }
                    };
                    if let Err(e) = res {
                        log::error!("{:?}", anyhow!(e).context("failed to run connection"));
                    }
                });
            }
        }
    });

    signal::ctrl_c().await?;

    server.shutdown.notify_waiters();
    server_task.abort();

    Ok(())
}

struct Server {
    renderer: Renderer,
    watcher: watch::Receiver<anyhow::Result<Arc<str>>>,
    shutdown: Notify,
    title: Box<str>,
    theme: Box<str>,
    template: Tera,
}

impl Server {
    async fn handle_request(
        self: &Arc<Self>,
        req: http::Request<hyper::Body>,
    ) -> http::Response<hyper::Body> {
        if req
            .headers()
            .get("accept")
            .map_or(false, |val| val == "text/event-stream")
        {
            self.clone().event_stream().await
        } else {
            self.get().await
        }
    }

    async fn get(&self) -> hyper::Response<hyper::Body> {
        let res = async move {
            let markdown = match &*self.watcher.borrow() {
                Ok(markdown) => markdown.clone(),
                Err(e) => return Err(clone_error(e)),
            };

            let rendered = match self.renderer.render(&markdown).await? {
                Ok(rendered) => rendered,
                Err(RateLimited { limit, reset }) => {
                    let time = reset
                        .duration_since(SystemTime::now())
                        .unwrap_or_else(|_| Duration::default());

                    // TODO: handle errors better
                    return Ok(http::Response::builder()
                        .status(http::StatusCode::FORBIDDEN)
                        .header("Content-Type", "text/plain")
                        .body(hyper::Body::from(format!(
                            "\
                                Rate Limited\n\
                                ============\n\

                                You have used your quota of {} requests and are now rate limited\
                                by the GitHub API.\n\

                                You may continue to send requests in {:?}.\
                            ",
                            limit, time,
                        )))
                        .unwrap());
                }
            };

            #[derive(Serialize)]
            struct HtmlTemplateOpts<'a> {
                title: &'a str,
                content: &'a str,
                theme: &'a str,
                javascript: &'a str,
            }
            let page = self
                .template
                .render(
                    "html",
                    &tera::Context::from_serialize(HtmlTemplateOpts {
                        title: &self.title,
                        content: &rendered,
                        theme: &self.theme,
                        javascript: include_str!("template.js"),
                    })
                    .unwrap(),
                )
                .context("failed to render template")?;

            Ok(http::Response::builder()
                .status(http::StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(hyper::Body::from(page))
                .unwrap())
        }
        .await;

        res.unwrap_or_else(|e| {
            http::Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "text/plain")
                .body(hyper::Body::from(format!(
                    "\
                            Internal Server Error\n\
                            =====================\n\

                            {:?}\
                        ",
                    e,
                )))
                .unwrap()
        })
    }

    async fn event_stream(self: Arc<Self>) -> hyper::Response<hyper::Body> {
        let mut watcher = self.watcher.clone();
        let stream = hyper::Body::wrap_stream::<_, _, Infallible>(try_stream! {
            loop {
                if watcher.changed().await.is_err() {
                    return;
                }

                let res = match &*watcher.borrow_and_update() {
                    Ok(markdown) => Ok(markdown.clone()),
                    Err(e) => Err(format!("{:?}", e)),
                };

                let markdown = match res {
                    Ok(markdown) => markdown,
                    Err(e) => {
                        yield sse("render_error", &format!("{:?}", e));
                        continue
                    },
                };

                yield match self.renderer.render(&markdown).await {
                    Ok(Ok(rendered)) => sse("update", &rendered),
                    Ok(Err(RateLimited { limit, reset })) => {
                        #[derive(Serialize)]
                        struct MessageData {
                            limit: u32,
                            reset: u64,
                        }
                        let data = MessageData {
                            limit,
                            reset: reset.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
                        };
                        sse("rate_limited", &serde_json::to_string(&data).unwrap())
                    }
                    Err(e) => sse("render_error", &format!("{:?}", e)),
                };
            }
        });

        http::Response::builder()
            .status(http::StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(stream)
            .unwrap()
    }
}

fn sse(kind: &str, data: &str) -> String {
    let mut event = "event: ".to_owned();
    event.push_str(kind);
    if data.is_empty() {
        event.push_str("\ndata: ");
    } else {
        for line in data.lines() {
            event.push_str("\ndata: ");
            event.push_str(line);
        }
    }
    event.push_str("\n\n");
    event
}

fn clone_error(e: &anyhow::Error) -> anyhow::Error {
    struct ErrorData {
        debug: String,
        display: String,
        source: Option<Box<Self>>,
    }
    impl ErrorData {
        fn new(e: impl StdError) -> Self {
            Self {
                debug: format!("{:?}", e),
                display: e.to_string(),
                source: e.source().map(|e| Box::new(Self::new(e))),
            }
        }
    }
    impl Debug for ErrorData {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str(&self.debug)
        }
    }
    impl Display for ErrorData {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str(&self.display)
        }
    }
    impl StdError for ErrorData {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            self.source
                .as_deref()
                .map(|e| -> &(dyn StdError + 'static) { e })
        }
    }

    anyhow!(ErrorData::new(e.chain().next().unwrap()))
}
