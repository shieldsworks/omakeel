#!/usr/bin/env python3
"""Write tests/fixtures/berkeley-marina.nmea, a synthetic minute out of
Berkeley Marina in omakeel's recording format, for the tests and
`mise replay` until real sails are recorded. It is not a real track, and
its vessels are invented, with fictional names and MMSIs.

It holds:
- three seconds of a receiver still acquiring (RMC status V, GGA quality 0),
- sixty seconds under way at 5 knots, 255 degrees true, RMC then GGA each second,
- one RMC with a corrupted digit, so its checksum fails,
- AIS from three vessels, encoded here the way their transponders would:
  - BAY RUNNER, a class A ferry at 20 knots, set to pass 0.2 nm from where
    Dash will be five minutes out: a collision alarm,
  - PACIFIC TRADER, a class A cargo ship three miles south, heading away,
  - SEA LARK, a class B sailboat, whose name comes in a type 24 message.
"""

import math
from datetime import datetime, timedelta, timezone
from functools import reduce
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "tests/fixtures/berkeley-marina.nmea"
START = datetime(2026, 9, 13, 21, 0, 0, tzinfo=timezone.utc)
LAT, LON = 37.8650, -122.3200  # just outside the Berkeley Marina breakwater
KNOTS, COURSE = 5.0, 255.0
UNDER_WAY = 3  # seconds of acquiring before the first fix


def checksum(data):
    return reduce(lambda acc, c: acc ^ ord(c), data, 0)


def sentence(data, start="$"):
    return f"{start}{data}*{checksum(data):02X}"


def dm(value, degree_digits, positive, negative):
    hemisphere = positive if value >= 0 else negative
    value = abs(value)
    degrees = int(value)
    minutes = (value - degrees) * 60
    return f"{degrees:0{degree_digits}d}{minutes:07.4f},{hemisphere}"


def moved(lat, lon, course, knots, seconds):
    """Where a vessel is after `seconds` on a course, flat-earth over a mile or two."""
    nm = knots * seconds / 3600
    lat2 = lat + nm * math.cos(math.radians(course)) / 60
    lon2 = lon + nm * math.sin(math.radians(course)) / (60 * math.cos(math.radians(lat)))
    return lat2, lon2


def dash_at(second):
    return moved(LAT, LON, COURSE, KNOTS, max(0, second - UNDER_WAY))


# AIS encoding (ITU-R M.1371): fields packed into bits, six bits to a character.

def uint(value, n):
    return format(value & ((1 << n) - 1), f"0{n}b")


def text(s, chars):
    s = s.upper().ljust(chars, "@")[:chars]
    return "".join(uint(ord(c) - 64 if ord(c) >= 64 else ord(c), 6) for c in s)


def armor(bits):
    fill = (-len(bits)) % 6
    bits += "0" * fill
    out = []
    for i in range(0, len(bits), 6):
        v = int(bits[i:i + 6], 2)
        out.append(chr(v + 48 if v < 40 else v + 56))
    return "".join(out), fill


def vdm(bits, channel, seq=None, split=None):
    """One or more `!AIVDM` sentences carrying `bits`."""
    payload, fill = armor(bits)
    parts = [payload] if split is None else [payload[:split], payload[split:]]
    lines = []
    for n, part in enumerate(parts, 1):
        last = n == len(parts)
        seq_id = "" if len(parts) == 1 else str(seq)
        data = f"AIVDM,{len(parts)},{n},{seq_id},{channel},{part},{fill if last else 0}"
        lines.append(sentence(data, "!"))
    return lines


def dims(bow, stern, port, starboard):
    return uint(bow, 9) + uint(stern, 9) + uint(port, 6) + uint(starboard, 6)


def class_a(mmsi, lat, lon, sog, cog, second, status=0):
    return (uint(1, 6) + uint(0, 2) + uint(mmsi, 30) + uint(status, 4) + uint(-128, 8)
            + uint(round(sog * 10), 10) + uint(1, 1)
            + uint(round(lon * 600000), 28) + uint(round(lat * 600000), 27)
            + uint(round(cog * 10), 12) + uint(round(cog) % 360, 9) + uint(second % 60, 6)
            + uint(0, 2) + uint(0, 3) + uint(0, 1) + uint(0, 19))


def class_b(mmsi, lat, lon, sog, cog, second):
    return (uint(18, 6) + uint(0, 2) + uint(mmsi, 30) + uint(0, 8)
            + uint(round(sog * 10), 10) + uint(1, 1)
            + uint(round(lon * 600000), 28) + uint(round(lat * 600000), 27)
            + uint(round(cog * 10), 12) + uint(511, 9) + uint(second % 60, 6)
            + uint(0, 2) + "1" + "0" + "0" + "1" + "0" + "0" + "0" + uint(0, 20))


