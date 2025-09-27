from asammdf import MDF, Signal
import numpy as np
import pandas as pd
import sys
import const_sigs
import time
from functools import wraps

def timed(func):
    """Decorator to measure execution time of functions."""
    @wraps(func)
    def wrapper(*args, **kwargs):
        start = time.perf_counter()
        result = func(*args, **kwargs)
        elapsed = time.perf_counter() - start
        print(f"[TIMER] {func.__name__} took {elapsed:.3f} seconds")
        return result
    return wrapper

@timed
def dump_sig_list(fname):
    with MDF(fname) as mdf_file:
        for group in mdf_file.groups:
            for channel in group.channels:
                if channel.name != "t":
                    print(channel.name)

@timed
def verify_signals(fname):
    """Read each non-time signal and print its sample count."""
    with MDF(fname) as mdf_file:
        for group in mdf_file.groups:
            for channel in group.channels:
                if channel.name != "t":
                    sig = mdf_file.get(channel.name)
                    print(f"{channel.name}: {len(sig.samples)} samples")

@timed
def read_test_signal(fname, signame):
    with MDF(fname, channels=[signame]) as mdf_file:
        sig = mdf_file.get(signame)
        print("Done!")

@timed
def write_test():
    print("Writing test mdf4...")
    data_double = Signal(
        samples=np.arange(10_000_000, dtype=np.single),
        timestamps=100_000_000 + np.arange(10_000_000, dtype=np.single) * 1_000,
        name="FloatLE"
    )
    with MDF(version='4.20') as mdf_file:
        mdf_file.append([data_double], comment="Example")
        mdf_file.save('asammdf_test.mf4')
    print("Done!")

@timed
def write_test_signals():
    with MDF(version='4.20') as mdf_file:
        signals = []
        timestamps = 100_000_000 + np.arange(10_000_000, dtype=np.float64) * 1_000
        for name, bit_count, typ, float_val, int_val in const_sigs.SIG_LIST:
            if typ is int:
                byte_width = ((bit_count - 1) // 8 + 1) * 8
                dtype = np.dtype(f"uint{byte_width}")
                samples = np.full(10_000_000, int_val, dtype=dtype)
            else:
                if bit_count == 16:
                    dtype = np.float16
                else:
                    dtype = np.float32
                samples = np.full(10_000_000, float_val, dtype=dtype)
            sig = Signal(samples=samples, timestamps=timestamps,
                         name=name, bit_count=bit_count)
            signals.append(sig)

        # Provide the common_timebase to place all channels in one group
        mdf_file.append(signals, comment="Example", common_timebase=True)
        mdf_file.save('asammdf_write_test_signals.tmp.mf4')
    print("Done!")

@timed
def write_test_bytes():
    with MDF(version='4.20') as mdf_file:
        sig_bytes = np.array(const_sigs.RAW_MSG, dtype=np.ubyte)
        samples = [sig_bytes] * 10_000_000
        timestamps = 100_000_000 + np.arange(10_000_000, dtype=np.single) * 1_000
        sig = Signal(samples=samples, timestamps=timestamps,
                     name="CAN_DataBytes", bit_count=512)
        mdf_file.append(sig)
        mdf_file.save('asammdf_write_test_frame.tmp.mf4')
    print("Done!")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Please supply the required arguments!")
        exit(-1)

    cmd = sys.argv[1]
    if cmd == "asammdf_read":
        if len(sys.argv) < 4:
            print("Please supply a filename and signal name!")
            exit(-1)
        read_test_signal(sys.argv[2], sys.argv[3])
    elif cmd == "asammdf_write":
        write_test()
    elif cmd == "asammdf_write_signals":
        write_test_signals()
    elif cmd == "asammdf_write_frame":
        write_test_bytes()
    elif cmd == "asammdf_dump_signals":
        dump_sig_list(sys.argv[2])
    elif cmd == "asammdf_verify_signals":
        verify_signals(sys.argv[2])
    else:
        print(f"Unknown command: {cmd}")
