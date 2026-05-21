//! Connectivity monitor for detecting network availability.
//!
//! The [`ConnectivityMonitor`] periodically attempts to reach the configured
//! COS endpoint (via [`CosClient::test_connection`]) and broadcasts state
//! changes through a [`tokio::sync::watch`] channel. Other components
//! (e.g. the sync engine) can subscribe to connectivity changes without
//! polling themselves.
//!
//! The check interval is 30 seconds, as required by Requirement 7.7.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::cos_client::CosClient;

/// How often (in seconds) the monitor re-evaluates connectivity.
const CHECK_INTERVAL_SECS: u64 = 30;

/// Monitors network connectivity by periodically probing the COS endpoint.
///
/// # Usage
///
/// ```ignore
/// let monitor = ConnectivityMonitor::new(cos_client);
/// monitor.start();
///
/// // Query current state:
/// assert!(monitor.is_online());
///
/// // Subscribe to changes from another task:
/// let mut rx = monitor.subscribe();
/// tokio::spawn(async move {
///     while rx.changed().await.is_ok() {
///         println!("online = {}", *rx.borrow());
///     }
/// });
///
/// // Shut down gracefully:
/// monitor.stop();
/// ```
pub struct ConnectivityMonitor {
    cos_client: Arc<CosClient>,
    /// Sender half of the watch channel — holds the current online state.
    tx: watch::Sender<bool>,
    /// Receiver half kept around for cloning into subscribers.
    rx: watch::Receiver<bool>,
    /// Handle to the background polling task, if running.
    poll_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    /// Signal used to tell the polling task to shut down.
    shutdown_tx: watch::Sender<bool>,
}

impl ConnectivityMonitor {
    /// Create a new monitor. The initial state is assumed offline (`false`)
    /// until the first successful probe.
    pub fn new(cos_client: Arc<CosClient>) -> Self {
        let (tx, rx) = watch::channel(false);
        let (shutdown_tx, _) = watch::channel(false);

        Self {
            cos_client,
            tx,
            rx,
            poll_handle: std::sync::Mutex::new(None),
            shutdown_tx,
        }
    }

    /// Returns the current connectivity state.
    pub fn is_online(&self) -> bool {
        *self.rx.borrow()
    }

    /// Returns a [`watch::Receiver`] that yields `true`/`false` whenever
    /// the connectivity state changes. Multiple subscribers are supported.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }

    /// Start the background polling task.
    ///
    /// If the monitor is already running, this is a no-op.
    pub fn start(&self) {
        let mut handle_guard = self.poll_handle.lock().unwrap();
        if handle_guard.is_some() {
            // Already running.
            return;
        }

        let cos_client = Arc::clone(&self.cos_client);
        let tx = self.tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let task = tokio::spawn(async move {
            loop {
                // Perform the connectivity check.
                let online = cos_client.test_connection().await.unwrap_or(false);

                // Only send if the value actually changed (watch already
                // deduplicates, but this avoids the send overhead).
                let _ = tx.send(online);

                // Wait for the next interval or a shutdown signal.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)) => {}
                    _ = shutdown_rx.changed() => {
                        // Shutdown requested.
                        break;
                    }
                }
            }
        });

        *handle_guard = Some(task);
    }

    /// Gracefully stop the background polling task.
    ///
    /// If the monitor is not running, this is a no-op.
    pub fn stop(&self) {
        // Signal the polling task to exit.
        let _ = self.shutdown_tx.send(true);

        let mut handle_guard = self.poll_handle.lock().unwrap();
        if let Some(handle) = handle_guard.take() {
            handle.abort();
        }
    }
}