def static(mmsi, callsign, name, ship_type, size, destination, draught):
    return (uint(5, 6) + uint(0, 2) + uint(mmsi, 30) + uint(0, 2) + uint(0, 30)
            + text(callsign, 7) + text(name, 20) + uint(ship_type, 8) + dims(*size)
            + uint(1, 4) + uint(0, 4) + uint(0, 5) + uint(24, 5) + uint(60, 6)
            + uint(round(draught * 10), 8) + text(destination, 20) + "0" + "0")


def class_b_name(mmsi, name):
    return uint(24, 6) + uint(0, 2) + uint(mmsi, 30) + uint(0, 2) + text(name, 20)


def class_b_particulars(mmsi, ship_type, callsign, size):
    return (uint(24, 6) + uint(0, 2) + uint(mmsi, 30) + uint(1, 2) + uint(ship_type, 8)
            + text("", 7) + text(callsign, 7) + dims(*size) + uint(0, 6))


FERRY, CARGO, SAILBOAT = 366999101, 366999102, 338999103


def ferry_at(second):
    """BAY RUNNER passes 0.2 nm from where Dash will be at 303 s."""
    meet = dash_at(303)
    # 0.2 nm to the side of the ferry's track, then back along it.
    side = moved(*meet, 150 + 90, 1, 0.2 * 3600)
    return moved(*side, 150, 20.0, second - 303)


def ais(second):
    """The AIS lines heard in `second`, as (millisecond offset, sentence)."""
    lines = []
    if second % 5 == 1:
        lat, lon = ferry_at(second)
        lines += [(300, s) for s in vdm(class_a(FERRY, lat, lon, 20.0, 150.0, second), "A")]
    if second == 4:
        bits = static(FERRY, "WDX9101", "BAY RUNNER", 60, (30, 10, 5, 5), "SF FERRY BLDG", 2.5)
        lines += [(600, s) for s in vdm(bits, "A", seq=1, split=60)]
    if second % 10 == 2:
        lat, lon = moved(37.8150, -122.3300, 190, 11.0, second)
        lines += [(300, s) for s in vdm(class_a(CARGO, lat, lon, 11.0, 190.0, second), "B")]
    if second == 9:
        bits = static(CARGO, "WDX9102", "PACIFIC TRADER", 70, (150, 30, 14, 14), "OAKLAND", 11.0)
        lines += [(600, s) for s in vdm(bits, "B", seq=2, split=60)]
    if second % 15 == 0:
        lat, lon = moved(37.8720, -122.3350, 300, 4.0, second)
        lines += [(700, s) for s in vdm(class_b(SAILBOAT, lat, lon, 4.0, 300.0, second), "A")]
    if second == 7:
        lines += [(700, s) for s in vdm(class_b_name(SAILBOAT, "SEA LARK"), "B")]
    if second == 8:
        lines += [(700, s) for s in vdm(class_b_particulars(SAILBOAT, 36, "WDY9103", (8, 3, 2, 2)), "B")]
    return lines


def main():
    records = []
    ms0 = int(START.timestamp() * 1000)
    for second in range(63):
        t = START + timedelta(seconds=second)
        ms = ms0 + second * 1000
        clock, date = t.strftime("%H%M%S.00"), t.strftime("%d%m%y")
        if second < UNDER_WAY:
            rmc = sentence(f"GPRMC,{clock},V,,,,,,,{date},,,N")
            gga = sentence(f"GPGGA,{clock},,,,,0,00,99.9,,M,,M,,")
        else:
            lat, lon = dash_at(second)
            position = f"{dm(lat, 2, 'N', 'S')},{dm(lon, 3, 'E', 'W')}"
            rmc = sentence(f"GPRMC,{clock},A,{position},{KNOTS:.1f},{COURSE:.1f},{date},,,A")
            gga = sentence(f"GPGGA,{clock},{position},1,09,0.9,2.1,M,-32.2,M,,")
        if second == 30:
            rmc = rmc.replace(",A,3751", ",A,3752", 1)  # corrupted in transit
        records.append((ms, rmc))
        records.append((ms + 40, gga))
        records += [(ms + offset + i * 20, line) for i, (offset, line) in enumerate(ais(second))]
    records.sort(key=lambda r: r[0])
    lines = [
        "# omakeel recording v1",
        "# Synthetic: a minute out of Berkeley Marina at 5 kn with three invented vessels, made by scripts/sample-sail.py",
    ] + [f"{ms} {line}" for ms, line in records]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines) + "\n")
    print(f"wrote {OUT.relative_to(OUT.parent.parent.parent)}: {len(records)} lines")


if __name__ == "__main__":
    main()
