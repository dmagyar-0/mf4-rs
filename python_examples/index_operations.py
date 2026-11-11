#!/usr/bin/env python3
"""Lazy reads through an ``MdfIndex``.

An index holds only metadata. It remembers a data source (file path or URL)
and fetches just the bytes each read needs; it has the same reading API as
``Mdf``. Run ``write_file.py`` first to create ``example.mf4``.
"""

import os

import numpy as np

import mf4_rs


def main():
    # Build an index (reads metadata only) and save it as JSON.
    index = mf4_rs.MdfIndex.from_file("example.mf4")
    index.save("example_index.json")
    print(f"indexed {index.file_size} bytes; source = {index.source}")

    # Later: load the index. The source is not serialised — re-attach it.
    # An http(s):// URL works the same way, using range requests:
    #   index = mf4_rs.MdfIndex.from_url("https://host/recording.mf4")
    index = mf4_rs.MdfIndex.load("example_index.json")
    index.set_source("example.mf4")

    # The same reading API as Mdf.
    for group in index.groups:
        print(f"{group.name}: {group.record_count} records, channels {group.channel_names}")
    temperature = index.read("Temperature")
    print(f"Temperature: {len(temperature)} samples, first {temperature.values[0]}")
    print("between 1.0s and 1.5s:", index.values("Temperature", start_time=1.0, end_time=1.5))
    print("time range:", index.time_range("Temperature"))
    print("conversion:", index.conversion("Temperature"))

    # Both readers agree.
    mdf = mf4_rs.Mdf.from_file("example.mf4")
    np.testing.assert_array_equal(mdf.values("Temperature"), index.values("Temperature"))

    # Power users: the byte ranges a channel occupies, for your own range requests.
    print("byte ranges of Temperature:", index.byte_ranges("Temperature"))
    print("…for records 10..20:", index.byte_ranges_for_records("Temperature", 10, 10))

    os.remove("example_index.json")


if __name__ == "__main__":
    main()
