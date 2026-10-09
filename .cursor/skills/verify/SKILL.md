---
name: verify-omakeel
description: "Drive omakeel the way a sailor and an app do (the hub on a replayed sail, its socket, omakeel watch, the recording) and keep proof. Use before calling any behavior change done, when reviewing someone else's change, or when asked to verify omakeel."
---

# Verify omakeel

`scripts/verify.sh` proves the code is sound. This skill proves the hub
works. It launches the binary from this checkout, drives a feature from the
user's side, and keeps evidence. Run both. The agent that built a change
does not sign off on this run.

## Launch

Run from the repo root. Every run gets its own directory. Do not use
`mise replay` or `mise follow`. Both talk to the default socket.

```sh
mise install
cargo build --locked
run=$(mktemp -d /tmp/omakeel-verify.XXXXXX)
mkdir -p "$run/artifacts/verify"
./target/debug/omakeel run \
  --source replay:tests/fixtures/berkeley-marina.nmea \
  --socket "$run/sock" \
  --record "$run/recorded.nmea" \
  >"$run/hub.log" 2>&1 &
echo $! >"$run/pid"
for _ in $(seq 50); do
  test -S "$run/sock" && break
  sleep 0.1
done
test -S "$run/sock"
```

Ready when `$run/sock` is a socket and the pid in `$run/pid` is alive. The
replay plays the sample sail once, in real time. The source then reports
`ended`, and the fix goes stale. Launch again for another run.

## Doctor

```sh
./target/debug/omakeel --version
test -S "$run/sock"
kill -0 "$(cat "$run/pid")"
echo "pid $(cat "$run/pid") serving $run/sock"
```

The version line is this checkout's binary. The pid is the process this run
started. The socket is under `$run`. Do not connect to a hub this run did
not start.

## Drive

Follow `features/`. Two paths talk to `$run/sock`.

- A sailor's terminal is `./target/debug/omakeel watch --socket "$run/sock"`.
- An app reads the socket, one JSON object per line, as `docs/protocol.md`
  describes.

```sh
python3 - "$run/sock" <<'EOF'
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
for line in s.makefile():
    print(line, end="", flush=True)
EOF
```

`timeout` ends `watch` with exit 124 while the hub is still running. Record
that exit and judge the output file.

## Evidence

Write under `$run/artifacts/verify/<feature-id>/`. Copy that directory out
of `/tmp` if it has to outlive the machine.

- Save the command, stdout, stderr, and the exit code.
- For the socket, save the lines. The second look is `$run/recorded.nmea`
  after the hub exits, or the next socket message.
- The replay source is the committed fixture. The run only reads it.
- `--record` points at a path under `$run` that does not exist yet.
- If a path cannot be reached, write the command and the missing
  precondition. Do not mark that path passed.

## Cleanup

```sh
kill "$(cat "$run/pid")" 2>/dev/null || true
for _ in $(seq 50); do
  kill -0 "$(cat "$run/pid")" 2>/dev/null || break
  sleep 0.1
done
test ! -e "$run/sock" && echo "hub removed its socket"
```

Kill only the pid this run started. Evidence stays under `$run/artifacts`.
