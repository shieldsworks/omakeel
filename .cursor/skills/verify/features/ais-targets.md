# AIS targets

Every vessel the AIS receiver hears, nearest first, with range, bearing,
and closest point of approach. A vessel that will pass within 0.5 nm in the
next 12 minutes is flagged. Apps get the `targets` message. A sailor sees
one `AIS` line per change in `omakeel watch`.

## Sub-features

- `targets-names` prints BAY RUNNER, SEA LARK, and PACIFIC TRADER.
- `targets-danger` prints `DANGER BAY RUNNER`.
- `targets-socket` shows MMSI 366999101 with `"danger":true` and `"name":"BAY RUNNER"`.

## How to get to it (user POV)

- Run `omakeel watch` against a running hub and read the `AIS` lines.
- Connect an app to the socket and read `targets` messages.

## Driving it with the replayed sail

Preconditions:

- The hub from `../SKILL.md` is up, launched less than 10 seconds ago.
- Doctor passes.
- `art="$run/artifacts/verify/ais-targets"; mkdir -p "$art"`

- **Watch the traffic.** A sailor reads the terminal. Run the block below. Exit 124 from `timeout` is expected. The transcript contains `DANGER BAY RUNNER`, `SEA LARK`, and `PACIFIC TRADER`.

```sh
status=0
timeout 20 ./target/debug/omakeel watch --socket "$run/sock" \
  >"$art/watch.txt" 2>"$art/watch.err" || status=$?
echo "$status" >"$art/watch.exit"
test "$status" = 124
grep -F 'DANGER BAY RUNNER' "$art/watch.txt"
grep -F 'SEA LARK' "$art/watch.txt"
grep -F 'PACIFIC TRADER' "$art/watch.txt"
```

- **Read what an app reads.** In parallel with watch, run the socket reader from `../SKILL.md` and save it.

```sh
timeout 20 python3 - "$run/sock" <<'EOF' >"$art/socket.jsonl"
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
for line in s.makefile():
    print(line, end="", flush=True)
EOF
python3 - "$art/socket.jsonl" <<'EOF'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
targets = [row for row in rows if row["type"] == "targets"]
ferry = {item["mmsi"]: item for item in targets[-1]["targets"]}[366999101]
assert ferry["name"] == "BAY RUNNER" and ferry["danger"] is True
print(ferry["name"], ferry["danger"])
EOF
```

- **Proof.** The two greps and the python print are the pass. Save `watch.txt`, `watch.err`, `watch.exit`, and `socket.jsonl` under `$art`.

## Gotchas

- The first `AIS` line can name a vessel `MMSI …` because the name arrives in a later message. Grep the whole transcript.
- Range and CPA change as the sail plays. Do not pin those numbers.
- `watch` exits only when the hub closes the socket, so `timeout` ends it with 124.
- Range and the danger flag need an `ok` fix. The sample is still acquiring for its first few seconds.
