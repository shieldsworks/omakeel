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

- **Proof.** The `ended` line shows one rejected sentence. `tests/hub.rs` asserts the same count on the socket.

## Gotchas

- A 20 second capture can show `ok` and may show the rejected count. It cannot show `ended`.
- `serial:` is unreached without a receiver. Report the command you would have run and the missing device.
- A quiet TCP link is the test `a_link_up_with_nothing_behind_it_goes_quiet`. A live proof of that case needs its own temp recording and about ten seconds.
