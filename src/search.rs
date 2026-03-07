use std::thread;
use std::sync::Arc;
use std::time::Duration;

use ::rawgrep::tracing::debug;
use rawgrep::worker::MatchSink;
use rawgrep::{Error, setup_signal_handler};
use rawgrep::{RawGrepCtx, RawGrepConfig};
use rawgrep::crossbeam_channel::{Receiver, Sender, unbounded};

use parking_lot::{Mutex, RwLock, RwLockReadGuard};

#[derive(Default, Clone, Debug)]
pub enum SearchStatus {
    #[default]
    Idle,
    Running,
    Done,
    Error(Box<str>),
}

// A single match stored as offsets into SearchState's flat buffers
#[derive(Clone, Copy)]
pub struct MatchEntry {
    pub path_start:   u32,
    pub path_len:     u32,
    pub text_start:   u32,
    pub text_len:     u32,
    pub ranges_start: u32,
    pub ranges_len:   u32,
    pub line_num:     u32,
}

pub struct MatchView<'a> {
    pub path:     &'a [u8],
    pub line_num: u32,
    pub text:     &'a [u8],
    pub ranges:   &'a [(u32, u32)],
}

#[derive(Clone)]
pub struct StoreSink(pub Arc<RwLock<MatchStore>>);

impl MatchSink for StoreSink {
    const STDOUT_NOP: bool = true;

    #[inline(always)]
    fn push(&self, path: &[u8], line_num: u32, text: &[u8], ranges: &[(u32, u32)]) {
        self.0.write().push(path, line_num, text, ranges);
    }
}

// Owns all match data. Lives on the search thread.
// Shared read-only with UI via Arc after each commit.
#[derive(Default)]
pub struct MatchStore {
    // Flattened fields for all matches
    path:     Vec<u8>,
    text:     Vec<u8>,
    ranges:   Vec<(u32, u32)>,

    entries:  Vec<MatchEntry>,
}

impl MatchStore {
    #[inline]
    fn new() -> Self { // @Memory @Tune
        Self {
            path:   Vec::with_capacity(256 * 1024),
            text:   Vec::with_capacity(256 * 1024),
            ranges: Vec::with_capacity(64 * 1024),
            entries:    Vec::with_capacity(4096),
        }
    }

    #[inline]
    fn reset(&mut self) {
        self.path.clear();
        self.text.clear();
        self.ranges.clear();
        self.entries.clear();
    }

    #[inline]
    fn push(&mut self, path: &[u8], line_num: u32, text: &[u8], ranges: &[(u32, u32)]) {
        let path_start   = self.path.len() as u32;
        let text_start   = self.text.len() as u32;
        let ranges_start = self.ranges.len() as u32;

        self.path.extend_from_slice(path);
        self.text.extend_from_slice(text);
        self.ranges.extend_from_slice(ranges);

        self.entries.push(MatchEntry {
            path_start,
            path_len:   path.len() as u32,
            text_start,
            text_len:   text.len() as u32,
            ranges_start,
            ranges_len: ranges.len() as u32,
            line_num,
        });
    }

    #[inline]
    fn get(&self, e: &MatchEntry) -> MatchView<'_> {
        MatchView {
            path:     &self.path  [e.path_start   as usize .. e.path_start   as usize + e.path_len   as usize],
            text:     &self.text  [e.text_start   as usize .. e.text_start   as usize + e.text_len   as usize],
            ranges:   &self.ranges[e.ranges_start as usize .. e.ranges_start as usize + e.ranges_len as usize],
            line_num: e.line_num,
        }
    }
}

struct SearchState {
    ctx:   RawGrepCtx<StoreSink>,
    store: Arc<RwLock<MatchStore>>,
}

impl SearchState {
    #[inline]
    fn new(num_threads: usize) -> Self {
        Self {
            ctx:   RawGrepCtx::new(num_threads, setup_signal_handler()),
            store: Default::default()
        }
    }

