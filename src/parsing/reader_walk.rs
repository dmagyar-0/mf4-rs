//! Walk an MDF file's metadata via a [`crate::index::ByteRangeReader`].
//!
//! Mirrors the mmap-based walk in [`crate::parsing::mdf_file::MdfFile::parse_from_file`]
//! but issues range reads instead of slicing into a memory map. Only metadata
//! is read (block headers, channel/group descriptors, name/comment text
//! blocks, conversion blocks). Sample data is not touched. This is the
//! foundation for building an [`crate::index::MdfIndex`] from a remote source
//! such as an HTTP URL or S3 object without downloading the whole file.

use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::blocks::common::{read_string_block_via_reader, BlockHeader, BlockParse};
use crate::blocks::conversion::ConversionBlock;
use crate::blocks::data_group_block::DataGroupBlock;
use crate::blocks::header_block::HeaderBlock;
use crate::blocks::identification_block::IdentificationBlock;
use crate::error::MdfError;
use crate::index::ByteRangeReader;

pub(crate) struct WalkedChannel {
    pub block: ChannelBlock,
    pub name: Option<String>,
    pub unit: Option<String>,
    pub conversion: Option<ConversionBlock>,
}

pub(crate) struct WalkedGroup {
    pub record_id_len: u8,
    pub data_block_addr: u64,
    pub cg: ChannelGroupBlock,
    pub cg_name: Option<String>,
    pub cg_comment: Option<String>,
    pub channels: Vec<WalkedChannel>,
}

pub(crate) struct ReaderWalkResult {
    #[allow(dead_code)]
    pub identification: IdentificationBlock,
    pub header: HeaderBlock,
    pub groups: Vec<WalkedGroup>,
}

const ID_BLOCK_LEN: u64 = 64;
const HD_BLOCK_LEN: u64 = 104;
const DG_BLOCK_LEN: u64 = 64;
const CG_BLOCK_LEN: u64 = 104;
const CN_BLOCK_LEN: u64 = 160;

pub(crate) fn walk<R>(reader: &mut R) -> Result<ReaderWalkResult, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    // ##ID at offset 0, ##HD at offset 64.
    let id_bytes = reader.read_range(0, ID_BLOCK_LEN)?;
    let identification = IdentificationBlock::from_bytes(&id_bytes)?;

    let hd_bytes = reader.read_range(ID_BLOCK_LEN, HD_BLOCK_LEN)?;
    let header = HeaderBlock::from_bytes(&hd_bytes)?;

    let mut groups = Vec::new();
    let mut dg_addr = header.first_dg_addr;
    while dg_addr != 0 {
        let dg_bytes = reader.read_range(dg_addr, DG_BLOCK_LEN)?;
        let dg = DataGroupBlock::from_bytes(&dg_bytes)?;
        let next_dg_addr = dg.next_dg_addr;
        let mut cg_addr = dg.first_cg_addr;

        while cg_addr != 0 {
            let cg_bytes = reader.read_range(cg_addr, CG_BLOCK_LEN)?;
            let cg = ChannelGroupBlock::from_bytes(&cg_bytes)?;
            let next_cg_addr = cg.next_cg_addr;

            let cg_name = read_string_block_via_reader(reader, cg.acq_name_addr)?;
            let cg_comment = read_string_block_via_reader(reader, cg.comment_addr)?;

            let mut channels = Vec::new();
            let mut ch_addr = cg.first_ch_addr;
            while ch_addr != 0 {
                let cn_bytes = reader.read_range(ch_addr, CN_BLOCK_LEN)?;
                let cn = ChannelBlock::from_bytes(&cn_bytes)?;
                let next_ch_addr = cn.next_ch_addr;

                let name = read_string_block_via_reader(reader, cn.name_addr)?;
                let unit = read_string_block_via_reader(reader, cn.unit_addr)?;

                let conversion = if cn.conversion_addr != 0 {
                    let cc_header_bytes = reader.read_range(cn.conversion_addr, 24)?;
                    let cc_header = BlockHeader::from_bytes(&cc_header_bytes)?;
                    let cc_full =
                        reader.read_range(cn.conversion_addr, cc_header.block_len)?;
                    let mut cc = ConversionBlock::from_bytes(&cc_full)?;
                    if let Err(e) =
                        cc.resolve_all_dependencies_via_reader(reader, cn.conversion_addr)
                    {
                        eprintln!(
                            "Warning: failed to resolve conversion for channel {:?}: {e}",
                            name.as_deref().unwrap_or("<unnamed>")
                        );
                    }
                    Some(cc)
                } else {
                    None
                };

                channels.push(WalkedChannel {
                    block: cn,
                    name,
                    unit,
                    conversion,
                });

                ch_addr = next_ch_addr;
            }

            groups.push(WalkedGroup {
                record_id_len: dg.record_id_len,
                data_block_addr: dg.data_block_addr,
                cg,
                cg_name,
                cg_comment,
                channels,
            });

            cg_addr = next_cg_addr;
        }

        dg_addr = next_dg_addr;
    }

    Ok(ReaderWalkResult {
        identification,
        header,
        groups,
    })
}
