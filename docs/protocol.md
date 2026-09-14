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
  - `error`: can't be opened or reached. It's retried every 2 seconds, and
    `message` says why.
  - `ended`: a replay reached the end of its recording.
- `sentences` counts lines that checked out as NMEA sentences. `rejected`
  counts the rest: bad checksums and garbage.

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
- Lines are written on a thread of their own, and each reaches the disk
  within 10 seconds, idle or not, so a power cut loses at most the last 10
  seconds. If the disk falls far behind, lines are dropped and omakeel says
  so on stderr, rather than stalling navigation. A failed write or sync
  stops the recording, with a message.
- On exit, omakeel waits up to 5 seconds for the last lines to reach the
  disk.

`replay:FILE` plays a recording back in real time. FILE must be a regular
file. It plays once, then the source reports `ended` and the fix goes stale.
