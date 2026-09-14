# Omakeel

The data hub for [Omahoy](https://github.com/shieldsworks/omahoy). Everything is built
on the keel.

Omakeel is a headless Rust daemon. It reads the boat's instruments and serves
one live stream to every Omahoy app.

**Status: early.** It reads GPS and AIS and serves both.
[omalookout](https://github.com/shieldsworks/omalookout) shows its traffic in
the Omarchy bar. GPS is tested on a real USB receiver; AIS only on a replayed
sail so far.

## What it does now

- Reads NMEA 0183 from a USB or serial GPS, a TCP stream, or a recording, and
  checks every sentence's checksum.
- Takes position, speed and course over ground, UTC, satellites and HDOP from
  RMC and GGA.
- Serves one state to every app over a Unix socket, as newline-delimited JSON
  ([docs/protocol.md](docs/protocol.md)).
- Says when the fix is stale. Speed and course are never passed off as
  current once the GPS has stopped sending them.
- Decodes AIS from a receiver like the dAISy: position reports from class A
  ships and class B small craft, plus names, callsigns, types, sizes and
  destinations. It keeps every vessel heard in the last 10 minutes with
  range, bearing, and closest point of approach, and flags any that will
  pass within 0.5 nm in the next 12 minutes.
- Records every line it receives, AIS included, with the time it arrived.
  Replays those recordings in real time, so the apps can be developed at the
  dock.

## Use

Install from GitHub with Rust 1.89 or newer:

```sh
cargo install --git https://github.com/shieldsworks/omakeel --locked
```

Or build a checkout with [mise](https://mise.jdx.dev) and Rust 1.98:

```sh
mise install
mise build
```

Run on the sample sail out of Berkeley Marina, then watch it from another
terminal:

```sh
mise replay
mise watch
```

```
omakeel 0.1.0 · protocol v1
nofix   | replay:tests/fixtures/berkeley-marina.nmea ok (6 ok, 0 bad)
ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ok (7 ok, 0 bad)
AIS    3 vessels  · nearest SEA LARK 0.83 nm 301°T, CPA 0.83 nm in 0.0 min  DANGER BAY RUNNER
ok      37°51.897′N  122°19.215′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ok (36 ok, 0 bad)
```

The sample's vessels are invented. BAY RUNNER is a ferry set to cross 0.2 nm
from the boat, to show the collision alarm.

On the boat, with a USB GPS and a dAISy AIS receiver, recording the sail:

```sh
target/debug/omakeel run \
  --source serial:/dev/ttyUSB0:115200 \
  --source serial:/dev/ttyACM0:38400 \
  --record ~/sails/$(date +%F).nmea
```

Use your GPS's baud rate. Our BU-353-style puck sends at 115200; older
pucks send at 4800. At the wrong rate no fix ever comes: if `omakeel watch`
shows no good sentences, try the other.

A phone sharing its GPS over Wi-Fi works too: `--source tcp:192.168.1.20:10110`.

**In a VM with no USB**, such as Try Omarchy on a Mac, share the GPS from the
host over TCP instead. On the Mac, with socat from Homebrew (macOS socat takes
`ispeed` and `ospeed`, not `b115200`):

```sh
socat TCP-LISTEN:10110,bind=127.0.0.1,reuseaddr,fork \
  FILE:/dev/cu.usbserial-140,ispeed=115200,ospeed=115200,raw,echo=0
```

Binding to 127.0.0.1 keeps the GPS off the home network. In the VM, QEMU's
user-mode network reaches the Mac at 10.0.2.2:

```sh
omakeel run --source tcp:10.0.2.2:10110
```

## Next

- The anchor watch for [omanchor](https://github.com/shieldsworks/omanchor).
- A bar widget showing speed and course.
- Serving the same protocol across the boat's network.

## Develop

```sh
mise lint     # rustfmt and clippy
mise test     # unit tests and the socket test, on a paused clock
mise sample   # regenerate tests/fixtures/berkeley-marina.nmea
```

## License

MIT
