//! The hub: sentences in from every source, one state out to every app.

use crate::fix::{Navigation, STALE_AFTER};
use crate::nmea;
use crate::protocol::{Message, SourceState, SourceStatus, VERSION};
use crate::source::{self, Event, Spec};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::{
        fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, Instant, MissedTickBehavior, interval, sleep, timeout},
};

/// Messages queued for one app. An app that falls this far behind is dropped.
const QUEUE: usize = 64;
/// An app that can't take one message in this long has stalled.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Lines waiting for the recorder's thread. A disk this far behind loses
/// lines, and says so, rather than stalling navigation.
const RECORD_QUEUE: usize = 4096;
/// How often the recording is synced to disk: the most a power cut loses.
const SYNC_EVERY: Duration = Duration::from_secs(10);

pub struct Config {
    pub sources: Vec<Spec>,
    pub socket: PathBuf,
    /// Where to record every line received, if anywhere. Never overwritten.
    pub record: Option<PathBuf>,
}

/// `$XDG_RUNTIME_DIR/omakeel/keel.sock`.
pub fn default_socket() -> io::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("omakeel").join("keel.sock"))
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR must be set to an absolute path"))
}

/// Runs until it fails to bind or to start recording. The socket file is
/// removed, and the recording synced, when the returned future ends or is
/// dropped.
pub async fn run(config: Config) -> io::Result<()> {
    let (listener, _socket) = bind(&config.socket)?;
    let mut recorder = config.record.as_deref().map(Recorder::create).transpose()?;
    let (tx, mut events) = mpsc::channel(1024);
    let mut sources: Vec<Tracked> = config
        .sources
        .iter()
        .map(|spec| Tracked {
            state: SourceState::new(spec.to_string()),
            heard: None,
        })
        .collect();
    for (index, spec) in config.sources.into_iter().enumerate() {
        source::spawn(index, spec, tx.clone());
    }
    drop(tx);

    let mut nav = Navigation::default();
    let mut clients: Vec<Client> = Vec::new();
    let mut last: Arc<str> = Arc::from("");
    // Once a second the hub re-judges freshness, so a fix goes stale on
    // time even when nothing arrives.
    let mut tick = interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            Some(event) = events.recv() => match event {
                Event::Line { source, line } => {
                    if let Some(r) = recorder.as_mut()
                        && !r.record(&line)
                    {
                        recorder = None;
                    }
                    let now = Instant::now();
                    let tracked = &mut sources[source];
                    tracked.heard = Some(now);
                    tracked.state.status = SourceStatus::Ok;
                    tracked.state.message = None;
                    match nmea::parse(&line) {
                        Ok(sentence) => {
                            tracked.state.sentences += 1;
                            if let Some(position) = nmea::position(&sentence) {
                                nav.update(position, now);
                            }
                        }
                        Err(_) => tracked.state.rejected += 1,
                    }
                }
                Event::Status { source, status, message } => {
                    let state = &mut sources[source].state;
                    state.status = status;
                    state.message = message;
                }
            },
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let client = Client::spawn(stream);
                    let hello = encode(&Message::Hello { v: VERSION, keel: env!("CARGO_PKG_VERSION") });
                    if client.tx.try_send(hello).is_ok() {
                        clients.push(client);
                    }
                }
                Err(e) => {
                    eprintln!("omakeel: accept: {e}");
                    sleep(Duration::from_millis(100)).await;
                }
            },
            _ = tick.tick() => {}
        }

        let now = Instant::now();
        for tracked in &mut sources {
            let silent = tracked
                .heard
                .is_some_and(|t| now.saturating_duration_since(t) >= STALE_AFTER);
            if tracked.state.status == SourceStatus::Ok && silent {
                tracked.state.status = SourceStatus::Quiet;
            }
        }
        let states: Vec<SourceState> = sources.iter().map(|t| t.state.clone()).collect();
        let state = encode(&Message::State {
            v: VERSION,
            fix: &nav.state(now),
            sources: &states,
        });
        let changed = state != last;
        clients.retain_mut(|client| {
            if !changed && !client.fresh {
                return true;
            }
            client.fresh = false;
            client.tx.try_send(state.clone()).is_ok()
        });
        last = state;
    }
}

struct Tracked {
    state: SourceState,
    heard: Option<Instant>,
}

