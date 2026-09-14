# omakeel protocol

Version 1. omakeel is the server. Every Omahoy app is a client.

## Transport

- A Unix stream socket, `$XDG_RUNTIME_DIR/omakeel/keel.sock` by default
  (`--socket` sets another). The directory is created with mode 0700.
- A lock file beside the socket (`keel.sock.lock`) keeps a second hub from
  starting on the same socket. A socket left behind by a crashed hub is
  replaced.
- Newline-delimited JSON, UTF-8, one object per line.
- Any number of apps may connect. Every app receives every message. Apps send
  nothing in version 1.
- Every message has `"type"` and `"v": 1`. An app that sees another `v` shows
  an error and stops using the data.
- Key order is not significant. Keys that don't apply are left out, never
  sent as `null`.
- Apps ignore message types and keys they don't know, so version 1 can grow
  without breaking them.
- An app that can't take a message within 2 seconds, or falls 64 messages
  behind, is disconnected. It can reconnect.

## Messages

`hello` is sent once on connect, followed by a full `state`.

```json
{"type":"hello","v":1,"keel":"0.1.0"}
```

`state` is the complete current state. It is re-sent whenever anything in it
changes, including `ageSeconds`, which the hub re-judges once a second. Apps
replace their copy rather than merging.

```json
{"type":"state","v":1,
 "fix":{"status":"ok","lat":37.864711,"lon":-122.3207314,"sogKn":5.0,"cogDeg":255.0,
        "utc":"2026-09-13T21:00:10Z","satellites":9,"hdop":0.9,"ageSeconds":0},
 "sources":[{"name":"serial:/dev/ttyUSB0:4800","status":"ok","sentences":1204,"rejected":2}]}
```

### `fix`

- `status` is one of:
  - `none`: no position sentence heard, or the receiver was still acquiring
    and has gone quiet.
  - `nofix`: the receiver is talking but reports no fix (RMC status `V`, GGA
    quality 0). The last known position, if any, stays in the message with
    its age.
  - `ok`: a fix less than 5 seconds old.
  - `stale`: the last fix is 5 seconds old or more. The position stays in
    the message with its age. Apps show it as stale, never as current.
- Only a real fix counts: RMC status `A` in a mode that isn't estimated,
  manual, simulated or not valid (`E`, `M`, `S`, `N`), or GGA quality 1 to 5.
  Dead reckoning (6), manual input (7) and simulation (8) are `nofix`.
  Coordinates must have exactly two whole-minute digits, `ddmm.mmmm`, because
  the checksum can't catch a moved decimal point.
- `lat` and `lon` are decimal degrees, WGS 84, rounded to 7 places. South and
  west are negative.
- `sogKn` and `cogDeg` are speed in knots and course in degrees true, over
  ground. `utc` is the receiver's time, whole seconds. All three come from
  RMC. Each goes missing when RMC leaves it empty, or when the newest RMC is
  5 seconds older than the newest position.
- `satellites` and `hdop` come from GGA, and go missing the same way.
- `ageSeconds` is whole seconds since the fix arrived at the hub, measured by
  the hub's clock, not the receiver's.
- With more than one position source, the most recent sentence from any of
  them wins. There's no preferred GPS yet, so run one GPS at a time until
  there is.

### `sources`

One entry per `--source`, in command-line order.

- `name` is the source as given: `serial:PATH:BAUD`, `tcp:HOST:PORT` or
  `replay:FILE`.
- `status` is one of:
  - `connecting`: nothing received yet.
  - `ok`: receiving.
  - `quiet`: nothing for 5 seconds.
  - `error`: can't be opened or reached, and `message` says why. Serial and
    TCP sources retry every 2 seconds. A replay that can't be read stays in
    `error`.
  - `ended`: a replay reached the end of its recording.
- `sentences` counts lines that checked out as NMEA sentences. `rejected`
  counts the rest: bad checksums and garbage.

### `targets`

Every vessel heard on AIS, sent after `state` on connect, then once a second
whenever anything in it has changed.

