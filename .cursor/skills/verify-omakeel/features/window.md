# Window

Omahoy's chart and bar live in other repos. This repo's sailor-facing view
is the terminal. A Quickshell window would be `ui/*.qml`. This checkout has
no `ui/` directory, so that window cannot be opened here.

## Sub-features

- `window-absent` records that Quickshell is not installed and `ui/` does not exist.
- `window-watch` is the `omakeel watch` transcript for the same state a window would show.

## How to get to it (user POV)

- On a machine with the window, open the Quickshell app against the hub's socket.
- On this checkout, run `omakeel watch --socket "$run/sock"`.

## Driving it with the replayed sail

Preconditions:

- The commands below were run on 2026-10-13 in this VM.
- `art="$run/artifacts/verify/window"; mkdir -p "$art"`

- **Look for the window.** These commands print the reason the window stays closed.

```sh
art="$run/artifacts/verify/window"
mkdir -p "$art"
{
  echo "DISPLAY=${DISPLAY:-unset}"
  command -v quickshell || echo "quickshell missing"
  command -v Hyprland || echo "Hyprland missing"
  command -v qmllint || echo "qmllint missing"
  if [ -d ui ]; then echo "ui present"; else echo "no ui dir"; fi
} >"$art/window.txt"
cat "$art/window.txt"
```

Observed on 2026-10-13:

```
DISPLAY=:1
quickshell missing
Hyprland missing
qmllint missing
no ui dir
```

`DISPLAY=:1` was a VNC session. No QML file exists to put on it.

- **Read the same state in the terminal.** The lines are the ones in `live-fix.md` and `ais-targets.md`. The first ok line and the danger line from that run:

```
ok      37°51.900′N  122°19.200′W   5.0 kn  255°T  0s ago  | replay:tests/fixtures/berkeley-marina.nmea ok (10 ok, 0 bad)
AIS    3 vessels  · nearest SEA LARK 0.83 nm 301°T, CPA 0.83 nm in 0.0 min  DANGER BAY RUNNER
```

- **Proof.** `window.txt` shows the missing tools and the missing `ui/` directory. The watch lines are the substitute that ran. A Quickshell window on Hyprland remains unchecked until this repo has `ui/*.qml` and those tools.

## Gotchas

- Do not install Quickshell to invent a window this repo does not ship.
- `mise follow` is `omakeel watch` on the default socket. A proof uses `--socket "$run/sock"`.
