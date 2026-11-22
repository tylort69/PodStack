//! The Tokio runtime ESI work runs on, and helpers to await it from GTK code.

use std::future::Future;
use std::sync::LazyLock;

pub static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("podstack-rt").build().expect("tokio runtime")
});

/// Runs `fut` on Tokio; the returned future can be awaited on the GTK main loop.
pub fn spawn<F>(fut: F) -> impl Future<Output = F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = RUNTIME.spawn(fut);
    async move { handle.await.expect("background task panicked") }
}

/// Runs `fut` on Tokio, then `done` on the GTK main thread with its result.
///
/// ```ignore
/// rt::run(async move { core.market.plex_price(&core.esi, false).await }, move |price| label.set_text(..));
/// ```
pub fn run<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static, done: impl FnOnce(T) + 'static) {
    gtk::glib::spawn_future_local(async move {
        let v = spawn(fut).await;
        done(v);
    });
}
