use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::common::{BlockHeader, BlockParse, DataType};
use mf4_rs::blocks::data_block::DataBlock;
use mf4_rs::blocks::data_group_block::DataGroupBlock;
use mf4_rs::blocks::data_list_block::DataListBlock;
use mf4_rs::blocks::header_block::HeaderBlock;
use mf4_rs::blocks::identification_block::IdentificationBlock;
use mf4_rs::blocks::metadata_block::MetadataBlock;
use mf4_rs::blocks::signal_data_block::SignalDataBlock;
use mf4_rs::blocks::source_block::SourceBlock;
use mf4_rs::blocks::text_block::TextBlock;
use mf4_rs::error::MdfError;

fn header(id: &str, len: u64, links: u64) -> BlockHeader {
    BlockHeader {
        id: id.to_string(),
        reserved0: 0,
        block_len: len,
        links_nr: links,
    }
}

#[test]
fn block_header_roundtrip() -> Result<(), MdfError> {
    let h = header("TEST", 64, 2);
    let bytes = h.to_bytes()?;
    let parsed = BlockHeader::from_bytes(&bytes)?;
    assert_eq!(parsed.id, "TEST");
    assert_eq!(parsed.block_len, 64);
    assert_eq!(parsed.links_nr, 2);
    Ok(())
}

#[test]
fn text_block_roundtrip() -> Result<(), MdfError> {
    let tb = TextBlock::new("hello");
    let bytes = tb.to_bytes()?;
    let parsed = TextBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.text, "hello");
    Ok(())
}

#[test]
fn metadata_block_parse() -> Result<(), MdfError> {
    let xml = "<x/>";
    let mut h = header("##MD", 0, 0);
    let needs_null = true;
    let base_len = 24 + xml.len() + if needs_null { 1 } else { 0 };
    let padding = (8 - (base_len % 8)) % 8;
    h.block_len = (base_len + padding) as u64;
    let mut bytes = h.to_bytes()?;
    bytes.extend_from_slice(xml.as_bytes());
    if needs_null {
        bytes.push(0);
    }
    bytes.extend_from_slice(&vec![0u8; padding]);
    let parsed = MetadataBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.xml, xml);
    Ok(())
}

#[test]
fn data_block_parse() -> Result<(), MdfError> {
    let data = vec![1u8, 2, 3, 4];
    let h = header("##DT", 24 + data.len() as u64, 0);
    let mut bytes = h.to_bytes()?;
    bytes.extend_from_slice(&data);
    let block = DataBlock::from_bytes(&bytes)?;
    assert_eq!(block.data, &data[..]);
    Ok(())
}

#[test]
fn data_list_block_roundtrip() -> Result<(), MdfError> {
    let dl = DataListBlock::new_equal(vec![0x10, 0x20], 8);
    let bytes = dl.to_bytes()?;
    let parsed = DataListBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.data_links, vec![0x10, 0x20]);
    assert_eq!(parsed.data_block_len, Some(8));
    Ok(())
}

#[test]
fn signal_data_block_parse() -> Result<(), MdfError> {
    let h = header("##SD", 32, 0);
    let mut bytes = h.to_bytes()?;
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.push(42);
    bytes.extend_from_slice(&[0u8; 7]);
    let sd = SignalDataBlock::from_bytes(&bytes)?;
    assert_eq!(sd.data[0..4], 1u32.to_le_bytes());
    assert_eq!(sd.data[4], 42);
    Ok(())
}

#[test]
fn source_block_parse() -> Result<(), MdfError> {
    let h = header("##SI", 56, 3);
    let mut bytes = h.to_bytes()?;
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    bytes.extend_from_slice(&3u64.to_le_bytes());
    bytes.extend_from_slice(&[1, 2, 3, 0, 0, 0, 0, 0]);
    let sb = SourceBlock::from_bytes(&bytes)?;
    assert_eq!(sb.name_addr, 1);
    assert_eq!(sb.path_addr, 2);
    assert_eq!(sb.comment_addr, 3);
    assert_eq!(sb.source_type, 1);
    assert_eq!(sb.bus_type, 2);
    assert_eq!(sb.flags, 3);
    Ok(())
}

