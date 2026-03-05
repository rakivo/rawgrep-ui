use std::thread;
use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rawgrep::setup_signal_handler;
use rawgrep::{RawGrepCtx, RawGrepConfig, worker::ChannelSink};
use rawgrep::worker::RawMatch;
use rawgrep::crossbeam_channel::{Receiver, Sender, unbounded};

use parking_lot::Mutex;

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
            rx:      None,
            results: Vec::new(),
        }
    }

    #[inline]
    fn start(&mut self, pattern: &str, root: &str) {
        self.ctx.cancel();
        self.ctx.wait();

        let (tx, rx) = rawgrep::crossbeam_channel::unbounded();
        eprintln!("[state] new channel created");

        self.results.clear();

        self.rx = Some(rx);

        let mut cfg = RawGrepConfig::new(pattern, root);
        cfg.pipe_to_stdout = false;
        self.ctx.search(
            cfg,
            ChannelSink(tx),
            |_, _, _, _| {}
        ).ok();

        eprintln!("[state] search started");
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else {
            eprintln!("[state] drain called but rx is None");
            return;
        };

        let before = self.results.len();
        while let Ok(m) = rx.try_recv() {
            self.results.push(m);
        }
        let after = self.results.len();

        if after > before {
            eprintln!("[state] drain got {} new matches", after - before);
        }
    }
}

pub enum SearchCmd {
    Start { pattern: Box<str>, root: Box<str> },
    Cancel,
    Shutdown,
}

pub struct SearchManager {
    pub pending: Arc<Mutex<Vec<RawMatch>>>,
    pub results: Vec<RawMatch>,

    pub cmd_tx: Sender<SearchCmd>,

    generation:         u64,
    pending_generation: Arc<AtomicU64>,
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
    }

    #[inline]
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<SearchCmd>();

        let pending   = Arc::default();
        let pending_generation = Arc::default();
        let searching = Arc::default();

        thread::spawn({
            let pending = Arc::clone(&pending);
            let pending_generation = Arc::clone(&pending_generation);

            move || {
               Self::run_search_thread(cmd_rx, pending, searching, pending_generation);
            }
        });

        Self { cmd_tx, pending, pending_generation, generation: 0, results: Vec::new() }
    }

    fn run_search_thread(
        cmd_rx:         Receiver<SearchCmd>,
        pending:        Arc<Mutex<Vec<RawMatch>>>,
        searching:      Arc<AtomicBool>,
        pending_generation: Arc<AtomicU64>,
    ) {
        let mut state = SearchState::new(6);
        let mut batch = Vec::with_capacity(256);

        loop {
            let cmd = match cmd_rx.recv() {
                Ok(c)  => c,
                Err(_) => break,
            };

            match cmd {
                SearchCmd::Shutdown => break,

                SearchCmd::Cancel => {
                    state.ctx.cancel();
                    searching.store(false, Ordering::SeqCst);
                }

                SearchCmd::Start { pattern, root } => {
                    {
                        let mut p = pending.lock();
                        p.clear();
                        pending_generation.fetch_add(1, Ordering::SeqCst);
                    }
                    state.start(&pattern, &root);

                    loop {
                        match cmd_rx.try_recv() {
                            Ok(SearchCmd::Start { pattern, root }) => {
                                {
                                    let mut p = pending.lock();
                                    p.clear();
                                    pending_generation.fetch_add(1, Ordering::SeqCst);
                                }

                                state.start(&pattern, &root);
                            }

                            Ok(SearchCmd::Cancel | SearchCmd::Shutdown) => {
                                state.ctx.cancel();
                                pending.lock().extend(batch.drain(..));
                                searching.store(false, Ordering::SeqCst);
                                break;
                            }

                            Err(_) => {}
                        }

                        state.drain();
                        batch.extend(state.results.drain(..));

                        if batch.len() >= 24 { // @Constant @Tune
                            pending.lock().extend(batch.drain(..));
                        }

                        if !state.ctx.is_running() {
                            state.drain();
                            batch.extend(state.results.drain(..));
                            pending.lock().extend(batch.drain(..));
                            searching.store(false, Ordering::SeqCst);
                            break;
                        }

                        thread::sleep(Duration::from_millis(8));
                    }
                }
            }
        }
    }
}
