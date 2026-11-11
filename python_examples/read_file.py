#!/usr/bin/env python3
"""Read an MDF 4 file with the name-based reader API.

Run ``write_file.py`` first to create ``example.mf4``. The same calls exist
in Rust and TypeScript — see docs/API.md.
"""

import mf4_rs


def main():
    mdf = mf4_rs.Mdf.from_file("example.mf4")
    print("start time:", mdf.start_time_ns, "ns")

    # Metadata: groups and their channels.
    for group in mdf.groups:
        print(f"\ngroup {group.name}: {group.record_count} records, master {group.master_channel}")
        for ch in group.channels:
            print(f"  {ch.name:<12} {ch.data_type:<18} {ch.bit_count:>2} bits  "
                  f"unit={ch.unit} conversion={ch.has_conversion}")

    # read() -> Signal: values (conversions applied) + the group's time axis.
    temperature = mdf.read("Temperature")
    print(f"\n{temperature.name}: {len(temperature)} samples")
    for t, v in list(zip(temperature.timestamps, temperature.values))[:3]:
        print(f"  t={t:.1f}s  {v}")

    # values() -> numpy float64 array: just the numbers.
    speed = mdf.values("Speed")
    print(f"Speed: {len(speed)} samples, mean {speed.mean():.2f}")

    # Text conversions come back as strings through read().
    print("Gear:", list(mdf.read("Gear").values[:3]))

    # Narrow by group, and read only a time or record window.
    print("first 5 Vehicle times:", mdf.values("Time", group="Vehicle", start_record=0, record_count=5))
    print("Speed between 1.00s and 1.05s:", mdf.values("Speed", start_time=1.0, end_time=1.05))
    print("shorthand:", mdf["Time", "Vehicle"])     # mdf[name] / mdf[name, group] is read()

    # Inspection.
    print("Speed time range:", mdf.time_range("Speed"))
    print("Temperature conversion:", mdf.conversion("Temperature"))

    # Unknown names: metadata lookups return None, data reads raise.
    assert mdf.channel("Nope") is None
    try:
        mdf.read("Nope")
    except mf4_rs.ChannelNotFoundError as exc:
        print("as expected:", exc)


if __name__ == "__main__":
    main()
