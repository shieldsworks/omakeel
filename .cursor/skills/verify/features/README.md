# omakeel verification map

This directory is the source for verifying omakeel's user-facing behavior.
Read this index, then use the matching feature file as the recipe. A change
that adds or alters a user path updates its feature file in the same PR.

## Baseline preconditions

- A build of this checkout, launched per `../SKILL.md`, with its own `$run`
  directory for the socket, the recording, and the logs.
- The replay source is `tests/fixtures/berkeley-marina.nmea`. The run reads
  it and does not write it.
- Doctor passes and names this run's process and socket.
- Never drive a hub this run did not start.

## Driving conventions

- Start every recipe from the baseline unless its preconditions say otherwise.
- Commands are literal. Keep quoted names and flags unchanged.
- Prefer stable handles. Message types and keys come from `docs/protocol.md`.
  CLI flags and recording comment lines are the other handles.
- `omakeel watch` lines carry ages that change from run to run. Match the
  parts a recipe names, not the whole line.
- Do not delete proof during cleanup.

## Proof and skip reporting

- CLI proof is the command, stdout, stderr, and the exit code.
- Socket proof is the reply lines, verbatim.
- Mutation proof is a second read of the recording on disk.
- Record the feature id with every artifact.
- An unreachable path is reported with the command tried and the unmet
  precondition. It is never reported as verified through a different path.
  `serial:` needs a receiver. On a machine without one, report it unreached.

## Feature entry contract

Each feature file starts with an H1 and one paragraph, then these four H2s,
in order.

1. `Sub-features`
2. `How to get to it (user POV)`
3. `Driving it with the replayed sail`
4. `Gotchas`

## Features

- [Replay a sail](./replay-a-sail.md) plays the Berkeley Marina recording.
- [Live fix](./live-fix.md) is the position a sailor reads, then the stale line.
- [AIS targets](./ais-targets.md) is the traffic list and the danger flag.
- [Source status](./source-status.md) is the source name, status, and counts.
- [Recordings](./recordings.md) is the file `--record` writes, including why the fix changed.
