#!/usr/bin/env python3
"""
HTTP Range Reading Example

Demonstrates reading MF4 files using a local index with HTTP range requests.
This example mocks an HTTP server to show how the index system enables efficient
remote file access by only fetching the exact bytes needed for specific channels.

IMPORTANT LIMITATION:
    The current Python bindings require a file path for decoding channel values.
    This example demonstrates:
    1. How to calculate exact byte ranges needed using the index
    2. How to fetch those bytes via HTTP range requests (mocked)
    3. Uses file-based decoding as a workaround for demonstration

    In production, you would implement a ByteRangeReader in Rust that handles
    both fetching (via HTTP/S3) and decoding in a single operation.

Key concepts:
- Local index contains all metadata (conversions, data types, offsets)
- Only data bytes need to be fetched via HTTP range requests
- Simulates real-world scenarios like reading from S3, HTTP servers, or cloud storage
- Demonstrates bandwidth savings from partial reads
"""

import mf4_rs
import os
import math
from typing import List, Tuple


class MockHttpRangeReader:
    """
    Simulates an HTTP server that responds to byte range requests.

    In production, this would use requests/urllib with HTTP Range headers:
        headers = {'Range': f'bytes={offset}-{offset + length - 1}'}
        response = requests.get(url, headers=headers)

    This mock implementation:
    - Tracks all range requests for statistics
    - Reads from a local file to simulate remote data
    - Provides analytics on bandwidth usage
    """

    def __init__(self, file_path: str):
        self.file_path = file_path
        self.file_size = os.path.getsize(file_path)
        self.request_count = 0
        self.total_bytes_fetched = 0
        self.requests_log: List[Tuple[int, int]] = []

    def read_range(self, offset: int, length: int) -> bytes:
        """
        Simulate HTTP range request: GET with Range: bytes={offset}-{offset+length-1}

        Args:
            offset: Starting byte position
            length: Number of bytes to read

        Returns:
            Bytes from the specified range

        Raises:
            ValueError: If range is invalid
        """
        if offset < 0 or offset >= self.file_size:
            raise ValueError(f"Invalid offset {offset} for file size {self.file_size}")

        if offset + length > self.file_size:
            raise ValueError(f"Range {offset}-{offset+length} exceeds file size {self.file_size}")

        # Track statistics
        self.request_count += 1
        self.total_bytes_fetched += length
        self.requests_log.append((offset, length))

        # Simulate HTTP range request by reading from file
        with open(self.file_path, 'rb') as f:
            f.seek(offset)
            data = f.read(length)

        return data

    def get_statistics(self) -> dict:
        """Get statistics about range requests made."""
        data_fetched_percent = (self.total_bytes_fetched / self.file_size * 100) if self.file_size > 0 else 0
        bandwidth_saved_percent = 100 - data_fetched_percent
        return {
            'request_count': self.request_count,
            'total_bytes_fetched': self.total_bytes_fetched,
            'file_size': self.file_size,
            'data_fetched_percent': data_fetched_percent,
            'bandwidth_saved_percent': bandwidth_saved_percent,
            'requests': self.requests_log
        }

    def print_statistics(self) -> None:
        """Print human-readable statistics."""
        stats = self.get_statistics()
        print(f"\nHTTP Range Request Statistics:")
        print(f"  Total requests: {stats['request_count']}")
        print(f"  Bytes fetched: {stats['total_bytes_fetched']:,} / {stats['file_size']:,}")
        print(f"  Data fetched: {stats['data_fetched_percent']:.2f}% of file")
        print(f"  Bandwidth saved: {stats['bandwidth_saved_percent']:.2f}%")

        if stats['request_count'] > 0:
            avg_request_size = stats['total_bytes_fetched'] / stats['request_count']
            print(f"  Avg request size: {avg_request_size:.0f} bytes")


