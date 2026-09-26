//! The hub: sentences in from every source, the fix and the traffic out to
//! every app.

use crate::ais::Assembler;
use crate::fix::{Navigation, STALE_AFTER};
use crate::journal::{self, Journal, Stderr};
use crate::nmea;
use crate::protocol::{FixStatus, Message, SourceState, SourceStatus, VERSION};
use crate::source::{self, Event, Spec};
use crate::targets::{Own, Traffic};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::{
        fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, channel, sync_channel},
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
/// How soon after a line is written the recording is synced to disk.
const SYNC_EVERY: Duration = Duration::from_secs(10);
/// How long exiting waits for the recording to reach the disk. A stalled
/// disk mustn't hang shutdown.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

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
    // Nothing on the hub's loop writes to stderr directly: see `Stderr`.
    let stderr = Stderr::spawn();
    let mut recorder = config
        .record
        .as_deref()
        .map(|path| Recorder::create(path, stderr.clone()))
        .transpose()?;
    let (tx, mut events) = mpsc::channel(1024);
    let mut sources: Vec<Tracked> = config
        .sources
        .iter()
        .map(|spec| Tracked {
            state: SourceState::new(spec.to_string()),
            heard: None,
            connected: None,
            positions: false,
            ais: Assembler::default(),
        })
        .collect();
    for (index, spec) in config.sources.into_iter().enumerate() {
        source::spawn(index, spec, tx.clone());
    }
    drop(tx);

    let mut nav = Navigation::default();
    let mut journal = Journal::default();
    let mut traffic = Traffic::default();
    let mut clients: Vec<Client> = Vec::new();
    let mut last_state: Arc<str> = Arc::from("");
    let mut last_targets = encode(&Message::Targets {
        v: VERSION,
        targets: &[],
    });
    // Once a second the hub re-judges freshness, so a fix goes stale on
    // time even when nothing arrives, and sends the traffic.
    let mut tick = interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let mut ticked = false;
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
                    tracked.connected = None;
                    tracked.state.status = SourceStatus::Ok;
                    tracked.state.message = None;
                    match nmea::parse(&line) {
                        Ok(sentence) => {
                            tracked.state.sentences += 1;
                            if let Some(position) = nmea::position(&sentence) {
                                tracked.positions = true;
                                nav.update(position, now);
                            }
                            for report in tracked.ais.feed(&sentence) {
                                traffic.update(report, now);
                            }
                        }
                        Err(_) => tracked.state.rejected += 1,
                    }
                }
                Event::Status { source, status, message } => {
                    sources[source].connected = None;
                    let state = &mut sources[source].state;
                    state.status = status;
                    state.message = message;
                }
                // Reached again, but nothing received yet: the error before
                // it no longer stands.
                Event::Connected { source } => {
                    let now = Instant::now();
                    let tracked = &mut sources[source];
                    tracked.connected = Some(now);
                    tracked.state.status = SourceStatus::Connecting;
                    tracked.state.message = None;
                    journal.connected(source, now);
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
                    stderr.say(format!("omakeel: accept: {e}"));
                    sleep(Duration::from_millis(100)).await;
                }
            },
            _ = tick.tick() => ticked = true,
        }

        let now = Instant::now();
        for tracked in &mut sources {
            // A link reached again counts as heard from then, so one that
            // stays up and sends nothing goes quiet like one that stopped.
            let silent = tracked
                .last_sign()
                .is_some_and(|t| now.saturating_duration_since(t) >= STALE_AFTER);
            let waiting = match tracked.state.status {
                SourceStatus::Ok => true,
                SourceStatus::Connecting => tracked.connected.is_some(),
                _ => false,
            };
            if waiting && silent {
                tracked.state.status = SourceStatus::Quiet;
            }
        }
        // Whether a GPS is still sending anything, for a stale fix: the
        // receiver or its link gone silent, or talking without a position.
        // AIS arriving says nothing about the GPS, so only sources that
        // have given a position count.
        let arriving = sources.iter().any(|t| {
            t.positions
                && t.heard
                    .is_some_and(|h| now.saturating_duration_since(h) < STALE_AFTER)
        });
        let fix = nav.state(now);
        // What changed, in the recording and on stderr, so a dropout can be
        // told apart afterwards: a link down, a link quiet, or the sky lost.
        // Lines lost on the way to the disk may have been some of these;
        // write everything as it stands again.
        if recorder.as_mut().is_some_and(Recorder::caught_up) {
            journal.forget();
        }
        let mut news: Vec<String> = sources
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let silent = t.last_sign().map(|h| now.saturating_duration_since(h));
                let message = t.state.message.as_deref();
                journal.source(i, &t.state.name, t.state.status, message, silent, now)
            })
            .collect();
        news.extend(journal.fix(&fix, arriving));
        for text in news {
            let ms = unix_ms();
            let local = journal::local_time((ms / 1000) as i64);
            stderr.say(format!("omakeel: {local} {text}"));
            if let Some(r) = recorder.as_mut()
                && !r.send(format!("# {ms} {text}"), true)
            {
                recorder = None;
            }
        }
        let states: Vec<SourceState> = sources.iter().map(|t| t.state.clone()).collect();
        let state = encode(&Message::State {
            v: VERSION,
            fix: &fix,
            sources: &states,
        });
        let state_changed = state != last_state;
        last_state = state;

        let mut targets_changed = false;
        if ticked {
            traffic.expire(now);
            // Ranges and CPAs only from a current fix.
            let own = match (fix.status, fix.lat, fix.lon) {
                (FixStatus::Ok, Some(lat), Some(lon)) => Some(Own {
                    lat,
                    lon,
                    sog_kn: fix.sog_kn,
                    cog_deg: fix.cog_deg,
                }),
                _ => None,
            };
            let targets = encode(&Message::Targets {
                v: VERSION,
                targets: &traffic.targets(now, own),
            });
            targets_changed = targets != last_targets;
            last_targets = targets;
        }

        clients.retain_mut(|client| {
            // An app that has left is let go even when there's nothing new.
            if client.tx.is_closed() {
                return false;
            }
            let fresh = std::mem::take(&mut client.fresh);
            let sent_state =
                !(state_changed || fresh) || client.tx.try_send(last_state.clone()).is_ok();
            sent_state
                && (!(targets_changed || fresh) || client.tx.try_send(last_targets.clone()).is_ok())
        });
    }
}

