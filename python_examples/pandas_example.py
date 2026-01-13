#!/usr/bin/env python3
"""
Pandas Integration Example

Demonstrates the pandas Series support in mf4-rs Python bindings.
Shows how to get channels as pandas Series with automatic time indexing.
"""

import mf4_rs
import os

def main():
    print("🐼 Pandas Integration Example")
    print("=" * 50)

    # Check if pandas is available
    try:
        import pandas as pd
        print("✓ pandas is installed")
    except ImportError:
        print("❌ pandas is not installed")
        print("   Install with: pip install pandas")
        return

    # Create a test file
    mdf_file = "pandas_example.mf4"

    try:
        # Clean up existing file
        if os.path.exists(mdf_file):
            os.remove(mdf_file)

        print("\n1️⃣ Creating test MDF file...")
        create_test_mdf(mdf_file)

        print("\n2️⃣ Reading channels as pandas Series...")
        mdf = mf4_rs.PyMDF(mdf_file)

        # Get channel as pandas Series with automatic time indexing
        # NEW: Time index is now a pandas DatetimeIndex with absolute timestamps!
        temp_series = mdf.get_channel_as_series("Temperature")
        rpm_series = mdf.get_channel_as_series("RPM")

        print(f"   Temperature Series: {len(temp_series)} samples")
        print(f"   Index type: {type(temp_series.index).__name__}")

        if isinstance(temp_series.index, pd.DatetimeIndex):
            print(f"   Datetime range: {temp_series.index[0]} to {temp_series.index[-1]}")
            print(f"   Index dtype: {temp_series.index.dtype}")
        else:
            print(f"   Index (numeric): {temp_series.index[0]:.2f} to {temp_series.index[-1]:.2f}")

        print(f"   Values: {temp_series.values[0]:.2f} to {temp_series.values[-1]:.2f}")

        print("\n3️⃣ Using pandas functionality...")

        # Statistical analysis
        print(f"   Temperature statistics:")
        print(f"     Mean: {temp_series.mean():.2f}°C")
        print(f"     Std:  {temp_series.std():.2f}°C")
        print(f"     Min:  {temp_series.min():.2f}°C")
        print(f"     Max:  {temp_series.max():.2f}°C")

        # Time-based indexing
        print(f"\n   Time-based lookup:")
        try:
            # Find value closest to time 1.0
            time_idx = 1.0
            closest_idx = (temp_series.index - time_idx).abs().argmin()
            print(f"     Value at t≈{time_idx}: {temp_series.iloc[closest_idx]:.2f}°C")
        except:
            print(f"     Using positional indexing")
            print(f"     Value at index 10: {temp_series.iloc[10]:.2f}°C")

        # Combine into DataFrame
        print("\n4️⃣ Creating DataFrame from multiple channels...")
        df = pd.DataFrame({
            'Temperature': temp_series,
            'RPM': rpm_series
        })

        print(f"   DataFrame shape: {df.shape}")
        print(f"\n   First 5 rows:")
        print(df.head())

        print(f"\n   Correlation matrix:")
        print(df.corr())

        # Advanced operations
        print("\n5️⃣ Advanced pandas operations...")

        # Rolling average
        rolling_temp = temp_series.rolling(window=5).mean()
        print(f"   Rolling average (window=5): {rolling_temp.iloc[-1]:.2f}°C")

        # Datetime-specific operations (if using DatetimeIndex)
        if isinstance(temp_series.index, pd.DatetimeIndex):
            print(f"\n   Datetime-specific features:")
            print(f"     Time span: {temp_series.index[-1] - temp_series.index[0]}")
            print(f"     Frequency: {pd.infer_freq(temp_series.index) or 'irregular'}")

            # Resampling to different time intervals
            try:
                # Resample to 1-second intervals (mean aggregation)
                resampled = temp_series.resample('1S').mean()
                print(f"     Resampled to 1s: {len(resampled)} samples (from {len(temp_series)})")
            except:
                print(f"     Resampling: not applicable for this dataset")

            # Time-based slicing
            try:
                first_second = temp_series.index[0]
                after_2s = first_second + pd.Timedelta(seconds=2)
                subset = temp_series.loc[first_second:after_2s]
                print(f"     First 2 seconds: {len(subset)} samples")
            except:
                pass
        else:
            print(f"   Data frequency: ~{(temp_series.index[1] - temp_series.index[0]):.3f}s between samples")

        # Filtering
        hot_samples = temp_series[temp_series > temp_series.mean()]
        print(f"   Samples above mean: {len(hot_samples)} / {len(temp_series)}")

        print("\n6️⃣ Using with Index system...")
        index = mf4_rs.PyMdfIndex.from_file(mdf_file)

        # Read as Series using index (faster for repeated access)
        temp_series_indexed = index.read_channel_as_series("Temperature", mdf_file)
        print(f"   Temperature via index: {len(temp_series_indexed)} samples")
        print(f"   Mean: {temp_series_indexed.mean():.2f}°C")

        print("\n✅ Pandas integration demonstrated successfully!")
        print("\n🎯 Key Benefits:")
        print("   • Native pandas Series objects")
        print("   • Automatic time indexing from master channel")
        print("   • Full pandas functionality (stats, plotting, etc.)")
        print("   • Easy DataFrame creation from multiple channels")
        print("   • Works with both PyMDF and PyMdfIndex")

    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()

    finally:
        # Cleanup
        if os.path.exists(mdf_file):
            os.remove(mdf_file)

def create_test_mdf(file_path):
    """Create a simple MDF file for testing."""
    writer = mf4_rs.PyMdfWriter(file_path)
    writer.init_mdf_file()

    # Create channel group
    group = writer.add_channel_group("Test Data")

    # Add channels - time channel must be first
    time_ch = writer.add_time_channel(group, "Time")
    temp_ch = writer.add_float_channel(group, "Temperature")
    rpm_ch = writer.add_int_channel(group, "RPM")

    # Write data
    writer.start_data_block(group)

    # Generate 50 samples
    for i in range(50):
        time_val = mf4_rs.create_float_value(i * 0.1)  # Time: 0.0 to 4.9 seconds
        temp_val = mf4_rs.create_float_value(20 + i * 0.5)  # Temperature: 20-44.5°C
        rpm_val = mf4_rs.create_uint_value(1000 + i * 20)  # RPM: 1000-1980

        writer.write_record(group, [time_val, temp_val, rpm_val])

    writer.finish_data_block(group)
    writer.finalize()

if __name__ == "__main__":
    main()
