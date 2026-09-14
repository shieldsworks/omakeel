#!/usr/bin/env python3
"""Write tests/fixtures/berkeley-marina.nmea, a synthetic minute out of
Berkeley Marina in omakeel's recording format, for the tests and
`mise replay` until real sails are recorded. It is not a real track.

It holds:
- three seconds of a receiver still acquiring (RMC status V, GGA quality 0),
- sixty seconds under way at 5 knots, 255 degrees true, RMC then GGA each second,
- one AIS sentence,
- one RMC with a corrupted digit, so its checksum fails.
"""

import math
from datetime import datetime, timedelta, timezone
from functools import reduce
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "tests/fixtures/berkeley-marina.nmea"
START = datetime(2026, 9, 13, 21, 0, 0, tzinfo=timezone.utc)
LAT, LON = 37.8650, -122.3200  # just outside the Berkeley Marina breakwater
KNOTS, COURSE = 5.0, 255.0
AIS = "!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C"


def sentence(data):
    checksum = reduce(lambda acc, c: acc ^ ord(c), data, 0)
    return f"${data}*{checksum:02X}"


def dm(value, degree_digits, positive, negative):
    hemisphere = positive if value >= 0 else negative
    value = abs(value)
    degrees = int(value)
    minutes = (value - degrees) * 60
    return f"{degrees:0{degree_digits}d}{minutes:07.4f},{hemisphere}"


def main():
    lines = [
        "# omakeel recording v1",
        "# Synthetic: a minute out of Berkeley Marina at 5 kn, made by scripts/sample-sail.py",
    ]
    ms0 = int(START.timestamp() * 1000)
    lat, lon = LAT, LON
    metres_per_second = KNOTS * 1852 / 3600
    for second in range(63):
        t = START + timedelta(seconds=second)
        ms = ms0 + second * 1000
        clock, date = t.strftime("%H%M%S.00"), t.strftime("%d%m%y")
        if second < 3:
            rmc = sentence(f"GPRMC,{clock},V,,,,,,,{date},,,N")
            gga = sentence(f"GPGGA,{clock},,,,,0,00,99.9,,M,,M,,")
        else:
            position = f"{dm(lat, 2, 'N', 'S')},{dm(lon, 3, 'E', 'W')}"
            rmc = sentence(f"GPRMC,{clock},A,{position},{KNOTS:.1f},{COURSE:.1f},{date},,,A")
            gga = sentence(f"GPGGA,{clock},{position},1,09,0.9,2.1,M,-32.2,M,,")
            north = metres_per_second * math.cos(math.radians(COURSE))
            east = metres_per_second * math.sin(math.radians(COURSE))
            lat += north / 111_320
            lon += east / (111_320 * math.cos(math.radians(lat)))
        if second == 30:
            rmc = rmc.replace(",A,3751", ",A,3752", 1)  # corrupted in transit
        lines.append(f"{ms} {rmc}")
        lines.append(f"{ms + 40} {gga}")
        if second == 20:
            lines.append(f"{ms + 500} {AIS}")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines) + "\n")
    print(f"wrote {OUT.relative_to(OUT.parent.parent.parent)}: {len(lines) - 2} lines")


if __name__ == "__main__":
    main()
