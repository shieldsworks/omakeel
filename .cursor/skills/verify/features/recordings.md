# Recordings

The sailor points `--record` at a new file and later reads why the fix
changed, as comment lines, without losing an existing sail.

## Sub-features

- `record-header` writes `# omakeel recording v1` as the first line.
- `record-sentence` writes the fixture's first valid RMC, with a millisecond prefix this check does not pin.
- `record-no-clobber` leaves an existing file untouched and exits 1.

## How to get to it (user POV)

- `omakeel run --record FILE`, then open the file.
- `grep '^#' FILE` shows why the fix changed.

## Driving it with the replayed sail

Preconditions:

- The launch in `../SKILL.md` already passes `--record "$run/recorded.nmea"`.
- `art="$run/artifacts/verify/recordings"; mkdir -p "$art"`

- **Read the recording while the hub is up.** The header is written before the socket accepts. Sentence lines follow as the recorder flushes.

```sh
grep -F '# omakeel recording v1' "$run/recorded.nmea"
grep -F '$GPRMC,210003.00,A,3751.9000,N,12219.2000,W,5.0,255.0,130926,,,A*46' \
  "$run/recorded.nmea"
```

- **Refuse to overwrite.** Stop the hub, copy the file, and start a second hub on the same path.

```sh
kill "$(cat "$run/pid")" 2>/dev/null || true
for _ in $(seq 50); do
  kill -0 "$(cat "$run/pid")" 2>/dev/null || break
  sleep 0.1
done
cp "$run/recorded.nmea" "$art/recorded.nmea"
./target/debug/omakeel run \
  --source replay:tests/fixtures/berkeley-marina.nmea \
  --socket "$run/second.sock" \
  --record "$run/recorded.nmea" \
  >"$art/clobber.err" 2>&1 || true
grep -F 'recorded.nmea' "$art/clobber.err"
cmp "$run/recorded.nmea" "$art/recorded.nmea"
```

- **Proof.** The header and the RMC are in the file. `cmp` shows the second hub did not change it.

## Gotchas

- Read a finished recording after the hub exits when you care about the last lines. The recorder syncs about every 10 seconds and again on exit.
- Never point `--record` at `tests/fixtures/`. `scripts/verify.sh` fails if the checkout changes.
- `mise sample` regenerates the fixture when the generator changes. A proof does not run it.