struct Tracked {
    state: SourceState,
    heard: Option<Instant>,
    /// When a serial or TCP link was last reached, until its first line or
    /// its next error.
    connected: Option<Instant>,
    /// Has sent a position sentence: a GPS, not only AIS.
    positions: bool,
    /// Multi-sentence AIS messages are joined per source.
    ais: Assembler,
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

impl Tracked {
    /// The last line heard, or the link being reached since.
    fn last_sign(&self) -> Option<Instant> {
        match (self.heard, self.connected) {
            (Some(h), Some(c)) => Some(h.max(c)),
            (h, c) => h.or(c),
        }
    }
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
    /// Hasn't been sent the current state and targets yet.
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

/// `keel.sock.lock` for `keel.sock`: the whole socket name plus `.lock`, so
/// no two socket names share a lock.
fn lock_path(socket: &Path) -> PathBuf {
    let mut name = OsString::from(socket.as_os_str());
    name.push(".lock");
    PathBuf::from(name)
}

/// An exclusive lock beside the socket, held until exit.
fn lock(socket: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path(socket))?;
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
    /// Disconnects when the thread has finished, however it finished.
    finished: Receiver<()>,
    path: PathBuf,
    behind: bool,
    stderr: Stderr,
    /// A comment was dropped while the disk was behind.
    lost_comment: bool,
    /// The queue has taken a line since a comment was dropped.
    caught_up: bool,
}

impl Recorder {
    fn create(path: &Path, stderr: Stderr) -> io::Result<Recorder> {
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
        let (finished_tx, finished) = channel::<()>();
        let shown = path.display().to_string();
        let said = stderr.clone();
        thread::spawn(move || {
            let _finished = finished_tx;
            if let Err(e) = record(&mut out, &rx) {
                said.say(format!("omakeel: stopped recording to {shown}: {e}"));
            }
        });
        Ok(Recorder {
            lines: Some(tx),
            finished,
            path: path.to_path_buf(),
            behind: false,
            stderr,
            lost_comment: false,
            caught_up: false,
        })
    }