def main() -> None:
    print("HTTP Range Reading Example")
    print("=" * 60)

    mdf_file = "http_example.mf4"
    index_file = "http_example.json"

    try:
        # Clean up existing files
        for f in [mdf_file, index_file]:
            if os.path.exists(f):
                os.remove(f)

        # ============================================================
        # Step 1: Create test MDF file
        # ============================================================
        print("\n1. Creating test MDF file...")
        create_test_mdf(mdf_file)
        mdf_size = os.path.getsize(mdf_file)
        print(f"   Created: {mdf_file} ({mdf_size:,} bytes)")

        # ============================================================
        # Step 2: Create and save index locally
        # ============================================================
        print("\n2. Creating index from MDF file...")
        index = mf4_rs.PyMdfIndex.from_file(mdf_file)
        index.save_to_file(index_file)

        index_size = os.path.getsize(index_file)
        print(f"   Index saved: {index_file} ({index_size:,} bytes)")
        print(f"   Index is {index_size/mdf_size*100:.1f}% of MDF file size")

        # ============================================================
        # Step 3: Load index (simulate loading from cache/database)
        # ============================================================
        print("\n3. Loading index from JSON...")
        loaded_index = mf4_rs.PyMdfIndex.load_from_file(index_file)

        # Inspect available channels
        groups = loaded_index.list_channel_groups()
        print(f"   Found {len(groups)} channel group(s)")

        for group_idx, group_name, channel_count in groups:
            group_display = f"'{group_name}'" if group_name else "'<unnamed>'"
            print(f"\n   Group {group_idx}: {group_display} ({channel_count} channels)")
            channels = loaded_index.list_channels(group_idx)
            for ch_idx, ch_name, data_type in channels:
                print(f"     [{ch_idx}] {ch_name} ({data_type.name})")

        # ============================================================
        # Step 4: Initialize mock HTTP reader
        # ============================================================
        print("\n4. Initializing mock HTTP range reader...")
        http_reader = MockHttpRangeReader(mdf_file)
        print(f"   Simulating HTTP server with file: {mdf_file}")
        print(f"   File size: {http_reader.file_size:,} bytes")

        # ============================================================
        # Step 5: Demonstrate HTTP range reading
        # ============================================================
        print("\n5. Reading channels via HTTP range requests...")

        # Example 1: Read entire Temperature channel
        print("\n   Example 1: Read entire 'Temperature' channel")
        temp_location = loaded_index.find_channel_by_name("Temperature")
        if temp_location:
            group_idx, channel_idx = temp_location

            # Get byte ranges from index (no network access needed)
            total_bytes, range_count = loaded_index.get_channel_byte_summary(group_idx, channel_idx)
            ranges = loaded_index.get_channel_byte_ranges(group_idx, channel_idx)

            if not ranges:
                print("   Warning: No byte ranges found for channel")
            else:
                print(f"   Channel needs: {total_bytes} bytes in {range_count} range(s)")

                # Fetch each required range via HTTP
                for i, (offset, length) in enumerate(ranges):
                    print(f"   Range {i+1}: bytes {offset}-{offset+length-1} ({length} bytes)")
                    data = http_reader.read_range(offset, length)
                    print(f"            Fetched {len(data)} bytes via HTTP")

                # Decode using file-based API (current API limitation)
                print(f"   Decoding values (using file-based API as workaround)...")
                temp_values = loaded_index.read_channel_values_by_name("Temperature", mdf_file)
                valid_temps = [v for v in temp_values if v is not None]
                if valid_temps:
                    print(f"   Result: {len(temp_values)} values, range {min(valid_temps):.2f} to {max(valid_temps):.2f}")

        # Example 2: Partial read demonstrates bandwidth savings
        print("\n   Example 2: Partial read - first 10 records of 'RPM' channel")
        rpm_location = loaded_index.find_channel_by_name("RPM")
        if rpm_location:
            group_idx, channel_idx = rpm_location

            # Compare full vs partial
            full_bytes, full_ranges = loaded_index.get_channel_byte_summary(group_idx, channel_idx)
            partial_ranges = loaded_index.get_channel_byte_ranges_for_records(
                group_idx, channel_idx, 0, 10
            )

            if not partial_ranges:
                print("   Warning: No partial ranges found")
            else:
                partial_bytes = sum(length for _, length in partial_ranges)
                savings = (1 - partial_bytes / full_bytes) * 100 if full_bytes > 0 else 0

                print(f"   Full channel: {full_bytes} bytes in {full_ranges} range(s)")
                print(f"   First 10 records: {partial_bytes} bytes in {len(partial_ranges)} range(s)")
                print(f"   Bandwidth savings: {savings:.1f}%")

                # Fetch partial data via HTTP
                for i, (offset, length) in enumerate(partial_ranges):
                    data = http_reader.read_range(offset, length)
                    print(f"   Fetched range {i+1}: {len(data)} bytes")

        # Example 3: Fetch multiple channels efficiently
        print("\n   Example 3: Compare bandwidth for different read patterns")
        if groups:
            group_idx = 0
            channels = loaded_index.list_channels(group_idx)

            print(f"   Analyzing {len(channels)} channels...")

            # Scenario A: Read all channels individually
            total_for_all = 0
            for ch_idx, ch_name, _ in channels:
                bytes_needed, _ = loaded_index.get_channel_byte_summary(group_idx, ch_idx)
                total_for_all += bytes_needed

            print(f"   Scenario A - All channels individually: {total_for_all:,} bytes")

            # Scenario B: Read just one channel
            if channels:
                bytes_one, _ = loaded_index.get_channel_byte_summary(group_idx, 1)
                savings = (1 - bytes_one / total_for_all) * 100 if total_for_all > 0 else 0
                print(f"   Scenario B - Single channel: {bytes_one:,} bytes ({savings:.1f}% saved)")

            # Scenario C: Read first 5 records of one channel
            if channels:
                partial = loaded_index.get_channel_byte_ranges_for_records(group_idx, 1, 0, 5)
                bytes_partial = sum(length for _, length in partial)
                savings = (1 - bytes_partial / total_for_all) * 100 if total_for_all > 0 else 0
                print(f"   Scenario C - 5 records of one channel: {bytes_partial:,} bytes ({savings:.1f}% saved)")

        # ============================================================
        # Step 6: Show statistics
        # ============================================================
        print("\n6. Results:")
        http_reader.print_statistics()

        print("\nKey Takeaways:")
        print("  * Index contains all metadata - no HTTP request for structure")
        print("  * Only data bytes are fetched via HTTP range requests")
        print("  * Partial reads drastically reduce bandwidth usage")
        print("  * Perfect for cloud storage (S3), CDNs, remote servers")

        print("\nReal-world applications:")
        print("  * Read large MDF files from cloud without downloading")
        print("  * Stream specific channels on-demand")
        print("  * Build web-based MDF viewers with minimal bandwidth")
        print("  * Cache indexes locally, fetch data remotely")

        print("\nImplementation notes:")
        print("  * This example mocks HTTP with local file reading")
        print("  * In production, use requests/urllib with Range headers:")
        print("    headers = {'Range': f'bytes={offset}-{offset+length-1}'}")
        print("    response = requests.get(url, headers=headers)")
        print("  * Current Python API requires file path for decoding")
        print("  * Production implementation needs ByteRangeReader in Rust")

    except Exception as e:
        print(f"\nError: {e}")
        import traceback
        traceback.print_exc()

    finally:
        # Cleanup
        for f in [mdf_file, index_file]:
            if os.path.exists(f):
                os.remove(f)


