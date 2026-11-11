#!/usr/bin/env python3
"""Write an MDF 4 file with the name-based writer API.

Creates ``example.mf4`` for the other examples. The same calls exist in Rust
and TypeScript — see docs/API.md.
"""

import numpy as np

import mf4_rs


def main():
    writer = mf4_rs.MdfWriter("example.mf4")
    writer.init_mdf_file()

    # One channel group with a time (master) channel and three data channels.
    group = writer.add_channel_group("Engine")
    writer.set_channel_group_comment(group, "Demo measurement")
    writer.add_time_channel(group, "Time")          # add the master first
    temperature = writer.add_float_channel(group, "Temperature")
    gear = writer.add_int_channel(group, "Gear")
    writer.add_string_channel(group, "State")

    # Conversions turn raw values into physical ones when reading.
    writer.add_linear_conversion(temperature, -40.0, 0.5)        # raw -> °C
    writer.add_value_to_text_conversion(gear, [(0, "N"), (1, "D"), (2, "R")], "?")

    writer.start_data_block(group)
    for i in range(100):
        # Plain values, one per channel, in the order the channels were added.
        writer.write_record(group, [i * 0.1, 120.0 + i, i % 3, "idle" if i % 2 == 0 else "running"])
    writer.finish_data_block(group)

    # A second, purely numeric group written column by column (the fast path).
    group = writer.add_channel_group("Vehicle")
    writer.add_time_channel(group, "Time")
    writer.add_float_channel(group, "Speed")
    time = np.arange(1000) * 0.01
    speed = 50.0 + 10.0 * np.sin(time)
    writer.start_data_block(group)
    writer.write_columns(group, [time, speed])
    writer.finish_data_block(group)

    writer.finalize()

    mdf = mf4_rs.Mdf.from_file("example.mf4")
    for group in mdf.groups:
        print(f"{group.name}: {group.record_count} records, channels {group.channel_names}")
    print("first temperatures:", mdf.values("Temperature")[:3])
    print("Created example.mf4")


if __name__ == "__main__":
    main()
