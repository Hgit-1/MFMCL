use crate::DownloadError;
use std::future::Future;
use tokio::sync::watch;

#[derive(Clone, Copy, Default)]
struct State {
    paused: bool,
    cancelled: bool,
}

/// Cooperative control shared by all engines/downloads to which it is attached.
/// Cancellation is permanent; create a new control for a new installation.
#[derive(Clone)]
pub struct DownloadControl {
    state: watch::Sender<State>,
}

impl std::fmt::Debug for DownloadControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadControl")
            .field("paused", &self.is_paused())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl Default for DownloadControl {
    fn default() -> Self {
        Self {
            state: watch::channel(State::default()).0,
        }
    }
}

impl DownloadControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pause(&self) {
        self.state.send_modify(|s| s.paused = true);
    }
    pub fn resume(&self) {
        self.state.send_modify(|s| s.paused = false);
    }
    pub fn cancel(&self) {
        self.state.send_modify(|s| s.cancelled = true);
    }
    pub fn is_paused(&self) -> bool {
        self.state.borrow().paused
    }
    pub fn is_cancelled(&self) -> bool {
        self.state.borrow().cancelled
    }

    /// Useful for installer work between artifact downloads as well.
    pub async fn checkpoint(&self) -> Result<(), DownloadError> {
        self.wait_for(std::future::ready(())).await
    }

    pub(crate) async fn wait_for<F: Future>(&self, future: F) -> Result<F::Output, DownloadError> {
        let mut receiver = self.state.subscribe();
        tokio::pin!(future);
        loop {
            let state = *receiver.borrow_and_update();
            if state.cancelled {
                return Err(DownloadError::Cancelled);
            }
            if state.paused {
                let _ = receiver.changed().await;
                continue;
            }
            tokio::select! {
                biased;
                _ = receiver.changed() => {},
                result = &mut future => return Ok(result),
            }
        }
    }
}
