use std::thread;
use std::sync::Arc;
use std::time::Duration;

use ::rawgrep::tracing::debug;
use rawgrep::{Error, setup_signal_handler};
use rawgrep::{RawGrepCtx, RawGrepConfig, worker::ChannelSink};
use rawgrep::worker::RawMatch;
use rawgrep::crossbeam_channel::{Receiver, Sender, unbounded};

use parking_lot::Mutex;

#[derive(Default, Clone, Debug)]
pub enum SearchStatus {
    #[default]
    Idle,
    Running,
    Done,
    Error(Box<str>),
}

pub struct PendingState {
    pub results:    Vec<RawMatch>,
    pub status:     SearchStatus,
    pub generation: u64,
}

impl Default for PendingState {
    #[inline]
    fn default() -> Self {
        Self {
            results:    Vec::new(),
            status:     SearchStatus::Idle,
            generation: 0,
        }
    }
}

impl PendingState {
    /// Called at the start of each new search. Clears results, bumps
    /// generation, and sets status to Running.
    #[inline]
    fn begin_search(&mut self) {
        self.results.clear();
        self.generation += 1;
        self.status = SearchStatus::Running;
    }
}

//
// SearchState - internal to the search thread
//

struct SearchState {
    ctx:     RawGrepCtx<ChannelSink>,
    rx:      Option<Receiver<RawMatch>>,
    results: Vec<RawMatch>,
}

impl SearchState {
    #[inline]
    fn new(num_threads: usize) -> Self {
        Self {
            ctx:     RawGrepCtx::new(num_threads, setup_signal_handler()),
            rx:      Default::default(),
            results: Default::default(),
        }
    }

    fn start(&mut self, pattern: &str, root: &str) -> Option<Box<str>> {
        self.ctx.cancel();
        self.ctx.wait_and_save_cache();
        self.results.clear();

        let (tx, rx) = rawgrep::crossbeam_channel::unbounded();
        self.rx = Some(rx);

        let mut cfg = RawGrepConfig::new(pattern, root);
        cfg.pipe_to_stdout = false;

        if let Err(e) = self.ctx.search(cfg, ChannelSink(tx), |_, _, _, _| {}) {
            let msg: Box<str> = match e {
                Error::InvalidPattern(p)         => format!("invalid pattern: {p}").into(),
                Error::PathNotFound { path, .. } => format!("path not found: {path}").into(),
                Error::PermissionDenied(p)       => format!("permission denied: {p}").into(),
                Error::UnknownFilesystem(f)      => format!("unknown filesystem: {f}").into(),
                Error::MatcherInit(e)            => format!("matcher error: {e}").into(),
                _                                => e.to_string().into(),
            };
            return Some(msg);
        }

        debug!("[state] search started pattern={pattern:?} root={root:?}");
        None
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };

        let before = self.results.len();
        while let Ok(m) = rx.try_recv() {
            self.results.push(m);
        }

        if self.results.len() > before {
            debug!("[state] drain got {} new matches", self.results.len() - before);
        }
    }
}

pub enum SearchCmd {
    Start { pattern: Box<str>, root: Box<str> },
    Cancel,
    Shutdown,
}

pub struct SearchManager {
    pub results: Vec<RawMatch>,
    pub status:  SearchStatus,

    generation: u64,
    pub pending: Arc<Mutex<PendingState>>,
    pub cmd_tx: Sender<SearchCmd>,
}

impl SearchManager {
    #[inline]
    pub fn start(&self, pattern: &str, root: &str) {
        self.cmd_tx.send(SearchCmd::Start {
            pattern: pattern.into(),
            root:    root.into(),
        }).ok();
    }

    /// Pull any new results and status from the search thread into local storage.
    ///
    /// Called every frame from the UI thread. Uses `try_lock` so it never
    /// blocks the render loop if the search thread is mid-flush.
    #[inline]
    pub fn drain(&mut self) {
        let Some(mut p) = self.pending.try_lock() else { return };

        if p.generation != self.generation {
            // New search started - discard stale results from previous search
            self.results.clear();
            self.generation = p.generation;
        }

        self.results.extend(p.results.drain(..));
        self.status = p.status.clone();
    }

    #[inline]
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<SearchCmd>();
        let pending = Arc::default();

        thread::spawn({
            let pending = Arc::clone(&pending);
            move || Self::run_search_thread(cmd_rx, pending)
        });

        Self {
            cmd_tx,
            pending,
            status:     Default::default(),
            generation: Default::default(),
            results:    Default::default(),
        }
    }

    fn run_search_thread(
        cmd_rx:  Receiver<SearchCmd>,
        pending: Arc<Mutex<PendingState>>,
    ) {
        let mut state = SearchState::new(6);
        let mut batch = Vec::with_capacity(256);

        loop {
            let Ok(cmd) = cmd_rx.recv() else { break };

            match cmd {
                SearchCmd::Shutdown => break,

                SearchCmd::Cancel => {
                    state.ctx.cancel();
                }

                SearchCmd::Start { pattern, root } => {
                    pending.lock().begin_search();
                    if let Some(err) = state.start(&pattern, &root) {
                        pending.lock().status = SearchStatus::Error(err);
                        continue;
                    }

                    // Drain loop - runs until search completes or a new cmd arrives
                    loop {
                        match cmd_rx.try_recv() {
                            Ok(SearchCmd::Start { pattern, root }) => {
                                pending.lock().begin_search();
                                if let Some(err) = state.start(&pattern, &root) {
                                    pending.lock().status = SearchStatus::Error(err);
                                    break;
                                }
                            }

                            Ok(SearchCmd::Cancel | SearchCmd::Shutdown) => {
                                state.ctx.cancel();
                                // Flush whatever we have then mark done
                                let mut p = pending.lock();
                                p.results.extend(batch.drain(..));
                                p.status = SearchStatus::Done;
                                break;
                            }

                            Err(_) => {}
                        }

                        state.drain();
                        batch.extend(state.results.drain(..));

                        if batch.len() >= 24 { // @Constant @Tune
                            pending.lock().results.extend(batch.drain(..));
                        }

                        if !state.ctx.is_running() {
                            // Final drain - workers are done, channel is drained
                            state.drain();
                            batch.extend(state.results.drain(..));
                            let mut p = pending.lock();
                            p.results.extend(batch.drain(..));
                            p.status = SearchStatus::Done;
                            break;
                        }

                        thread::sleep(Duration::from_millis(8));  // @Constant @Tune
                    }
                }
            }
        }
    }
}
