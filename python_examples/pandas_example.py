#!/usr/bin/env python3
"""pandas integration: ``Signal.to_series()``.

``read()`` returns a ``Signal`` (numpy ``values`` + ``timestamps``);
``to_series()`` turns it into a ``pandas.Series`` indexed by absolute time.
Run ``write_file.py`` first to create ``example.mf4``.
"""

import pandas as pd

import mf4_rs


def main():
    mdf = mf4_rs.Mdf.from_file("example.mf4")

    # A Series with a DatetimeIndex (file start time + relative timestamps).
    speed = mdf.read("Speed").to_series()
    print(speed.head())
    print("index dtype:", speed.index.dtype)

    # Everything pandas offers from here on.
    print("\n1-second means:")
    print(speed.resample("1s").mean().head())

    # A whole group as a DataFrame.
    engine = mdf.group("Engine")
    frame = pd.DataFrame({
        name: mdf.read(name, group=engine.name).to_series()
        for name in engine.channel_names
        if name != engine.master_channel
    })
    print("\nEngine group:")
    print(frame.head())

    # The same through an index (lazy / remote reads).
    index = mf4_rs.MdfIndex.from_file("example.mf4")
    window = index.read("Speed", start_time=2.0, end_time=2.05).to_series()
    print("\nSpeed between 2.00s and 2.05s:")
    print(window)


if __name__ == "__main__":
    main()