#[test]
fn identification_block_roundtrip() -> Result<(), MdfError> {
    let ib = IdentificationBlock::default();
    let bytes = ib.to_bytes()?;
    let parsed = IdentificationBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.version_number, ib.version_number);
    Ok(())
}

#[test]
fn header_block_roundtrip() -> Result<(), MdfError> {
    let hb = HeaderBlock::default();
    let bytes = hb.to_bytes()?;
    let parsed = HeaderBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.first_dg_addr, 0);
    assert_eq!(parsed.header.id, "##HD");
    Ok(())
}

#[test]
fn data_group_block_roundtrip() -> Result<(), MdfError> {
    let dg = DataGroupBlock::default();
    let bytes = dg.to_bytes()?;
    let parsed = DataGroupBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.record_id_len, dg.record_id_len);
    Ok(())
}

#[test]
fn channel_group_block_roundtrip() -> Result<(), MdfError> {
    let cg = ChannelGroupBlock::default();
    let bytes = cg.to_bytes()?;
    let parsed = ChannelGroupBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.samples_byte_nr, cg.samples_byte_nr);
    Ok(())
}

#[test]
fn channel_group_block_link_count_follows_master_link() -> Result<(), MdfError> {
    // MDF 4.1: 104 bytes, byte-identical round trip.
    let mut cg = ChannelGroupBlock::default();
    cg.cycles_nr = 42;
    cg.samples_byte_nr = 9;
    let bytes = cg.to_bytes()?;
    assert_eq!(bytes.len(), 104);
    assert_eq!(ChannelGroupBlock::from_bytes(&bytes)?.to_bytes()?, bytes);

    // A remote master: 7 links / 112 bytes and flag bit 3, whatever the header says.
    cg.master_cg_addr = 0x1000;
    let bytes = cg.to_bytes()?;
    assert_eq!(bytes.len(), 112);
    let parsed = ChannelGroupBlock::from_bytes(&bytes)?;
    assert_eq!((parsed.header.links_nr, parsed.header.block_len), (7, 112));
    assert_eq!(parsed.master_cg_addr, 0x1000);
    assert_eq!(parsed.flags & (1 << 3), 1 << 3);
    assert_eq!((parsed.cycles_nr, parsed.samples_byte_nr), (42, 9));
    assert_eq!(parsed.to_bytes()?, bytes);

    // A parsed 7-link block without a master keeps its 7 links (null link).
    let mut parsed = parsed;
    parsed.master_cg_addr = 0;
    let bytes = parsed.to_bytes()?;
    assert_eq!(bytes.len(), 112);
    let again = ChannelGroupBlock::from_bytes(&bytes)?;
    assert_eq!((again.master_cg_addr, again.flags & (1 << 3)), (0, 0));
    assert_eq!(again.cycles_nr, 42);

    // Data section beyond 7 links is read at the offset the link count implies.
    let mut eight = bytes[..24 + 56].to_vec();
    eight.extend_from_slice(&0u64.to_le_bytes());
    eight.extend_from_slice(&bytes[24 + 56..]);
    eight[8..16].copy_from_slice(&120u64.to_le_bytes());
    eight[16..24].copy_from_slice(&8u64.to_le_bytes());
    assert_eq!(ChannelGroupBlock::from_bytes(&eight)?.cycles_nr, 42);

    // A link count the block length cannot hold is an error.
    let mut bad = cg.to_bytes()?;
    bad[8..16].copy_from_slice(&104u64.to_le_bytes());
    assert!(ChannelGroupBlock::from_bytes(&bad).is_err());
    Ok(())
}

#[test]
fn channel_block_roundtrip() -> Result<(), MdfError> {
    let ch = ChannelBlock::default();
    let bytes = ch.to_bytes()?;
    let parsed = ChannelBlock::from_bytes(&bytes)?;
    assert_eq!(parsed.bit_count, ch.bit_count);
    assert_eq!(
        parsed.data_type.to_u8(),
        DataType::UnsignedIntegerLE.to_u8()
    );
    Ok(())
}
