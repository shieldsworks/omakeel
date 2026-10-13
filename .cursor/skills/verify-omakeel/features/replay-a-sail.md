# Replay a sail

Play the Berkeley Marina recording through the hub and read it from another
terminal, on a socket this run created.

## Sub-features

- `replay-hello` prints `omakeel 0.1.0 · protocol v1`.
- `replay-position` prints `37°51.900′N` and `5.0 kn`.
- `replay-source` names `replay:tests/fixtures/berkeley-marina.nmea` on that line.

## How to get to it (user POV)

- In one terminal, `omakeel run --source replay:tests/fixtures/berkeley-marina.nmea`.
- In another, `omakeel watch`.
- The README's `mise replay` and `mise follow` are the same pair on the default socket. A proof does not use them.

## Driving it with the replayed sail

Preconditions:

- The launch and the doctor check in `../SKILL.md` have run.
- `art="$run/artifacts/verify/replay-a-sail"; mkdir -p "$art"`

- **Watch the sail.** A user looks at the terminal. Exit 124 from `timeout` is expected.

```sh
status=0
timeout 20 ./target/debug/omakeel watch --socket "$run/sock" \
  >"$art/watch.txt" 2>"$art/watch.err" || status=$?
test "$status" = 124
grep -F 'omakeel 0.1.0 · protocol v1' "$art/watch.txt"
grep -F '37°51.900′N' "$art/watch.txt"
grep -F '5.0 kn' "$art/watch.txt"
grep -F 'replay:tests/fixtures/berkeley-marina.nmea' "$art/watch.txt"
```

Observed on 2026-10-13. Exit 124. stderr empty. The greps printed:

```
omakeel 0.1.0 · protocol v1
ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ok (10 ok, 0 bad)
ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ok (10 ok, 0 bad)
nofix   | replay:tests/fixtures/berkeley-marina.nmea ok (1 ok, 0 bad)
```

The first lines of `watch.txt` were:

```
omakeel 0.1.0 · protocol v1
nofix   | replay:tests/fixtures/berkeley-marina.nmea ok (1 ok, 0 bad)
AIS    0 vessels
```

- **Second look.** The recording contains the same RMC the fixture played, and `tests/fixtures/` is unchanged.

```sh
grep -F '$GPRMC,210003.00,A,3751.9000,N' "$run/recorded.nmea"
test "$(git status --porcelain --untracked-files=all -- tests/fixtures)" = ""
```

Observed on 2026-10-13. The RMC grep printed one line, and `git status` printed nothing:

```
1791874604288 $GPRMC,210003.00,A,3751.9000,N,12219.2000,W,5.0,255.0,130926,,,A*46
```

- **Proof.** `watch.txt` is the transcript. The recording is the second look.

## Gotchas

- `mise watch` runs mise's file watcher. The CLI is `omakeel watch`. The mise task is `mise follow`, and it has no `--socket` flag.
- Do not require `0s ago`. Age moves.
- `timeout 20` covers the first fix. It does not cover `ended` or a stale fix.
