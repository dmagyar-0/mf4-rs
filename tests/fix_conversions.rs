//! Value-level tests for the MDF 4.1 conversion subsystem fixes
//! (B1 rational, B2 value/range-to-text defaults, B3 single-pair tables,
//! B4 bitfield text, B5 algebraic X1 alias, B6 non-finite cc_val JSON,
//! B8 text-to-value determinism).

use std::collections::HashMap;

use mf4_rs::blocks::common::BlockHeader;
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::parsing::decoder::DecodedValue;

/// Build a minimal in-memory conversion block.
fn cc(cc_type: ConversionType, cc_val: Vec<f64>, cc_ref: Vec<u64>) -> ConversionBlock {
    ConversionBlock {
        header: BlockHeader {
            id: "##CC".to_string(),
            reserved0: 0,
            block_len: 0,
            links_nr: 4 + cc_ref.len() as u64,
        },
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref_count: cc_ref.len() as u16,
        cc_ref,
        cc_type,
        cc_precision: 0,
        cc_flags: 0,
        cc_val_count: cc_val.len() as u16,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val,
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
        default_conversion: None,
    }
}

fn as_f64(v: &DecodedValue) -> f64 {
    match v {
        DecodedValue::Float(f) => *f,
        other => panic!("expected Float, got {:?}", other),
    }
}

fn as_str(v: &DecodedValue) -> &str {
    match v {
        DecodedValue::String(s) => s.as_str(),
        other => panic!("expected String, got {:?}", other),
    }
}

/// Append a ##TX block containing `text` at `addr` in `file_data`.
fn put_tx(file_data: &mut Vec<u8>, addr: usize, text: &[u8]) {
    assert!(file_data.len() <= addr, "blocks must be laid out in order");
    file_data.resize(addr, 0);
    file_data.extend_from_slice(b"##TX");
    file_data.extend_from_slice(&[0u8; 4]);
    file_data.extend_from_slice(&((24 + text.len()) as u64).to_le_bytes());
    file_data.extend_from_slice(&0u64.to_le_bytes());
    file_data.extend_from_slice(text);
}

/// Append a serialized ##CC block at `addr` in `file_data`.
fn put_cc(file_data: &mut Vec<u8>, addr: usize, block: &ConversionBlock) {
    assert!(file_data.len() <= addr, "blocks must be laid out in order");
    file_data.resize(addr, 0);
    file_data.extend_from_slice(&block.to_bytes().expect("serialize ##CC"));
}

// ---------------------------------------------------------------- linear

#[test]
fn linear_math() {
    let block = cc(ConversionType::Linear, vec![2.0, 3.0], vec![]);
    let out = block.apply_decoded(DecodedValue::Float(4.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 14.0);

    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(10), &[])
        .unwrap();
    assert_eq!(as_f64(&out), 32.0);
}

// -------------------------------------------------------------- rational (B1)

#[test]
fn rational_tiny_denominator_is_not_falsified() {
    // y = 1 / x  =>  num = 1, den = x
    let block = cc(
        ConversionType::Rational,
        vec![0.0, 0.0, 1.0, 0.0, 1.0, 0.0],
        vec![],
    );
    // 1e-20 is below f64::EPSILON; the old guard returned the raw value.
    let out = block.apply_decoded(DecodedValue::Float(1e-20), &[]).unwrap();
    assert_eq!(as_f64(&out), 1e20);
}

#[test]
fn rational_true_zero_denominator_gives_ieee_inf() {
    let block = cc(
        ConversionType::Rational,
        vec![0.0, 0.0, 1.0, 0.0, 1.0, 0.0],
        vec![],
    );
    let out = block.apply_decoded(DecodedValue::Float(0.0), &[]).unwrap();
    assert!(as_f64(&out).is_infinite());
}

// -------------------------------------------------------------- tables (B3)

