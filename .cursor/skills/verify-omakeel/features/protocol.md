# Protocol

An app connects to the hub's Unix socket and reads newline-delimited JSON.
The first line is `hello`, then `state`, then `targets` as the sail changes.
Version 1 defines no message for the app to send. A line written on the
socket does not stop the stream.

## Sub-features

- `protocol-hello` is `{"keel":"0.1.0","type":"hello","v":1}`.
- `protocol-state` is a `state` line whose fix status becomes `ok` at 37.865 N, 122.32 W.
- `protocol-write` sends one JSON line and keeps receiving `state` and `targets`.

## How to get to it (user POV)

- An Omahoy app connects to the socket `omakeel run` bound.
- The app reads one JSON object per line, as `docs/protocol.md` describes.
- The app writes nothing.

## Driving it with the replayed sail

Preconditions:

- The hub from `../SKILL.md` is up.
- `art="$run/artifacts/verify/protocol"; mkdir -p "$art"`

- **Read the lines, then write one.** Connect, take the first line, send a line the protocol does not define, and keep reading.

```sh
python3 - "$run/sock" "$art/write.txt" <<'EOF'
import socket, sys, time
path, outp = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX)
s.connect(path)
s.settimeout(2)
buf = b""
lines = []

def pull():
    global buf
    while b"\n" in buf:
        line, buf = buf.split(b"\n", 1)
        lines.append(line.decode())

deadline = time.time() + 3
while time.time() < deadline and not lines:
    try:
        chunk = s.recv(4096)
    except socket.timeout:
        break
    if not chunk:
        break
    buf += chunk
    pull()
sent = b'{"type":"nope","v":1}\n'
s.sendall(sent)
deadline = time.time() + 6
while time.time() < deadline:
    try:
        chunk = s.recv(4096)
    except socket.timeout:
        break
    if not chunk:
        break
    buf += chunk
    pull()
kinds = []
for line in lines:
    if '"type":"' in line:
        kinds.append(line.split('"type":"', 1)[1].split('"', 1)[0])
open(outp, "w").write(
    "sent " + sent.decode()
    + "lines " + str(len(lines)) + "\n"
    + "kinds " + " ".join(kinds[:12]) + "\n"
    + "first " + (lines[0] if lines else "") + "\n"
    + "state " + next((line for line in lines if '"type":"state"' in line), "") + "\n"
)
EOF
grep -F 'sent {"type":"nope","v":1}' "$art/write.txt"
grep -F '{"keel":"0.1.0","type":"hello","v":1}' "$art/write.txt"
grep -F 'kinds hello state' "$art/write.txt"
```

Observed on 2026-10-13. `$art/write.txt` began:

```
sent {"type":"nope","v":1}
lines 27
kinds hello state targets state state targets state state state targets state state
first {"keel":"0.1.0","type":"hello","v":1}
state {"fix":{"status":"nofix"},"sources":[{"name":"replay:tests/fixtures/berkeley-marina.nmea","rejected":0,"sentences":1,"status":"ok"}],"type":"state","v":1}
```

The same sail's `socket.jsonl` from `ais-targets.md`, once the fix was ok:

```
{"fix":{"ageSeconds":0,"cogDeg":255.0,"lat":37.865,"lon":-122.32,"sogKn":5.0,"status":"ok","utc":"2026-09-13T21:00:03Z"},"sources":[{"name":"replay:tests/fixtures/berkeley-marina.nmea","rejected":0,"sentences":10,"status":"ok"}],"type":"state","v":1}
```

That ok line came from the 20 second capture in `ais-targets.md`:

```sh
python3 - "$run/artifacts/verify/ais-targets/socket.jsonl" <<'EOF'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
ok = next(row for row in rows if row["type"] == "state" and row["fix"]["status"] == "ok")
print(json.dumps(ok, separators=(",", ":")))
EOF
```

- **Proof.** `write.txt` shows `hello` and further `state` lines after the client write. The longer capture shows a `state` whose status is `ok` at 37.865 N, 122.32 W.

## Gotchas

- Match keys. The order in `docs/protocol.md`'s examples can differ from the line the hub sends.
- The first `state` can be `nofix`. Wait for `"status":"ok"` before treating the position as the fix.
- A positive read on the app's socket keeps the connection. Closing the socket is what drops the client.
