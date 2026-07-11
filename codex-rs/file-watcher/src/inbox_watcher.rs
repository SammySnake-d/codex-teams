//! Event-driven inbox change waiter for Codex Teams.
//!
//! # Why this exists (first principles)
//!
//! Monitoring is "be notified when state changes", NOT "keep asking whether
//! state changed". The lead and teammate inbox pollers originally slept on a
//! fixed interval (1s / 500ms) and re-read the mailbox every tick — a poll loop
//! whose cost is `frequency × read-cost` with most reads returning nothing.
//!
//! An inbox is a file at a deterministic path
//! (`teams/{team}/inboxes/{agent}.json`), so the OS itself can push a
//! notification the instant it changes (FSEvents on macOS, inotify on Linux).
//! [`InboxWatcher`] wraps [`crate::FileWatcher`] so a poller can `await` the
//! next change instead of spinning. Steady-state cost drops to the number of
//! ACTUAL writes; there is no empty polling.
//!
//! Debounce coalesces bursts (an atomic mailbox write is create+rename+chmod),
//! so a single logical write wakes the consumer once, not several times.
//!
//! # Fallback
//!
//! If the OS watcher cannot be created (rare: descriptor limits, unsupported
//! platform) the watcher degrades to an interval tick so behavior never
//! regresses below the old poll loop. Callers therefore treat a wake-up as
//! "maybe changed" and must still read + diff the inbox — exactly what they did
//! before. The watcher is a wake-up source, not a source of truth.

use std::path::PathBuf;
use std::time::Duration;

use crate::DebouncedWatchReceiver;
use crate::FileWatcher;
use crate::FileWatcherSubscriber;
use crate::WatchPath;
use crate::WatchRegistration;

/// Debounce window for coalescing a burst of raw FS events into one wake-up.
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(50);

/// Interval used only when the OS watcher is unavailable and the watcher falls
/// back to plain ticking. Matches the lead poller's historical cadence so the
/// degraded path is no worse than the original poll loop.
const FALLBACK_TICK: Duration = Duration::from_millis(1000);

/// Awaitable "the inbox may have changed" signal for one agent's mailbox.
///
/// Watches the enclosing `inboxes/` directory (watching the file directly would
/// miss atomic create-rename replacement, which swaps the inode). Callers loop:
/// `while watcher.changed().await { /* read + inject + ack */ }`.
pub struct InboxWatcher {
    kind: InboxWatcherKind,
}

enum InboxWatcherKind {
    /// OS-backed: `recv()` resolves when the watched directory changes.
    Watched {
        rx: DebouncedWatchReceiver,
        // Held for its `Drop` (unregisters paths); the subscriber keeps the
        // watcher alive for the lifetime of this struct.
        _registration: WatchRegistration,
        _subscriber: FileWatcherSubscriber,
        _watcher: std::sync::Arc<FileWatcher>,
    },
    /// Degraded: no OS watcher, tick on a fixed interval instead.
    Fallback { interval: Duration },
}

impl InboxWatcher {
    /// Create a watcher for `inboxes/{agent}.json` under `teams/{team}` rooted at
    /// `teams_root`. Never fails: an OS-watcher error degrades to interval
    /// ticking. `inboxes_dir` is the directory returned by
    /// `team_store::inboxes_dir` (passed in to avoid a dependency cycle).
    pub fn new(inboxes_dir: PathBuf) -> Self {
        match FileWatcher::new() {
            Ok(watcher) => Self::watched(std::sync::Arc::new(watcher), inboxes_dir),
            Err(err) => {
                tracing::warn!(
                    "teams inbox watcher: OS watcher unavailable ({err}); falling back to interval polling"
                );
                Self {
                    kind: InboxWatcherKind::Fallback {
                        interval: FALLBACK_TICK,
                    },
                }
            }
        }
    }

    /// Build an OS-backed watcher over the inboxes directory. The directory may
    /// not exist yet (no teammate has written a mailbox); the underlying watcher
    /// tolerates missing paths via an ancestor fallback and reports the create
    /// when it appears.
    fn watched(watcher: std::sync::Arc<FileWatcher>, inboxes_dir: PathBuf) -> Self {
        let (subscriber, rx) = watcher.add_subscriber();
        let registration = subscriber.register_paths(vec![WatchPath {
            path: inboxes_dir,
            recursive: false,
        }]);
        Self {
            kind: InboxWatcherKind::Watched {
                rx: DebouncedWatchReceiver::new(rx, DEBOUNCE_INTERVAL),
                _registration: registration,
                _subscriber: subscriber,
                _watcher: watcher,
            },
        }
    }

    /// Wait for the next change signal. Returns `true` to keep looping, `false`
    /// only if the underlying watcher channel closed (watcher dropped), which
    /// tells the caller to stop.
    ///
    /// A `true` result means "read the inbox now" — it does NOT guarantee new
    /// unread messages (debounced FS events are coarse). The caller re-reads and
    /// decides. This preserves the delivery-before-read contract unchanged.
    pub async fn changed(&mut self) -> bool {
        match &mut self.kind {
            InboxWatcherKind::Watched { rx, .. } => rx.recv().await.is_some(),
            InboxWatcherKind::Fallback { interval } => {
                tokio::time::sleep(*interval).await;
                true
            }
        }
    }

    /// Whether this watcher is OS-backed (`true`) or degraded to ticking
    /// (`false`). Exposed for tests and diagnostics.
    pub fn is_os_backed(&self) -> bool {
        matches!(self.kind, InboxWatcherKind::Watched { .. })
    }
}
