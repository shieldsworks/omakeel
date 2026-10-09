# Live fix

The sailor sees the fix go from acquiring, to a position, to stale after the
recording ends. The last position stays on the stale line.

## Sub-features

- `fix-ok` is a watch line that starts with `ok` and shows the marina position.
- `fix-stale` is a watch line that starts with `stale` after the recording ends, with the position still present.

## How to get to it (user POV)

- Run `omakeel watch` while `omakeel run` replays the sail.
- The status word is the start of the line.

## Driving it with the replayed sail

Preconditions:

- A fresh launch from `../SKILL.md`, with watch started as soon as the socket exists.
- `art="$run/artifacts/verify/live-fix"; mkdir -p "$art"`

- **Read the fix through the sail.** The recording is about 63 seconds, and a fix goes stale 5 seconds after the last position. `timeout 80` covers that.

```sh
timeout 80 ./target/debug/omakeel watch --socket "$run/sock" \
  >"$art/watch.txt" 2>"$art/watch.err" || true
grep '^ok ' "$art/watch.txt"
grep -F '37°51.900′N' "$art/watch.txt"
grep '^stale ' "$art/watch.txt"
```

- **Proof.** `watch.txt` shows both an `ok` line and a `stale` line. The paused-clock regression is `an_app_follows_the_sail_until_the_fix_goes_stale` in `tests/hub.rs`.

## Gotchas

- A watch that starts after the recording ends can see `stale` without `ok`. Start watch as soon as the socket exists.
- The position on a stale line is the last fix, not a blank.
- Do not match `Ns ago` or `ageSeconds`.
