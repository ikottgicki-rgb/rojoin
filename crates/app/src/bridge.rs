//! The one place async work crosses into the UI thread.
//!
//! Slint's event loop is not `Send` and must only be touched from the thread
//! that owns it. Everything network-shaped runs on a multi-threaded tokio
//! runtime on a side thread, and results come back through
//! `upgrade_in_event_loop`. Keeping that in a single helper means no screen has
//! to think about threading.
//!
//! Two variants exist deliberately:
//!   * `call`  — errors become a toast (or the expired-session banner). Use for
//!               fire-and-display work.
//!   * `call_res` — hands the whole `Result` to the closure. Use anywhere a
//!               busy flag or optimistic UI must be reconciled on *both*
//!               success and failure, or the UI gets stuck spinning forever.

use std::future::Future;
use std::sync::Arc;

use slint::ComponentHandle;
use tokio::runtime::Runtime;

use crate::MainWindow;

pub struct Bridge {
    rt: Arc<Runtime>,
    ui: slint::Weak<MainWindow>,
}

impl Bridge {
    pub fn new(ui: &MainWindow) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;

        Ok(Self { rt: Arc::new(rt), ui: ui.as_weak() })
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.rt
    }

    /// Run `op`, then `on_ok` on the UI thread. Errors are reported centrally.
    pub fn call<T, F, Fut, G>(&self, op: F, on_ok: G)
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = rojoin_roblox::Result<T>> + Send,
        G: FnOnce(MainWindow, T) + Send + 'static,
    {
        let ui = self.ui.clone();
        self.rt.spawn(async move {
            let result = op().await;
            let _ = ui.upgrade_in_event_loop(move |handle| match result {
                Ok(value) => on_ok(handle, value),
                Err(e) => report(&handle, e),
            });
        });
    }

    /// Run `op` and hand the whole `Result` to `on_done` on the UI thread.
    pub fn call_res<T, F, Fut, G>(&self, op: F, on_done: G)
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = rojoin_roblox::Result<T>> + Send,
        G: FnOnce(MainWindow, rojoin_roblox::Result<T>) + Send + 'static,
    {
        let ui = self.ui.clone();
        self.rt.spawn(async move {
            let result = op().await;
            let _ = ui.upgrade_in_event_loop(move |handle| on_done(handle, result));
        });
    }

    /// Fire-and-forget background work that never touches the UI.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(fut);
    }
}

/// Central error handling.
///
/// An expired session is a *state*, not an incident: it raises a persistent
/// banner rather than a toast that vanishes after three seconds and leaves the
/// user with a broken app and no explanation.
pub fn report(ui: &MainWindow, err: rojoin_roblox::Error) {
    match err {
        rojoin_roblox::Error::Expired => {
            tracing::warn!("session expired");
            ui.set_session_expired(true);
        }
        other => {
            tracing::error!(error = %other, "request failed");
        }
    }
}