fn encode(message: &Message) -> Arc<str> {
    let mut line = serde_json::to_string(message).expect("messages serialize");
    line.push('\n');
    line.into()
}

/// One connected app, fed through a queue by its own writer task. Dropping
/// it closes the app's socket at once, queued messages and all.
struct Client {
    tx: mpsc::Sender<Arc<str>>,
    /// Hasn't been sent the current state yet.
    fresh: bool,
    task: JoinHandle<()>,
}

impl Client {
    fn spawn(stream: UnixStream) -> Client {
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(QUEUE);
        let task = tokio::spawn(async move {
            let (mut reader, mut writer) = stream.into_split();
            let mut scratch = [0u8; 256];
            loop {
                tokio::select! {
                    line = rx.recv() => {
                        let Some(line) = line else { break };
                        let written = timeout(WRITE_TIMEOUT, writer.write_all(line.as_bytes())).await;
                        if !matches!(written, Ok(Ok(()))) {
                            break;
                        }
                    }
                    // Apps send nothing in version 1, so a read ends only when
                    // the app has gone: close its socket now, not at the next
                    // broadcast.
                    read = reader.read(&mut scratch) => {
                        if !matches!(read, Ok(n) if n > 0) {
                            break;
                        }
                    }
                }
            }
        });
        Client {
            tx,
            fresh: true,
            task,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Takes the lock beside the socket, so one socket has one hub, then binds,
/// replacing a socket a crashed hub left behind.
fn bind(path: &Path) -> io::Result<(UnixListener, SocketFile)> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && !dir.exists()
    {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let lock = lock(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and isn't a socket", path.display()),
            ));
        }
        // Holding the lock means no hub is serving it.
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    Ok((
        listener,
        SocketFile {
            path: path.to_path_buf(),
            _lock: lock,
        },
    ))
}

/// An exclusive lock on `keel.lock` beside `keel.sock`, held until exit.
fn lock(socket: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(socket.with_extension("lock"))?;
    // SAFETY: `flock` on a descriptor `file` owns; the lock lasts until the
    // file is closed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("omakeel is already running on {}", socket.display()),
            ));
        }
        return Err(e);
    }
    Ok(file)
}

/// Removes the socket, then lets go of the lock.
struct SocketFile {
    path: PathBuf,
    _lock: File,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Every line as it arrived, stamped with Unix milliseconds
/// (`docs/protocol.md`, recordings), written on a thread of its own so a
/// slow disk never holds up navigation.
struct Recorder {
    lines: Option<SyncSender<String>>,
    thread: Option<thread::JoinHandle<()>>,
    path: PathBuf,
    behind: bool,
}

impl Recorder {
    fn create(path: &Path) -> io::Result<Recorder> {
        let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(named)?;
        let mut out = BufWriter::new(file);
        writeln!(out, "# omakeel recording v1")?;
        out.flush()?;
        out.get_ref().sync_all()?;
        // A new file's name is only on disk once its directory is synced.
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        File::open(dir.unwrap_or(Path::new(".")))?.sync_all()?;

        let (tx, rx) = sync_channel::<String>(RECORD_QUEUE);
        let shown = path.display().to_string();
        let thread = thread::spawn(move || {
            let mut synced = std::time::Instant::now();
            for line in rx {
                if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
                    eprintln!("omakeel: stopped recording to {shown}: {e}");
                    return;
                }
                if synced.elapsed() >= SYNC_EVERY {
                    let _ = out.get_ref().sync_data();
                    synced = std::time::Instant::now();
                }
            }
            let _ = out.flush();
            let _ = out.get_ref().sync_data();
        });
        Ok(Recorder {
            lines: Some(tx),
            thread: Some(thread),
            path: path.to_path_buf(),
            behind: false,
        })
    }

    /// Stamps a line and queues it. False once the recorder has stopped.
    fn record(&mut self, line: &str) -> bool {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let Some(lines) = &self.lines else {
            return false;
        };
        match lines.try_send(format!("{ms} {line}")) {
            Ok(()) => {
                self.behind = false;
                true
            }
            Err(TrySendError::Full(_)) => {
                if !self.behind {
                    eprintln!(
                        "omakeel: the disk is behind; dropping lines from {}",
                        self.path.display()
                    );
                    self.behind = true;
                }
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Close the queue; the thread writes what's left, syncs, and ends.
        drop(self.lines.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