def create_test_mdf(file_path: str) -> None:
    """
    Create a test MDF file with multiple channels and realistic data.

    This simulates a typical automotive measurement scenario:
    - Time channel (master)
    - Temperature sensor
    - RPM sensor
    - Voltage sensor

    Args:
        file_path: Path where the MDF file will be created
    """
    writer = mf4_rs.PyMdfWriter(file_path)
    writer.init_mdf_file()

    # Create channel group for engine data
    group = writer.add_channel_group("Engine Sensors")

    # Add channels
    time_ch = writer.add_time_channel(group, "Time")
    temp_ch = writer.add_float_channel(group, "Temperature")
    rpm_ch = writer.add_int_channel(group, "RPM")
    voltage_ch = writer.add_float_channel(group, "Voltage")

    # Write 100 records of measurement data
    writer.start_data_block(group)

    for i in range(100):
        time_val = mf4_rs.create_float_value(i * 0.01)  # 10ms intervals

        # Simulate realistic sensor values using absolute values to avoid negatives
        temp_val = mf4_rs.create_float_value(20 + 50 * abs(math.sin(i * 0.1)))  # 20-70°C
        rpm_val = mf4_rs.create_uint_value(1500 + int(1500 * abs(math.sin(i * 0.05))))  # 1500-3000 RPM
        voltage_val = mf4_rs.create_float_value(12.0 + 0.5 * math.sin(i * 0.2))  # 11.5-12.5V

        writer.write_record(group, [time_val, temp_val, rpm_val, voltage_val])

    writer.finish_data_block(group)
    writer.finalize()


if __name__ == "__main__":
    main()
