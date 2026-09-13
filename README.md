# Omakeel

The data hub for [Omahoy](https://github.com/shieldsworks/omahoy). Everything is built
on the keel.

Omakeel is a headless Rust daemon. It reads the boat's instruments and serves
one live stream to every Omahoy app.

**Status: planned.** Nothing to install yet. Omakeel is being built first.

## What it will do

- Read position from a USB GPS (NMEA 0183) and ship reports from a dAISy AIS
  receiver.
- Serve every Omahoy app over a Unix socket, and serve the same protocol across
  the boat's network so a second screen can join.
- Record every sail and replay it, so the apps can be developed at the dock.
- Run the watches that can't wait for a screen (anchor, AIS, log), and keep
  running when the laptop is closed.
- Send NMEA 0183 over TCP on port 10110, so a phone app can read the boat too.

## License

MIT
