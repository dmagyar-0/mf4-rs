#!/usr/bin/env python3
"""
Example: Visualize the on-disk block structure of an MDF file.

Shows how to:
  1. Build a FileLayout either through PyMDF.file_layout() or directly
     from a path via mf4_rs.file_layout_from_file().
  2. Render it as a flat byte-sorted listing or an indented link tree.
  3. Dump it to JSON for further analysis.
  4. Walk the in-memory block list programmatically.
"""

import mf4_rs


def main():
    path = "example.mf4"  # assumes write_file.py has been run

    # Option 1: through an MDF instance you already hold.
    mdf = mf4_rs.PyMDF(path)
    layout = mdf.file_layout()

    # Option 2: build directly from a path.
    layout_direct = mf4_rs.file_layout_from_file(path)
    assert layout.file_size == layout_direct.file_size

    # ---- Flat listing ----
    print("===== Flat listing =====")
    print(layout.to_text())

    # ---- Tree view ----
    print("===== Link tree =====")
    print(layout.to_tree())

    # ---- Programmatic access ----
    print(
        f"File size: {layout.file_size} bytes, "
        f"{len(layout.blocks)} blocks, "
        f"{len(layout.gaps)} gaps"
    )
    print("\nFirst 10 blocks:")
    for block in layout.blocks[:10]:
        print(
            f"  {block.block_type:<6} "
            f"0x{block.offset:010x}..0x{block.end_offset:010x} "
            f"({block.size:>6} B)  {block.description}"
        )
        for link in block.links:
            target = "null" if link.target == 0 else f"0x{link.target:010x}"
            ttype = link.target_type or ""
            print(f"      {link.name:<28} -> {target} {ttype}")

    # ---- Persist to disk ----
    layout.write_text_to_file("example.layout.txt")
    layout.write_tree_to_file("example.layout.tree.txt")
    layout.write_json_to_file("example.layout.json")
    print(
        "\nWrote example.layout.txt, example.layout.tree.txt, example.layout.json"
    )


if __name__ == "__main__":
    main()