#[test]
fn single_pair_table_clamps_to_its_value() {
    for interp_type in [
        ConversionType::TableLookupInterp,
        ConversionType::TableLookupNoInterp,
    ] {
        let block = cc(interp_type, vec![10.0, 42.0], vec![]);
        for raw in [-1e9, 0.0, 10.0, 1e9] {
            let out = block.apply_decoded(DecodedValue::Float(raw), &[]).unwrap();
            assert_eq!(as_f64(&out), 42.0, "type {:?}, raw {}", interp_type, raw);
        }
    }
}

#[test]
fn interp_table_clamps_at_both_ends_and_interpolates() {
    let block = cc(
        ConversionType::TableLookupInterp,
        vec![0.0, 0.0, 10.0, 100.0],
        vec![],
    );
    // Clamp below the first key
    let out = block.apply_decoded(DecodedValue::Float(-5.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 0.0);
    // Clamp above the last key
    let out = block.apply_decoded(DecodedValue::Float(15.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 100.0);
    // Linear interpolation in the middle
    let out = block.apply_decoded(DecodedValue::Float(5.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 50.0);
}

#[test]
fn no_interp_table_tie_picks_lower_key() {
    let block = cc(
        ConversionType::TableLookupNoInterp,
        vec![0.0, 10.0, 10.0, 20.0],
        vec![],
    );
    // Exactly between the keys: the lower key's value wins.
    let out = block.apply_decoded(DecodedValue::Float(5.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 10.0);
    // Nearer the upper key
    let out = block.apply_decoded(DecodedValue::Float(7.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 20.0);
}

#[test]
fn range_lookup_int_inclusive_vs_float_exclusive_upper() {
    // Ranges: [0,10] -> 1, [10,20] -> 2, default 99
    let block = cc(
        ConversionType::RangeLookup,
        vec![0.0, 10.0, 1.0, 10.0, 20.0, 2.0, 99.0],
        vec![],
    );
    // Integer raw values use an inclusive upper bound: 10 matches range 0.
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(10), &[])
        .unwrap();
    assert_eq!(as_f64(&out), 1.0);
    // Float raw values use an exclusive upper bound: 10.0 matches range 1.
    let out = block.apply_decoded(DecodedValue::Float(10.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 2.0);
    // No range matched: default value.
    let out = block.apply_decoded(DecodedValue::Float(50.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 99.0);
}

// -------------------------------------------------------- value-to-text (B2)

#[test]
fn value_to_text_matched_returns_text() {
    let mut block = cc(ConversionType::ValueToText, vec![1.0, 2.0], vec![1, 1]);
    block.resolved_texts = Some(HashMap::from([
        (0usize, "One".to_string()),
        (1usize, "Two".to_string()),
    ]));
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(2), &[])
        .unwrap();
    assert_eq!(as_str(&out), "Two");
}

#[test]
fn value_to_text_unmatched_with_default_text() {
    // cc_ref has one extra (default) entry beyond cc_val.
    let mut block = cc(ConversionType::ValueToText, vec![1.0, 2.0], vec![1, 1, 1]);
    block.resolved_texts = Some(HashMap::from([
        (0usize, "One".to_string()),
        (1usize, "Two".to_string()),
        (2usize, "Default".to_string()),
    ]));
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(9), &[])
        .unwrap();
    assert_eq!(as_str(&out), "Default");
}

#[test]
fn value_to_text_unmatched_with_nil_default_passes_raw_through() {
    // Default slot exists but is NIL (0): unmatched values pass through raw.
    let block = cc(ConversionType::ValueToText, vec![1.0, 2.0], vec![0, 0, 0]);
    let out = block.apply_decoded(DecodedValue::Float(9.5), &[]).unwrap();
    assert_eq!(as_f64(&out), 9.5);
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(9), &[])
        .unwrap();
    assert!(matches!(out, DecodedValue::UnsignedInteger(9)));
}

#[test]
fn value_to_text_unmatched_without_default_slot_passes_raw_through() {
    // No default slot at all (cc_ref.len() == cc_val.len()).
    let block = cc(ConversionType::ValueToText, vec![1.0, 2.0], vec![0, 0]);
    let out = block.apply_decoded(DecodedValue::Float(7.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 7.0);
}

#[test]
fn value_to_text_matched_key_with_nil_text_link_yields_empty_string() {
    let block = cc(ConversionType::ValueToText, vec![1.0], vec![0]);
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(1), &[])
        .unwrap();
    assert_eq!(as_str(&out), "");
}

// -------------------------------------------------------- range-to-text (B2/B8)

#[test]
fn range_to_text_first_match_wins_on_overlapping_ranges() {
    // Overlapping ranges [0,10] -> "A" and [5,15] -> "B": 7 hits "A".
    let mut block = cc(
        ConversionType::RangeToText,
        vec![0.0, 10.0, 5.0, 15.0],
        vec![1, 1],
    );
    block.resolved_texts = Some(HashMap::from([
        (0usize, "A".to_string()),
        (1usize, "B".to_string()),
    ]));
    let out = block.apply_decoded(DecodedValue::Float(7.0), &[]).unwrap();
    assert_eq!(as_str(&out), "A");
    // 12 only falls in the second range.
    let out = block.apply_decoded(DecodedValue::Float(12.0), &[]).unwrap();
    assert_eq!(as_str(&out), "B");
}

#[test]
fn range_to_text_unmatched_with_nil_default_passes_raw_through() {
    let block = cc(
        ConversionType::RangeToText,
        vec![0.0, 10.0, 20.0, 30.0],
        vec![0, 0, 0],
    );
    let out = block.apply_decoded(DecodedValue::Float(15.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 15.0);
}

// -------------------------------------------------------- text-to-value (B8)

#[test]
fn text_to_value_first_matching_reference_wins() {
    // Duplicate key text at indices 0 and 2: index 0 must win every time.
    let mut block = cc(
        ConversionType::TextToValue,
        vec![10.0, 20.0, 30.0],
        vec![1, 1, 1],
    );
    block.resolved_texts = Some(HashMap::from([
        (0usize, "dup".to_string()),
        (1usize, "other".to_string()),
        (2usize, "dup".to_string()),
    ]));
    for _ in 0..50 {
        let out = block
            .apply_decoded(DecodedValue::String("dup".to_string()), &[])
            .unwrap();
        assert_eq!(as_f64(&out), 10.0);
    }
}

// ------------------------------------------------------------- bitfield (B4)

#[test]
fn bitfield_with_tx_only_ref_contributes_resolved_text() {
    let mut file_data = Vec::new();
    put_tx(&mut file_data, 64, b"FLAG_A");

    let mut block = cc(
        ConversionType::BitfieldText,
        vec![f64::from_bits(0x1)],
        vec![64],
    );
    block.resolve_all_dependencies(&file_data).unwrap();

    // Apply with EMPTY file data (index read): the TX text must contribute.
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(1), &[])
        .unwrap();
    assert_eq!(as_str(&out), "FLAG_A");
}

#[test]
fn bitfield_tx_only_ref_legacy_path_with_file_data() {
    // No resolution step: the legacy (file_data) path must also honor ##TX refs.
    let mut file_data = Vec::new();
    put_tx(&mut file_data, 64, b"FLAG_B");

    let block = cc(
        ConversionType::BitfieldText,
        vec![f64::from_bits(0x2)],
        vec![64],
    );
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(2), &file_data)
        .unwrap();
    assert_eq!(as_str(&out), "FLAG_B");
}

#[test]
fn bitfield_nested_conversion_name_survives_index_reads() {
    // Layout: TX "Status" (name) at 64, TX "On" at 128, nested ##CC at 192.
    let mut file_data = Vec::new();
    put_tx(&mut file_data, 64, b"Status");
    put_tx(&mut file_data, 128, b"On");

    let mut nested = cc(ConversionType::ValueToText, vec![1.0], vec![128]);
    nested.cc_tx_name = Some(64);
    put_cc(&mut file_data, 192, &nested);

    let mut block = cc(
        ConversionType::BitfieldText,
        vec![f64::from_bits(0x1)],
        vec![192],
    );
    block.resolve_all_dependencies(&file_data).unwrap();

    // Applying with EMPTY file data must not panic and must still include
    // the nested conversion's name, resolved at resolution time.
    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(1), &[])
        .unwrap();
    assert_eq!(as_str(&out), "Status = On");
}

#[test]
fn bitfield_nested_conversion_without_name_emits_value_only() {
    let mut file_data = Vec::new();
    put_tx(&mut file_data, 64, b"On");

    let nested = cc(ConversionType::ValueToText, vec![1.0], vec![64]);
    put_cc(&mut file_data, 128, &nested);

    let mut block = cc(
        ConversionType::BitfieldText,
        vec![f64::from_bits(0x1)],
        vec![128],
    );
    block.resolve_all_dependencies(&file_data).unwrap();

    let out = block
        .apply_decoded(DecodedValue::UnsignedInteger(1), &[])
        .unwrap();
    assert_eq!(as_str(&out), "On");
}

// ------------------------------------------------------------ algebraic (B5)

#[test]
fn algebraic_x1_alias_is_evaluated() {
    let mut block = cc(ConversionType::Algebraic, vec![], vec![]);
    block.formula = Some("X1*2+1".to_string());
    let out = block.apply_decoded(DecodedValue::Float(3.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 7.0);
}

#[test]
fn algebraic_plain_x_still_works() {
    let mut block = cc(ConversionType::Algebraic, vec![], vec![]);
    block.formula = Some("X*X + 1".to_string());
    let out = block.apply_decoded(DecodedValue::Float(3.0), &[]).unwrap();
    assert_eq!(as_f64(&out), 10.0);
}

// ------------------------------------------------------------ JSON serde (B6)

#[test]
fn json_round_trip_preserves_non_finite_cc_val_bit_for_bit() {
    let nan_mask = f64::from_bits(0xFFFF_FFFF_FFFF_FFFF);
    let mut block = cc(
        ConversionType::BitfieldText,
        vec![
            f64::INFINITY,
            f64::NEG_INFINITY,
            nan_mask,
            1.5,
            0.0,
            -2.25,
        ],
        vec![],
    );
    block.cc_phy_range_min = Some(f64::NEG_INFINITY);
    block.cc_phy_range_max = Some(f64::from_bits(0x7FF8_0000_0000_0001));

    let json = serde_json::to_string(&block).expect("serialize");
    let back: ConversionBlock = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(block.cc_val.len(), back.cc_val.len());
    for (i, (a, b)) in block.cc_val.iter().zip(back.cc_val.iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "cc_val[{}] changed: {:?} -> {:?}",
            i,
            a,
            b
        );
    }
    assert_eq!(
        block.cc_phy_range_min.unwrap().to_bits(),
        back.cc_phy_range_min.unwrap().to_bits()
    );
    assert_eq!(
        block.cc_phy_range_max.unwrap().to_bits(),
        back.cc_phy_range_max.unwrap().to_bits()
    );
}

#[test]
fn json_finite_values_stay_plain_numbers_and_none_ranges_stay_null() {
    let block = cc(ConversionType::Linear, vec![1.5, 3.0], vec![]);
    let json = serde_json::to_string(&block).expect("serialize");
    // Backward-compatible encoding: finite values are plain JSON numbers.
    assert!(json.contains("\"cc_val\":[1.5,3.0]"), "json was: {}", json);
    let back: ConversionBlock = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.cc_val, vec![1.5, 3.0]);
    assert_eq!(back.cc_phy_range_min, None);
    assert_eq!(back.cc_phy_range_max, None);
}

#[test]
fn json_legacy_null_cc_val_entries_deserialize_as_nan() {
    // Older versions let serde_json write non-finite values as null.
    let block = cc(ConversionType::Linear, vec![1.5], vec![]);
    let json = serde_json::to_string(&block).expect("serialize");
    let legacy = json.replace("\"cc_val\":[1.5]", "\"cc_val\":[null]");
    assert_ne!(json, legacy);
    let back: ConversionBlock = serde_json::from_str(&legacy).expect("deserialize");
    assert_eq!(back.cc_val.len(), 1);
    assert!(back.cc_val[0].is_nan());
}
