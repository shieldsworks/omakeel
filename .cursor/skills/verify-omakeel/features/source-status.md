# Source status

The sailor sees each source's name, status, good sentences, and rejected
sentences on the watch line, after the `|`.

## Sub-features

- `source-ok` shows `replay:tests/fixtures/berkeley-marina.nmea ok` during the sail.
- `source-ended` shows that source as `ended` after the last sentence.
- `source-rejected` counts one rejected sentence, the corrupted checksum.

## How to get to it (user POV)

- Read the part of an `omakeel watch` line after `|`.
- An app reads `state.sources`.

## Driving it with the replayed sail

Preconditions:

- The long watch from `live-fix.md`, or a fresh `timeout 80` on a new launch.
- `art="$run/artifacts/verify/source-status"; mkdir -p "$art"`

- **Watch the source through the end of the recording.**

```sh
timeout 80 ./target/debug/omakeel watch --socket "$run/sock" \
  >"$art/watch.txt" 2>"$art/watch.err" || true
grep -F 'replay:tests/fixtures/berkeley-marina.nmea ok' "$art/watch.txt"
grep -E 'replay:tests/fixtures/berkeley-marina.nmea ended \([0-9]+ ok, 1 bad\)' "$art/watch.txt"
```

Observed on 2026-10-13. Exit 124. The greps printed:

```
nofix   | replay:tests/fixtures/berkeley-marina.nmea ok (1 ok, 0 bad)
ok      37°51.879′N  122°19.300′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ended (156 ok, 1 bad)
```

- **TCP, in place of a live GPS.** This VM has no serial device. A python process on `127.0.0.1:10110` sent the sample's first valid RMC and GGA, then held the socket open for 8 seconds. Launch a fresh hub. Do not reuse the replay `$run`.

```sh
tcp=$(mktemp -d /tmp/omakeel-tcp.XXXXXX)
python3 - "$tcp" <<'EOF' &
import socket, sys, time
path = sys.argv[1]
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 10110))
srv.listen(1)
srv.settimeout(15)
open(path + "/server.ready", "w").write("listening\n")
conn, _ = srv.accept()
conn.sendall(
    b"$GPRMC,210003.00,A,3751.9000,N,12219.2000,W,5.0,255.0,130926,,,A*46\r\n"
    b"$GPGGA,210003.00,3751.9000,N,12219.2000,W,1,09,0.9,2.1,M,-32.2,M,,*51\r\n"
)
time.sleep(8)
conn.close()
srv.close()
EOF
echo $! >"$tcp/server.pid"
for _ in $(seq 50); do test -f "$tcp/server.ready" && break; sleep 0.1; done
./target/debug/omakeel run \
  --source tcp:127.0.0.1:10110 \
  --socket "$tcp/sock" \
  --record "$tcp/recorded.nmea" \
  >"$tcp/hub.log" 2>&1 &
echo $! >"$tcp/pid"
for _ in $(seq 50); do test -S "$tcp/sock" && break; sleep 0.1; done
timeout 8 ./target/debug/omakeel watch --socket "$tcp/sock" \
  >"$tcp/watch.txt" 2>"$tcp/watch.err" || true
grep -F 'tcp:127.0.0.1:10110 ok' "$tcp/watch.txt"
grep -F '37°51.900′N' "$tcp/watch.txt"
kill "$(cat "$tcp/pid")"
kill "$(cat "$tcp/server.pid")" 2>/dev/null || true
```

Observed on 2026-10-13. Watch exit 124. The first fix line and the later quiet line:

```
ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | tcp:127.0.0.1:10110 ok (2 ok, 0 bad)
stale   37°51.900′N  122°19.200′W   5.0 kn  255°T  5s ago  | tcp:127.0.0.1:10110 quiet (2 ok, 0 bad)
```

`"$tcp/hub.log"` contained:

```
omakeel: 2026-10-13 07:03:40 source tcp:127.0.0.1:10110 ok
omakeel: 2026-10-13 07:03:40 fix ok
omakeel: 2026-10-13 07:03:48 source tcp:127.0.0.1:10110 error: 127.0.0.1:10110: connection closed
```

The recording contained both sentences and `# omakeel recording v1`.

- **Serial, with no receiver.** The command below reaches the serial path. `/dev/ttyUSB0` is absent, so the source stays in `error`. Kill the pid this block starts.

```sh
ser=$(mktemp -d /tmp/omakeel-serial.XXXXXX)
./target/debug/omakeel run \
  --source serial:/dev/ttyUSB0:4800 \
  --socket "$ser/sock" \
  --record "$ser/recorded.nmea" \
  >"$ser/hub.log" 2>&1 &
echo $! >"$ser/pid"
for _ in $(seq 50); do test -S "$ser/sock" && break; sleep 0.1; done
timeout 4 ./target/debug/omakeel watch --socket "$ser/sock" >"$ser/watch.txt" 2>"$ser/watch.err" || true
grep -F 'serial:/dev/ttyUSB0:4800 error' "$ser/watch.txt"
grep -F 'No such file or directory' "$ser/hub.log"
kill "$(cat "$ser/pid")"
```

Observed on 2026-10-13:

```
none    | serial:/dev/ttyUSB0:4800 error (0 ok, 0 bad)
omakeel: 2026-10-13 07:03:53 source serial:/dev/ttyUSB0:4800 error: /dev/ttyUSB0: No such file or directory (os error 2)
```

- **Proof.** The replay `ended` line shows one rejected sentence. `tests/hub.rs` asserts the same count on the socket. The TCP lines show a live source. The serial lines show the missing device.

## Gotchas

- A 20 second capture can show `ok` and may show the rejected count. It cannot show `ended`.
- `serial:/dev/ttyUSB0` needs a receiver. On a machine without one, the error block above is the result. A GPS on that path is still unchecked.
- A quiet TCP link is the test `a_link_up_with_nothing_behind_it_goes_quiet`. The TCP block above went quiet after the two sentences, then `error` when the peer closed. A link that never sends a line needs its own temp recording and about ten seconds.
