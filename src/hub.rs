//! The hub: sentences in from every source, one state out to every app.

use crate::fix::{Navigation, STALE_AFTER};
use crate::nmea;
use crate::protocol::{Message, SourceState, SourceStatus, VERSION};
use crate::source::{self, Event, Spec};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
    sync::mpsc,
    time::{Duration, Instant, MissedTickBehavior, interval, sleep, timeout},
};

/// Messages queued for one app. An app that falls this far behind is dropped.
const QUEUE: usize = 64;
/// An app that can't take one message in this long has stalled.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Runs until it fails to bind or record. The socket file is removed when
/// the returned future ends or is dropped.
pub async fn run(config: Config) -> io::Result<()> {
    let mut recorder = config.record.as_deref().map(Recorder::create).transpose()?;
    let (listener, _socket) = bind(&config.socket)?;
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
                        && let Err(e) = r.write(&line)
                    {
                        eprintln!("omakeel: stopped recording to {}: {e}", r.path.display());
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

/// One connected app, fed through a queue by its own writer task.
struct Client {
    tx: mpsc::Sender<Arc<str>>,
    /// Hasn't been sent the current state yet.
    fresh: bool,
}

impl Client {
    fn spawn(mut stream: UnixStream) -> Client {
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(QUEUE);
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if !matches!(
                    timeout(WRITE_TIMEOUT, stream.write_all(line.as_bytes())).await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
        });
        Client { tx, fresh: true }
    }
}

/// Binds the socket, replacing a leftover one from a hub that didn't exit
/// cleanly, and refusing to start beside a hub that's still running.
fn bind(path: &Path) -> io::Result<(UnixListener, SocketFile)> {
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and isn't a socket", path.display()),
            ));
        }
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("omakeel is already running on {}", path.display()),
            ));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    Ok((listener, SocketFile(path.to_path_buf())))
}

struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Every line as it arrived, stamped with Unix milliseconds
/// (`docs/protocol.md`, recordings). Flushed per line: a boat loses power.
struct Recorder {
    out: BufWriter<File>,
    path: PathBuf,
}

impl Recorder {
    fn create(path: &Path) -> io::Result<Recorder> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let mut out = BufWriter::new(file);
        writeln!(out, "# omakeel recording v1")?;
        out.flush()?;
        Ok(Recorder {
            out,
            path: path.to_path_buf(),
        })
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        writeln!(self.out, "{ms} {line}")?;
        self.out.flush()
    }
}
