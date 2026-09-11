//! The bridge between GTK's main loop and tokio.
//!
//! SPEC §6: no widget ever awaits the network. Work that talks to Google runs on a tokio
//! runtime, and its result is handed back on the GTK thread, where it is safe to touch
//! widgets.

use std::future::Future;
use std::sync::OnceLock;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use gtk::glib;

/// How often the main loop checks whether a background task has finished.
///
/// ponytail: polling, rather than an async channel that would wake the loop exactly once.
/// `async-channel` is not in the approved dependency set, and a 50 ms poll that runs only
/// while a task is in flight costs nothing measurable. Swap it in if a task ever needs a
/// tighter turnaround than a fifth of a frame.
const POLL: Duration = Duration::from_millis(50);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Runtime::new().expect("the tokio runtime could not be started")
    })
}

/// Run `future` off the main thread, then call `on_done` back on it.
///
/// `on_done` is not `Send` on purpose: it runs on the GTK thread and is expected to touch
/// widgets. `future` is, because it does not.
pub fn spawn<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
    on_done: impl FnOnce(T) + 'static,
) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    runtime().spawn(async move {
        // A send that fails means the window went away while the work was in flight, which
        // is not an error — there is simply no longer anyone to tell.
        let _ = sender.send(future.await);
    });

    let mut on_done = Some(on_done);
    glib::timeout_add_local(POLL, move || match receiver.try_recv() {
        Ok(value) => {
            if let Some(on_done) = on_done.take() {
                on_done(value);
            }
            glib::ControlFlow::Break
        }
        Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        // The task panicked or was dropped. Stop polling rather than spin forever.
        Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}
