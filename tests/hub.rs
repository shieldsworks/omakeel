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

    // The sample's ferry is set to pass 0.2 nm from the boat in about five
    // minutes: a danger once both have positions, speeds and courses. The
    // danger can come before the names do (the cargo ship's arrives at 9 s),
    // so wait for all three vessels named and the danger flagged.
    let traffic = loop {
        let message = next(&mut lines).await;
        let Some(targets) = message["targets"].as_array() else {
            continue;
        };
        let named = targets.len() == 3 && targets.iter().all(|t| t["name"].is_string());
        if named && targets.iter().any(|t| t["danger"] == true) {
            break message;
        }
    };
    let vessel = |mmsi: u32| {
        traffic["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["mmsi"] == mmsi)
            .unwrap_or_else(|| panic!("{mmsi} missing"))
            .clone()
    };
    let ferry = vessel(366999101);
    assert_eq!(
        ferry["name"], "BAY RUNNER",
        "joined from a two-sentence type 5"
    );
    assert_eq!(
        (ferry["kind"].as_str(), ferry["class"].as_str()),
        (Some("passenger"), Some("A"))
    );
    assert_eq!(ferry["lengthM"], 40);
    assert!(ferry["cpaNm"].as_f64().unwrap() < 0.5);
    assert!(ferry["tcpaMinutes"].as_f64().unwrap() <= 12.0);
    let cargo = vessel(366999102);
    assert_eq!(cargo["name"], "PACIFIC TRADER");
    assert_eq!(cargo["danger"], false, "heading away");
    let sailboat = vessel(338999103);
    assert_eq!(sailboat["name"], "SEA LARK", "from a type 24 part A");
    assert_eq!(
        (sailboat["class"].as_str(), sailboat["kind"].as_str()),
        (Some("B"), Some("sailing"))
    );
    assert_eq!(sailboat["lengthM"], 11);

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

    // Between the sentences, what happened to the source and the fix, in
    // order, each stamped like a sentence.
    let news: Vec<String> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .skip(1) // the header
        .filter_map(|l| l.strip_prefix("# "))
        .map(|l| {
            let (ms, text) = l.split_once(' ').unwrap();
            assert!(ms.parse::<u64>().is_ok(), "{l}");
            text.to_string()
        })
        .collect();
    let at = |prefix: &str| {
        news.iter()
            .position(|t| t.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} in {news:#?}"))
    };
    let name = format!("source replay:{SAIL}");
    assert!(at("fix none") < at("fix ok"));
    assert!(at(&format!("{name} ok")) < at(&format!("{name} ended")));
    assert!(
        at(&format!("{name} ended")) < at("fix stale · last 9 satellites, hdop 0.9 · no sentences")
    );
    assert_eq!(
        news.iter()
            .filter(|t| t.starts_with(&format!("{name} ok")))
            .count(),
        1,
        "a source that stays up is written once: {news:#?}"
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
async fn a_socket_left_by_a_crashed_hub_is_replaced() {
    let dir = scratch("leftover");
    let socket = dir.join("keel.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists(), "a crash leaves the socket file behind");
    let hub = tokio::spawn(hub::run(replay(&socket, None)));
    let mut lines = connect(&socket).await;
    assert_eq!(next(&mut lines).await["type"], "hello");
    hub.abort();
}

#[tokio::test(start_paused = true)]
async fn an_app_that_leaves_is_let_go() {
    let dir = scratch("leaves");
    let socket = dir.join("keel.sock");
    let hub = tokio::spawn(hub::run(replay(&socket, None)));
    for _ in 0..3 {
        let mut lines = connect(&socket).await;
        assert_eq!(next(&mut lines).await["type"], "hello");
    }
    // The hub still serves a new app after others have come and gone.
    let mut lines = connect(&socket).await;
    assert_eq!(next(&mut lines).await["type"], "hello");
    assert_eq!(next(&mut lines).await["type"], "state");
    hub.abort();
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

/// A bridge that is up with nothing behind it: the Mac's socat accepting,
/// the GPS unplugged. The app sees the source quiet, and the recording says
/// the link was reached, not that it was down. Real time: about 6 s.
#[tokio::test]
async fn a_link_up_with_nothing_behind_it_goes_quiet() {
    let dir = scratch("silent-link");
    let socket = dir.join("keel.sock");
    let record = dir.join("recorded.nmea");
    let bridge = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = bridge.local_addr().unwrap().to_string();
    let held = tokio::spawn(async move {
        let (stream, _) = bridge.accept().await.unwrap();
        sleep(Duration::from_secs(60)).await;
        drop(stream);
    });
    let config = Config {
        sources: vec![Spec::Tcp {
            address: address.clone(),
        }],
        socket: socket.clone(),
        record: Some(record.clone()),
    };
    let hub = tokio::spawn(hub::run(config));
    let mut lines = connect(&socket).await;
    let quiet = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = next(&mut lines).await;
            if state["type"] == "state" && state["sources"][0]["status"] == "quiet" {
                break state;
            }
        }
    })
    .await
    .expect("the source should go quiet");
    assert!(quiet["sources"][0]["message"].is_null());
    hub.abort();
    let _ = hub.await;
    held.abort();
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(
        text.contains(&format!(
            "source tcp:{address} connected · nothing heard for 5 s"
        )),
        "{text}"
    );
}

/// The comment on a stale fix: whether the GPS was still talking.
async fn stale_comment(name: &str, recording: &str) -> String {
    let dir = scratch(name);
    let sail = dir.join("sail.nmea");
    std::fs::write(&sail, recording).unwrap();
    let record = dir.join("recorded.nmea");
    let config = Config {
        sources: vec![Spec::Replay { path: sail }],
        socket: dir.join("keel.sock"),
        record: Some(record.clone()),
    };
    let hub = tokio::spawn(hub::run(config));
    sleep(Duration::from_secs(30)).await;
    hub.abort();
    let _ = hub.await;
    std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .find_map(|l| l.split_once(" fix stale").map(|(_, rest)| rest.to_string()))
        .expect("the fix should go stale")
}

const RMC_3: &str = "$GPRMC,210003.00,A,3751.9000,N,12219.2000,W,5.0,255.0,130926,,,A*46";
const RMC_4: &str = "$GPRMC,210004.00,A,3751.8996,N,12219.2017,W,5.0,255.0,130926,,,A*40";

#[tokio::test(start_paused = true)]
async fn a_bursts_trailing_satellites_are_not_the_gps_still_talking() {
    // Each second: the position, then GSV nearly a second behind it. Then
    // the receiver dies after one last burst.
    let recording = format!(
        "# omakeel recording v1\n500 {RMC_3}\n1400 $GPGSV,3,3,09*00\n1500 {RMC_4}\n2400 $GPGSV,3,3,09*00\n"
    );
    assert_eq!(
        stale_comment("trailing-gsv", &recording).await,
        " · no sentences"
    );
}

#[tokio::test(start_paused = true)]
async fn a_receiver_talking_without_a_position_is_still_arriving() {
    // The position stops, but the receiver goes on sending satellites.
    let mut recording = format!("# omakeel recording v1\n500 {RMC_3}\n1500 {RMC_4}\n");
    for n in 2..12 {
        recording.push_str(&format!("{} $GPGSV,3,3,09*00\n", 500 + n * 1000));
    }
    assert_eq!(
        stale_comment("gsv-only", &recording).await,
        " · sentences still arriving"
    );
}

#[tokio::test(start_paused = true)]
async fn ais_on_the_gpss_own_stream_is_not_the_gps_still_talking() {
    // A multiplexer: GPS and AIS on one stream. The GPS stops; AIS goes on.
    let mut recording = format!("# omakeel recording v1\n500 {RMC_3}\n1500 {RMC_4}\n");
    for n in 2..12 {
        recording.push_str(&format!(
            "{} !AIVDM,1,1,,A,15M67FC000G?ufbE`FepT@3n00Sa,0*5C\n",
            500 + n * 1000
        ));
    }
    assert_eq!(stale_comment("mux", &recording).await, " · no sentences");
}