    #[inline]
    fn start(&mut self, pattern: &str, root: &str) -> Option<Box<str>> {
        self.ctx.cancel();
        self.ctx.wait_and_save_cache();
        self.store.write().reset();

        let store = Arc::clone(&self.store);
        let sink = StoreSink(store);

        let cfg = RawGrepConfig::new(pattern, root);
        if let Err(e) = self.ctx.search(cfg, sink, |_, _, _, _| {}) {
            let msg = match e {
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
}

pub enum SearchCmd {
    Start { pattern: Box<str>, root: Box<str> },
    Cancel,
    Shutdown,
}

// What the search thread shares with the UI thread -
// just a count of committed entries, not the data itself
struct PendingState {
    committed:  usize,        // How many MatchEntries are ready to read
    generation: u64,
    status:     SearchStatus,
}

impl Default for PendingState {
    fn default() -> Self {
        Self { committed: 0, generation: 0, status: SearchStatus::Idle }
    }
}

pub struct SearchManager {
    pub status:    SearchStatus,
    pub store:     Arc<RwLock<MatchStore>>,  // Read-only access to the match data

    generation:    u64,
    committed:     usize,                  // How many entries we've seen

    pending:       Arc<Mutex<PendingState>>,
    pub cmd_tx:    Sender<SearchCmd>,
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
    pub fn cancel(&self) {
        self.cmd_tx.send(SearchCmd::Cancel).ok();
    }

    // Called every frame, never blocks
    #[inline]
    pub fn drain(&mut self) {
        let Some(p) = self.pending.try_lock() else { return };

        if p.generation != self.generation {
            self.generation = p.generation;
            self.committed  = 0;  // New search, reset our read cursor
        }

        self.committed = p.committed;
        self.status    = p.status.clone();
    }

    #[inline]
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<SearchCmd>();
        let pending          = Arc::default();
        let store            = Arc::new(RwLock::new(MatchStore::new()));

        thread::spawn({
            let pending = Arc::clone(&pending);
            let store   = Arc::clone(&store);
            move || Self::run_search_thread(cmd_rx, pending, store)
        });

        Self {
            cmd_tx,
            pending,
            store,
            status:     Default::default(),
            generation: Default::default(),
            committed:  Default::default(),
        }
    }

    fn run_search_thread(
        cmd_rx:  Receiver<SearchCmd>,
        pending: Arc<Mutex<PendingState>>,
        store:   Arc<RwLock<MatchStore>>,
    ) {
        let mut state = SearchState::new(6); // @Hack
        // Point state's store at the shared one
        state.store = Arc::clone(&store);

        loop {
            let Ok(cmd) = cmd_rx.recv() else { break };

            match cmd {
                SearchCmd::Shutdown => break,

                SearchCmd::Cancel => state.ctx.cancel(),

                SearchCmd::Start { pattern, root } => {
                    {
                        let mut p = pending.lock();
                        p.generation += 1;
                        p.committed   = 0;
                        p.status      = SearchStatus::Running;
                    }

                    if let Some(err) = state.start(&pattern, &root) {
                        pending.lock().status = SearchStatus::Error(err);
                        continue;
                    }

                    loop {
                        match cmd_rx.try_recv() {
                            Ok(SearchCmd::Start { pattern, root }) => {  // @Cutnpaste from above
                                {
                                    let mut p = pending.lock();
                                    p.generation += 1;
                                    p.committed   = 0;
                                    p.status      = SearchStatus::Running;
                                }

                                if let Some(err) = state.start(&pattern, &root) {
                                    pending.lock().status = SearchStatus::Error(err);
                                    break;
                                }
                            }

                            Ok(SearchCmd::Cancel | SearchCmd::Shutdown) => {
                                state.ctx.cancel();

                                let committed = store.read().entries.len();
                                let mut p = pending.lock();

                                p.committed = committed;
                                p.status    = SearchStatus::Done;

                                break;
                            }

                            Err(_) => {}
                        }

                        // Commit however many entries are ready
                        let committed = store.read().entries.len();
                        {
                            let mut p = pending.lock();
                            p.committed = committed;
                        }

                        if !state.ctx.is_running() {
                            let committed = store.read().entries.len();
                            let mut p = pending.lock();
                            p.committed = committed;
                            p.status    = SearchStatus::Done;
                            break;
                        }

                        thread::sleep(Duration::from_millis(8));
                    }
                }
            }
        }
    }
}

pub struct LockedMatches<'a> {
    store:     Option<RwLockReadGuard<'a, MatchStore>>,
    committed: usize,
}

impl<'a> LockedMatches<'a> {
    #[inline]
    pub fn len(&self) -> usize {
        self.committed
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.committed == 0
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<MatchView<'_>> {
        if index >= self.committed { return None; }

        let store = self.store.as_ref()?;
        Some(store.get(&store.entries[index]))
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = MatchView<'_>> {
        let committed = self.committed;
        (0..committed).flat_map(move |i| self.get(i))
    }
}

impl SearchManager {
    #[inline]
    pub fn matches(&self) -> LockedMatches<'_> {
        LockedMatches {
            store:     Some(self.store.read()),
            committed: self.committed,
        }
    }

    #[inline]
    pub fn try_matches(&self) -> LockedMatches<'_> {
        match self.store.try_read() {
            Some(guard) => LockedMatches {
                store:     Some(guard),
                committed: self.committed,
            },
            None => LockedMatches {
                store:     None,
                committed: 0,
            },
        }
    }

    #[inline]
    pub fn match_count(&self) -> usize {
        self.committed
    }

    #[inline]
    pub fn clear(&mut self) {
        self.committed = 0;
        self.set_idle();
    }

    #[inline]
    pub fn set_idle(&mut self) {
        self.status = SearchStatus::Idle;
        self.pending.lock().status = SearchStatus::Idle;
    }
}
