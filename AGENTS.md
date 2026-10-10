# Working on omakeel

omakeel is part of Omahoy, a suite of small Omarchy apps for sailors, each in
its own repo under github.com/shieldsworks. omakeel is the data hub. The
daemon `omakeel run` reads NMEA 0183 from a serial device, a TCP stream, or
a recording. It works out the boat's fix and the AIS traffic, and it serves
one state over a Unix socket as newline-delimited JSON. Every other Omahoy
app is a client of that socket. `omakeel watch` is the terminal client.
`docs/protocol.md` is the contract.

## Toolchain

Run `mise install` first. `mise.toml` pins Rust 1.98 with rustfmt and clippy.
`Cargo.toml`'s `rust-version` is 1.89, the oldest Rust the code promises to
build on. A system cargo older than that fails. `mise tasks` lists every job,
and each runs as `mise <job>`.

There is no window, so there are no GTK or Quickshell packages. `python3` is
only for `mise sample`.

## Done means verified

You are not done until this passes from the repo root:

```sh
scripts/verify.sh
```

It runs `mise lint`, then `mise test`, then checks that `clippy.toml`'s
`msrv` equals `rust-version`, and that verification left the checkout as it
found it. `mise lint` is rustfmt, clippy on all targets with warnings denied,
and `scripts/check-comments.sh`. CI runs the same script on x86_64 and
aarch64, then a release build. When `ui/*.qml` exists, CI also runs qmllint
with Quickshell's import warnings off. This repo has no `ui/` directory.
Fix what the script reports. Never weaken the check that reported it.

`mise test` drives the socket on a paused clock against
`tests/fixtures/berkeley-marina.nmea`. This repo has no golden files. The
sail test in `tests/hub.rs` checks that each JSON example in
`docs/protocol.md` has the same keys as the message the hub sends, and that
no message carries `null`.

To prove behavior with the hub running, follow
`.cursor/skills/verify/SKILL.md` and its feature map.

## Where each rule is enforced

| Rule | What fails when you break it |
|---|---|
| No `unwrap()` outside tests. `expect` only with the invariant that makes it safe. | clippy, `unwrap_used` and `expect_used` |
| Every `unsafe` block has a `// SAFETY:` comment. | clippy, `undocumented_unsafe_blocks` |
| A silenced lint says why. | clippy, `allow_attributes_without_reason` |
| No `dbg!`, `todo!`, or `unimplemented!`. | clippy |
| No apologetic or workaround comments. | `scripts/check-comments.sh`, inside `mise lint` |
| `docs/protocol.md`'s JSON examples have the keys the hub sends, and no key is `null`. | `tests/hub.rs` |
| Tests write only to a temp dir. | `scripts/verify.sh` |

To silence one lint at one site, put
`#[expect(clippy::<lint>, reason = "<the fact that makes this correct>")]`
on the smallest item. `#[expect]` fails once the code stops needing it.
`tests/hub.rs` starts with
`#![allow(clippy::unwrap_used, clippy::expect_used, reason = "a panic is how an integration test fails")]`,
because `clippy.toml` does not cover helper functions in `tests/`. No other
file gets a blanket allow.

## The gates are not yours to move

These files change only in a PR whose whole purpose is changing them, and a
human reviews that PR.

- `[lints]` in `Cargo.toml` and `clippy.toml`
- `.github/workflows/`, `scripts/verify.sh`, `scripts/check-comments.sh`
- the `lint` and `test` jobs in `mise.toml`

`clippy::pedantic` and the `cast_*` lints are off in this PR. CI denies
warnings, so turning them on fails the build until the hits are fixed.

## What no check catches

- Every behavior change needs a test or a golden. A bug fix starts with a
  failing test that reproduces the bug. New behavior lands with the test
  that pins it. A refactor changes no test expectation. If one has to
  change, it was not a refactor.
- Keep `docs/protocol.md` in sync. Change it in the same PR as any change
  to what the hub sends or records. The test checks the examples' keys, not
  the prose around them.
- The sample sail changes only through `scripts/sample-sail.py` and
  `mise sample`. Do not edit `tests/fixtures/` by hand. Say in the PR how
  the sail changed. Its vessels and MMSIs are invented.
- Comments state facts about the code and the world it handles. These fail
  `scripts/check-comments.sh`: TODO, FIXME, XXX, HACK, workaround, temporary
  fix, quick fix, for the time being, not ideal, should be fixed, sorry,
  kludge, band-aid. If something is wrong, fix it in this change or open an
  issue. A workaround for an outside bug is written as the fact. Name what
  the outside thing does, and what this code does about it.
- Minimal dependencies. `omakeel-protocol` uses `serde` and `serde_json`.
  The daemon also uses `libc` and `tokio`. Ask before adding a crate.
  Range, bearing, and CPA are computed in `src/targets.rs`. Do not add
  GDAL, geo, proj, or any map library.
  Serial ports use termios through `libc`. Always pass `--locked`. Change
  `Cargo.lock` only in a PR about dependencies.
- `unsafe` stays in the three libc wrappers. `open_serial` is in
  `src/source.rs`, `lock` is in `src/hub.rs`, and `local_time` is in
  `src/journal.rs`. Add another only when no safe std API exists.

## The agent that builds a change never approves it

The agent that wrote a change does not approve, merge, or mark it verified.
A different agent, with fresh context and a clean checkout, or Casey,
reviews it. They run `scripts/verify.sh` and drive the feature-map entries
the change touches.

The PR lists what changed, the tests that prove it, which feature-map
entries were driven, and what was not checked. The reviewer reports what
they ran and saw.

## Layout

- `src/main.rs` parses `run`, `watch`, and `--version`.
- `src/hub.rs` takes sources in and sends the fix and the traffic out. It binds the socket and holds the lock.
- `src/source.rs` reads serial, TCP, and replayed recordings.
- `src/nmea.rs` checks NMEA 0183 framing and checksums, and reads RMC and GGA.
- `src/fix.rs` holds the boat's fix and how fresh it is.
- `src/ais.rs` decodes AIS `!AIVDM` messages, types 1, 2, 3, 5, 18, 19, and 24.
- `src/targets.rs` holds the vessel table, range, bearing, CPA, and the danger flag.
- `src/journal.rs` writes the recording's status comments, and stderr on its own thread.
- `src/protocol.rs` is the wire messages that `docs/protocol.md` specifies. `protocol/` publishes that file as the `omakeel-protocol` crate.
- `src/watch.rs` is `omakeel watch`, one line per change.
- `tests/hub.rs` runs the hub end to end on the sample sail, on a paused clock.
- `tests/fixtures/berkeley-marina.nmea` is the synthetic sample sail.
- `scripts/sample-sail.py` writes that sail.
- `scripts/verify.sh` is the done command.
- `scripts/check-comments.sh` rejects the comment words listed above.
- `docs/protocol.md` is the socket protocol and the recording format.
