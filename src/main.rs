use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt::{self, Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{anyhow, Context as _};
use async_stream::try_stream;
use clap::Parser;
use hyper::http;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::sync::Notify;
use tokio::{fs, signal};

mod watcher;

mod renderer;
use renderer::{RateLimited, Renderer};

mod templater;
use templater::{Liveness, Templater, Theme};

#[derive(Parser)]
#[clap(about = "GitHub Markdown previewer")]
#[clap(group(clap::ArgGroup::new("action").args(&["port", "output"])))]
struct Args {
    /// The markdown file to render.
    #[clap(parse(from_os_str))]
    input: PathBuf,

    /// The authorization token to use. You can create a personal one at
    /// <https://github.com/settings/tokens>.
    #[clap(short, long, env = "GITHUB_TOKEN")]
    token: String,

    /// The theme to generate the resulting page using.
    #[clap(long, arg_enum, ignore_case = true, default_value_t)]
    theme: Theme,

    /// The title of the page. Defaults to the filename.
    #[clap(long)]
    title: Option<String>,

    /// The port the server should bind to.
    #[clap(short, long, default_value = "39131")]
    port: u16,

    /// The HTML file to generate. If this is specified, no server will be started and instead a
    /// single static file will be produced.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::env::set_var("RUST_LOG", "INFO");
    pretty_env_logger::init();

    let args = Args::parse();

    let renderer = Renderer::new(reqwest::Client::new(), args.token);
    let templater = Templater::new(
        args.title
            .map(String::into_boxed_str)
            .unwrap_or_else(|| args.input.to_string_lossy().into()),
        args.theme,
    );

    if let Some(output) = args.output {
        gen_output(&args.input, renderer, templater, &output).await?;
    } else {
        run_server(&args.input, renderer, templater, args.port).await?;
    }

    Ok(())
}

async fn gen_output(
    input: &Path,
    renderer: Renderer,
    templater: Templater,
    output: &Path,
) -> anyhow::Result<()> {
    let markdown = fs::read_to_string(input).await?;
    let rendered = renderer.render(&markdown).await??;
    let page = templater.generate(&rendered, Liveness::Static).await?;
    if output.to_str() == Some("-") {
        print!("{}", page);
    } else {
        fs::write(output, page)
            .await
            .context("could not write to output file")?;
    }
    Ok(())
}

async fn run_server(
    input: &Path,
    renderer: Renderer,
    templater: Templater,
    port: u16,
) -> anyhow::Result<()> {
    let server = Arc::new(Server {
        renderer,
        templater,
        watcher: watcher::watch_file(&input).await?,
        shutdown: Notify::new(),
    });

    let http = Http::new();
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
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
    templater: Templater,
    watcher: watch::Receiver<anyhow::Result<Arc<str>>>,
    shutdown: Notify,
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
        let res: anyhow::Result<_> = async move {
            let markdown = self.watcher.borrow().as_ref().map_err(clone_error)?.clone();

            let rendered = match self.renderer.render(&markdown).await? {
                Ok(rendered) => rendered,
                Err(rate_limited) => {
                    // TODO: handle errors better
                    return Ok(http::Response::builder()
                        .status(http::StatusCode::FORBIDDEN)
                        .header("Content-Type", "text/plain")
                        .body(hyper::Body::from(format!(
                            "\
                                Rate Limited\n\
                                ============\n\
                                \
                                {}
                            ",
                            rate_limited,
                        )))
                        .unwrap());
                }
            };

            let page = self.templater.generate(&rendered, Liveness::Live).await?;

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
                            \
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
