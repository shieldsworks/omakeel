//! The hub end to end: a recorded sail in, an app's view out, on a paused
//! clock so the minute-long sail replays in no real time.

use omakeel::{
    hub::{self, Config},
    source::Spec,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    net::UnixStream,
    time::{Duration, sleep},
};

const SAIL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/berkeley-marina.nmea"
);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omakeel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The sentences in a recording, in order, without their timestamps.
fn sentences(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split_once(' ').unwrap().1.to_string())
        .collect()
}

async fn connect(socket: &Path) -> Lines<BufReader<UnixStream>> {
    loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => return BufReader::new(stream).lines(),
            Err(_) => sleep(Duration::from_millis(10)).await,
        }
    }
}

async fn next(lines: &mut Lines<BufReader<UnixStream>>) -> Value {
    let line = lines.next_line().await.unwrap().expect("hub closed");
    serde_json::from_str(&line).unwrap()
}

fn replay(socket: &Path, record: Option<PathBuf>) -> Config {
    Config {
        sources: vec![Spec::Replay { path: SAIL.into() }],
        socket: socket.to_path_buf(),
        record,
    }
}

#[tokio::test(start_paused = true)]
async fn an_app_follows_the_sail_until_the_fix_goes_stale() {
    let dir = scratch("sail");
    let socket = dir.join("keel.sock");
    let record = dir.join("recorded.nmea");
    let hub = tokio::spawn(hub::run(replay(&socket, Some(record.clone()))));
    let mut lines = connect(&socket).await;

    let hello = next(&mut lines).await;
    assert_eq!(
        (hello["type"].as_str(), hello["v"].as_u64()),
        (Some("hello"), Some(1))
    );

    // RMC arrives first each second and GGA 40 ms later, so the first `ok`
    // has speed but not yet satellites; wait for a state with both.
    let ok = loop {
        let state = next(&mut lines).await;
        if state["fix"]["status"] == "ok" && state["fix"]["satellites"].is_number() {
            break state;
        }
    };
    let fix = &ok["fix"];
    assert!((fix["lat"].as_f64().unwrap() - 37.865).abs() < 0.001);
    assert!((fix["lon"].as_f64().unwrap() + 122.32).abs() < 0.001);
    assert_eq!(fix["sogKn"], 5.0);
    assert_eq!(fix["satellites"], 9);

    let recorded = sentences(Path::new(SAIL));
    let ended = loop {
        let state = next(&mut lines).await;
        if state["sources"][0]["status"] == "ended" {
            break state;
        }
    };
    // The sample sail has one sentence with a corrupted checksum.
    assert_eq!(ended["sources"][0]["sentences"], recorded.len() - 1);
    assert_eq!(ended["sources"][0]["rejected"], 1);

    let stale = loop {
        let state = next(&mut lines).await;
        if state["fix"]["status"] == "stale" {
            break state;
        }
    };
    assert!(stale["fix"]["ageSeconds"].as_u64().unwrap() >= 5);
    assert!(
        stale["fix"]["lat"].is_number(),
        "a stale fix still shows where it was"
    );

    hub.abort();
    let _ = hub.await;
    assert!(!socket.exists(), "the hub removes its socket");
    assert_eq!(
        sentences(&record),
        recorded,
        "the recording is every line, in order"
    );
}

#[tokio::test(start_paused = true)]
async fn a_second_hub_refuses_a_running_hubs_socket() {
    let dir = scratch("second");
    let socket = dir.join("keel.sock");
    let first = tokio::spawn(hub::run(replay(&socket, None)));
    let _lines = connect(&socket).await;
    let error = hub::run(replay(&socket, None)).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    first.abort();
}

#[tokio::test(start_paused = true)]
async fn recording_never_overwrites_a_sail() {
    let dir = scratch("overwrite");
    let record = dir.join("sail.nmea");
    std::fs::write(&record, "yesterday's sail\n").unwrap();
    let error = hub::run(replay(&dir.join("keel.sock"), Some(record.clone())))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        "yesterday's sail\n"
    );
}