    /// Stamps a line and queues it. False once the recorder has stopped.
    fn record(&mut self, line: &str) -> bool {
        self.send(format!("{} {line}", unix_ms()), false)
    }

    /// True once, when the queue takes lines again after a comment was
    /// dropped.
    fn caught_up(&mut self) -> bool {
        std::mem::take(&mut self.caught_up)
    }

    /// Queues a line as it is, stamp and all. False once the recorder has
    /// stopped.
    fn send(&mut self, line: String, comment: bool) -> bool {
        let Some(lines) = &self.lines else {
            return false;
        };
        match lines.try_send(line) {
            Ok(()) => {
                self.behind = false;
                if std::mem::take(&mut self.lost_comment) {
                    self.caught_up = true;
                }
                true
            }
            Err(TrySendError::Full(_)) => {
                if !self.behind {
                    self.stderr.say(format!(
                        "omakeel: the disk is behind; dropping lines from {}",
                        self.path.display()
                    ));
                    self.behind = true;
                }
                self.lost_comment |= comment;
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

/// The recorder's thread: write each line, and sync it to disk within
/// `SYNC_EVERY` of writing it, idle or not. Stops at the first failed write
/// or sync, since a recording that can't reach the disk isn't one.
fn record(out: &mut BufWriter<File>, lines: &Receiver<String>) -> io::Result<()> {
    let mut unsynced_since: Option<std::time::Instant> = None;
    loop {
        let wait = unsynced_since.map_or(Duration::from_secs(3600), |t| {
            SYNC_EVERY.saturating_sub(t.elapsed())
        });
        match lines.recv_timeout(wait) {
            Ok(line) => {
                writeln!(out, "{line}")?;
                out.flush()?;
                unsynced_since.get_or_insert_with(std::time::Instant::now);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if unsynced_since.is_some_and(|t| t.elapsed() >= SYNC_EVERY) {
            out.get_ref().sync_data()?;
            unsynced_since = None;
        }
    }
    out.flush()?;
    out.get_ref().sync_data()
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Close the queue so the thread writes what's left and syncs, then
        // wait for it, but not forever.
        drop(self.lines.take());
        if let Err(RecvTimeoutError::Timeout) = self.finished.recv_timeout(SHUTDOWN_WAIT) {
            // At exit, off the loop: waiting on stderr is fine now, but a
            // closed one must not turn exiting into a panic.
            let _ = writeln!(
                io::stderr(),
                "omakeel: gave up waiting for {} to reach the disk",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_comment_is_made_good_once_the_disk_catches_up() {
        // A queue of one that nothing drains: the disk is behind.
        let (lines, queued) = sync_channel::<String>(1);
        let (_, finished) = channel::<()>();
        let mut r = Recorder {
            lines: Some(lines),
            finished,
            path: PathBuf::from("sail.nmea"),
            behind: false,
            stderr: Stderr::to(io::sink()),
            lost_comment: false,
            caught_up: false,
        };
        assert!(r.send("# 1 fix ok".into(), true));
        assert!(r.send("2 $GPRMC".into(), false), "a sentence dropped");
        assert!(
            !r.caught_up(),
            "a lost sentence needs nothing written again"
        );
        assert!(r.send("# 3 fix stale".into(), true), "a comment dropped");
        assert!(!r.caught_up(), "still behind");
        queued.recv().unwrap();
        assert!(r.send("4 $GPRMC".into(), false));
        assert!(r.caught_up(), "write everything as it stands again");
        assert!(!r.caught_up(), "once");
    }

    #[test]
    fn a_lock_is_named_for_the_whole_socket_name() {
        assert_eq!(
            lock_path(Path::new("/run/omakeel/keel.sock")),
            Path::new("/run/omakeel/keel.sock.lock")
        );
        assert_ne!(
            lock_path(Path::new("gps.sock")),
            lock_path(Path::new("gps.other"))
        );
        assert_eq!(lock_path(Path::new("gps.lock")), Path::new("gps.lock.lock"));
    }
}
