use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use fn_error_context::context;
use notify::Watcher;
use tokio::fs;
use tokio::sync::watch;
use tokio::sync::Notify;

#[context("failed to watch file `{}`", path.as_ref().display())]
pub(crate) async fn watch_file(
    path: impl AsRef<Path>,
) -> anyhow::Result<watch::Receiver<anyhow::Result<Arc<str>>>> {
    let path = <Arc<Path>>::from(fs::canonicalize(path.as_ref()).await?);

    let initial_contents = <Arc<str>>::from(fs::read_to_string(&path).await?);

    let modified = Arc::new(Notify::new());
    let mut watcher = notify::recommended_watcher({
        let path = path.clone();
        let modified = modified.clone();
        move |event: notify::Result<notify::Event>| {
            let event = match event {
                Ok(event) => event,
                Err(e) => {
                    log::error!("{:?}", anyhow!(e).context("failed to watch file"));
                    return;
                }
            };
            if !event.paths.iter().any(|p| *p == *path) {
                return;
            }
            if let notify::EventKind::Access(_) = event.kind {
                return;
            }
            modified.notify_one();
        }
    })?;
    let dir = path.parent().context("file has no parent")?;
    watcher.watch(&dir, notify::RecursiveMode::Recursive)?;

    let (sender, receiver) = watch::channel(Ok(initial_contents.clone()));

    tokio::spawn(async move {
        let mut previous_contents = Some(initial_contents);
        loop {
            modified.notified().await;

            let res = fs::read_to_string(&path)
                .await
                .context("failed to read file");

            let same = matches!(
                (&res, &previous_contents),
                (Ok(contents), Some(previous_contents)) if **contents == **previous_contents
            );
            if same {
                continue;
            }

            let res = res.map(<Arc<str>>::from);
            previous_contents = res.as_ref().ok().cloned();
            if sender.send(res).is_err() {
                break;
            }
        }
        drop(watcher);
    });

    Ok(receiver)
}
