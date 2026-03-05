use std::thread;
use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

struct SearchState {
    ctx:     RawGrepCtx<ChannelSink>,
    rx:      Option<Receiver<RawMatch>>,
    status:  SearchStatus,
    results: Vec<RawMatch>,
}

impl SearchState {
    #[inline]
    fn new(num_threads: usize) -> Self {
        Self {
            ctx:     RawGrepCtx::new(num_threads, setup_signal_handler()),
            rx:      Default::default(),
            results: Default::default(),
            status:  Default::default()
        }
    }

    fn start(&mut self, pattern: &str, root: &str) {
        self.ctx.cancel();
        self.ctx.wait_and_save_cache();
        self.status = SearchStatus::Idle;

        let (tx, rx) = rawgrep::crossbeam_channel::unbounded();
        debug!("[state] new channel created");

        self.results.clear();

        self.rx = Some(rx);

        let mut cfg = RawGrepConfig::new(pattern, root);
        cfg.pipe_to_stdout = false;
        if let Err(e) = self.ctx.search(
            cfg,
            ChannelSink(tx),
            |_, _, _, _| {}
        ) {
            self.status = SearchStatus::Error(match e {
                Error::InvalidPattern(p)    => format!("invalid pattern: {p}").into(),
                Error::PathNotFound { path, .. } => format!("path not found: {path}").into(),
                Error::PermissionDenied(p)  => format!("permission denied: {p}").into(),
                Error::UnknownFilesystem(f) => format!("unknown filesystem: {f}").into(),
                Error::MatcherInit(e)       => format!("matcher error: {e}").into(),
                _ => e.to_string().into(),
            });
        }

        debug!("[state] search started");
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else {
            debug!("[state] drain called but rx is None");
            return;
        };

        let before = self.results.len();
        while let Ok(m) = rx.try_recv() {
            self.results.push(m);
        }
        let after = self.results.len();

        if after > before {
            debug!("[state] drain got {} new matches", after - before);
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
    generation:  u64,

    pub pending: Arc<Mutex<Vec<RawMatch>>>,
    pub pending_status: Arc<Mutex<SearchStatus>>,
    pending_generation: Arc<AtomicU64>,

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

    #[inline]
    pub fn drain(&mut self) {
        let current = self.pending_generation.load(Ordering::SeqCst);
        if current != self.generation {
            self.results.clear();
            self.generation = current;
        }

        if let Some(mut r) = self.pending.try_lock() {
            self.results.extend(r.drain(..));
        }
        if let Some(s) = self.pending_status.try_lock() {
            self.status = s.clone();
        }
    }

    #[inline]
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<SearchCmd>();

        let pending            = Arc::default();
        let pending_status     = Arc::default();
        let pending_generation = Arc::default();
        let searching          = Arc::default();

        thread::spawn({
            let pending = Arc::clone(&pending);
            let pending_generation = Arc::clone(&pending_generation);
            let pending_status = Arc::clone(&pending_status);

            move || Self::run_search_thread(
                cmd_rx,
                pending,
                searching,
                pending_status,
                pending_generation
            )
        });

        Self {
            cmd_tx,
            pending,
            pending_status,
            pending_generation,
            status: Default::default(),
            generation: Default::default(),
            results: Default::default(),
        }
    }

    fn run_search_thread(
        cmd_rx:             Receiver<SearchCmd>,
        pending_results:            Arc<Mutex<Vec<RawMatch>>>,
        searching:          Arc<AtomicBool>,
        pending_status:     Arc<Mutex<SearchStatus>>,
        pending_generation: Arc<AtomicU64>,
    ) {
        let mut state = SearchState::new(6);
        let mut batch = Vec::with_capacity(256);

        loop {
            let Ok(cmd) = cmd_rx.recv() else { break };

            match cmd {
                SearchCmd::Shutdown => break,

                SearchCmd::Cancel => {
                    state.ctx.cancel();
                    searching.store(false, Ordering::SeqCst);
                }

                SearchCmd::Start { pattern, root } => {
                    {
                        *pending_status.lock() = SearchStatus::Running;
                        pending_results.lock().clear();
                        pending_generation.fetch_add(1, Ordering::SeqCst);
                    }
                    state.start(&pattern, &root);

                    // Check if start() produced an error
                    if let SearchStatus::Error(_) = &state.status {
                        *pending_status.lock() = state.status.clone();
                        continue; // Skip the drain loop
                    }

                    loop {
                        match cmd_rx.try_recv() {
                            Ok(SearchCmd::Start { pattern, root }) => {
                                {
                                    *pending_status.lock() = SearchStatus::Running;
                                    pending_results.lock().clear();
                                    pending_generation.fetch_add(1, Ordering::SeqCst);
                                }
                                state.start(&pattern, &root);

                                // Check if start() produced an error
                                if let SearchStatus::Error(_) = &state.status {
                                    *pending_status.lock() = state.status.clone();
                                    break;
                                }
                            }

                            Ok(SearchCmd::Cancel | SearchCmd::Shutdown) => {
                                state.ctx.cancel();
                                pending_results.lock().extend(batch.drain(..));
                                searching.store(false, Ordering::SeqCst);
                                break;
                            }

                            Err(_) => {}
                        }

                        state.drain();
                        batch.extend(state.results.drain(..));

                        if batch.len() >= 24 {  // @Constant @Tune
                            pending_results.lock().extend(batch.drain(..));
                        }

                        if !state.ctx.is_running() {
                            state.drain();
                            batch.extend(state.results.drain(..));
                            pending_results.lock().extend(batch.drain(..));
                            searching.store(false, Ordering::SeqCst);
                            break;
                        }

                        thread::sleep(Duration::from_millis(8)); // @Constant @Tune
                    }
                }
            }
        }
    }
}