```json
{"type":"targets","v":1,"targets":[
 {"mmsi":366999101,"name":"BAY RUNNER","callsign":"WDX9101","shipType":60,"kind":"passenger",
  "class":"A","status":"under way using engine","lat":37.8809371,"lon":-122.3463369,
  "sogKn":20.0,"cogDeg":150.0,"headingDeg":150,"lengthM":40,"beamM":10,
  "destination":"SF FERRY BLDG","ageSeconds":1,"rangeNm":1.52,"bearingDeg":311.2,
  "cpaNm":0.19,"tcpaMinutes":4.2,"danger":true}]}
```

- One entry per vessel heard in the last 10 minutes, up to 1000. They're
  sorted nearest first, and vessels that can't be ranged come last. When the
  table is full, a new vessel replaces the one least worth keeping: first a
  vessel with no position, then the one heard longest ago.
- omakeel decodes `!AIVDM` position reports (types 1, 2 and 3 from class A;
  18 and 19 from class B) and particulars (types 5, 19 and 24). A message
  split across sentences is joined per source and channel, so messages on
  channels A and B can interleave. If one of its sentences is lost, so is the
  message. A message too short to hold every field omakeel reads for its
  type is dropped: 168 bits for types 1, 2, 3 and 18, 312 for 19, 422 for 5
  (whose last two bits omakeel doesn't read), and 160 and 162 for a type 24
  part A and part B. Fill bits run 0 to 5 on a message's last sentence and
  are 0 on the others. Own-ship `!AIVDO` is ignored.
- When the table is full, a vessel with a position is only displaced by
  another position report, never by particulars alone.
- Particulars build up across messages. A class B sends its name in one
  type 24 message and its type, callsign and size in another.
- `mmsi` is the vessel's identity. `kind` is `shipType` in words, and
  `class` is `A` or `B`. `status` is class A's navigational status in words.
- `lat`, `lon`, `sogKn`, `cogDeg` and `headingDeg` are as last reported.
  `ageSeconds` is how long ago that report arrived. A position more than
  10 minutes old is dropped, even while the vessel's particulars keep
  arriving.
- `lengthM` and `beamM` are summed from the reported distances of the GPS
  antenna to each side. An auxiliary craft (MMSI 98...) sends its mother
  ship's MMSI there instead, so it has no size.
- `rangeNm` and `bearingDeg` (true, from the boat) are sent only with an `ok`
  fix. They use the vessel's position carried forward along its reported
  course and speed to now, the way a chartplotter shows a target between
  reports.
- `cpaNm` and `tcpaMinutes` are the closest point of approach and the
  minutes until it. They need both vessels' speed and course. A vessel under
  0.5 knots with no course counts as stopped. For a vessel already past its
  closest, `cpaNm` is the range now and `tcpaMinutes` is 0.
- `danger` is true for a CPA of 0.5 nm or less within 12 minutes. These
  thresholds are fixed for now.
- Distances are flat-earth from the boat, which is off by a small fraction
  of a mile at the edge of AIS range.

## Recordings

`--record FILE` writes every line received from every source, rejected lines
included, stamped with the time it arrived. Each line is trimmed of surrounding
whitespace, invalid UTF-8 is replaced, and blank lines are skipped. omakeel
never overwrites an existing file.

```
# omakeel recording v1
1789333203000 $GPRMC,210003.00,A,3751.9000,N,12219.2000,W,5.0,255.0,130926,,,A*46
1789333203040 $GPGGA,210003.00,3751.9000,N,12219.2000,W,1,09,0.9,2.1,M,-32.2,M,,*51
```

- Lines starting with `#` are comments.
- Every other line is Unix milliseconds, one space, then the line.
- Lines are written on a thread of their own and synced to disk about every
  10 seconds, idle or not. A power cut can lose the lines written since the
  last sync, and more if the disk itself has stalled. If the disk falls far
  behind, lines are dropped and omakeel says so on stderr, rather than
  stalling navigation. A failed write or sync stops the recording, with a
  message.
- On exit, omakeel waits up to 5 seconds for the last lines to reach the
  disk.

`replay:FILE` plays a recording back in real time. FILE must be a regular
file. It plays once, then the source reports `ended` and the fix goes stale.
