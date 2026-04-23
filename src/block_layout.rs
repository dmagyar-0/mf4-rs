//! Block layout visualization for MDF4 files.
//!
//! Walks every block in a memory-mapped MDF file, records the byte range it
//! occupies and the links it carries, and renders the result as a flat table,
//! an indented tree, or JSON. Useful for inspecting how data is laid out on
//! disk, verifying link chains and comparing structure against the MDF 4.1
//! specification.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;

use byteorder::{ByteOrder, LittleEndian};
use serde::{Deserialize, Serialize};

use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::blocks::common::{BlockHeader, BlockParse};
use crate::blocks::conversion::ConversionBlock;
use crate::blocks::data_group_block::DataGroupBlock;
use crate::blocks::data_list_block::DataListBlock;
use crate::blocks::header_block::HeaderBlock;
use crate::blocks::identification_block::IdentificationBlock;
use crate::blocks::metadata_block::MetadataBlock;
use crate::blocks::source_block::SourceBlock;
use crate::blocks::text_block::TextBlock;
use crate::error::MdfError;

/// A named link inside a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkInfo {
    /// Human-readable name of the link field (e.g. `first_dg_addr`).
    pub name: String,
    /// Absolute file offset the link points to. `0` means null.
    pub target: u64,
    /// Block ID at that offset if it could be resolved (e.g. `##TX`).
    pub target_type: Option<String>,
}

/// A single block visited while walking the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockInfo {
    /// Absolute file offset where the block starts.
    pub offset: u64,
    /// Byte immediately after the block (`offset + size`).
    pub end_offset: u64,
    /// Total size of the block in bytes (`block_len` from its header, or 64
    /// for the identification block).
    pub size: u64,
    /// MDF block ID (`##HD`, `##DG`, …) or `##ID` for the identification
    /// block.
    pub block_type: String,
    /// Short description of the block's role (e.g. `Channel 'Time'`).
    pub description: String,
    /// All links carried by the block, in their on-disk order.
    pub links: Vec<LinkInfo>,
    /// Additional free-form details (e.g. record layout of a `##DT`).
    pub extra: Option<String>,
}

/// A stretch of the file that was not covered by any visited block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapInfo {
    pub start: u64,
    pub end: u64,
    pub size: u64,
}

/// Full layout of an MDF file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLayout {
    pub file_size: u64,
    /// Blocks sorted by offset.
    pub blocks: Vec<BlockInfo>,
    /// Byte ranges inside the file that no visited block covered.
    pub gaps: Vec<GapInfo>,
}

impl FileLayout {
    /// Build a layout by reading an MDF file from disk into memory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: &str) -> Result<Self, MdfError> {
        let data = fs::read(path)?;
        Self::from_bytes(&data)
    }

    /// Build a layout by walking an in-memory MDF image.
    pub fn from_bytes(data: &[u8]) -> Result<Self, MdfError> {
        let mut walker = Walker::new(data);
        walker.walk()?;
        let mut blocks = walker.blocks;
        blocks.sort_by_key(|b| b.offset);
        let gaps = compute_gaps(data.len() as u64, &blocks);
        Ok(FileLayout {
            file_size: data.len() as u64,
            blocks,
            gaps,
        })
    }

    /// Render a flat, sorted listing followed by per-block link sections.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "MDF block layout (file size: {} bytes / 0x{:x})",
            self.file_size, self.file_size
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "Blocks ({} total), sorted by offset:", self.blocks.len());
        let _ = writeln!(
            out,
            "{:<12}  {:<12}  {:>10}  {:<6}  {}",
            "offset", "end", "size", "type", "description"
        );
        let _ = writeln!(
            out,
            "{:-<12}  {:-<12}  {:->10}  {:-<6}  {:-<60}",
            "", "", "", "", ""
        );
        for b in &self.blocks {
            let _ = writeln!(
                out,
                "0x{:010x}  0x{:010x}  {:>10}  {:<6}  {}",
                b.offset, b.end_offset, b.size, b.block_type, b.description
            );
            if let Some(extra) = &b.extra {
                let _ = writeln!(out, "{:<44}{}", "", extra);
            }
        }

        if !self.gaps.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "Unreferenced byte ranges (gaps):");
            for g in &self.gaps {
                let _ = writeln!(
                    out,
                    "  0x{:010x} .. 0x{:010x}  ({} bytes)",
                    g.start, g.end, g.size
                );
            }
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "Block links:");
        for b in &self.blocks {
            if b.links.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "  {} @ 0x{:010x} ({})",
                b.block_type, b.offset, b.description
            );
            for link in &b.links {
                if link.target == 0 {
                    let _ = writeln!(out, "    {:<28} -> (null)", link.name);
                } else {
                    let type_label = link.target_type.as_deref().unwrap_or("?");
                    let _ = writeln!(
                        out,
                        "    {:<28} -> 0x{:010x} ({})",
                        link.name, link.target, type_label
                    );
                }
            }
        }
        out
    }

    /// Render the link graph as an indented tree starting at `##ID`.
    pub fn to_tree(&self) -> String {
        let by_offset: HashMap<u64, &BlockInfo> =
            self.blocks.iter().map(|b| (b.offset, b)).collect();
        let mut out = String::new();
        let mut visited = HashSet::new();
        if let Some(root) = by_offset.get(&0) {
            render_tree_node(root, &by_offset, &mut visited, &mut out, "", true, true, None);
        }
        // Any block unreachable from ##ID (shouldn't normally happen) gets
        // appended as extra roots so nothing is silently dropped.
        for b in &self.blocks {
            if !visited.contains(&b.offset) {
                render_tree_node(b, &by_offset, &mut visited, &mut out, "", true, true, None);
            }
        }
        out
    }

    /// Serialize the layout to a pretty JSON string.
    pub fn to_json(&self) -> Result<String, MdfError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| MdfError::BlockSerializationError(format!("layout JSON: {e}")))
    }

    /// Write the text listing to `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_text_to_file(&self, path: &str) -> Result<(), MdfError> {
        fs::write(path, self.to_text())?;
        Ok(())
    }

    /// Write the tree view to `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_tree_to_file(&self, path: &str) -> Result<(), MdfError> {
        fs::write(path, self.to_tree())?;
        Ok(())
    }

    /// Write JSON to `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_json_to_file(&self, path: &str) -> Result<(), MdfError> {
        fs::write(path, self.to_json()?)?;
        Ok(())
    }
}

fn compute_gaps(file_size: u64, blocks: &[BlockInfo]) -> Vec<GapInfo> {
    let mut gaps = Vec::new();
    let mut cursor = 0u64;
    for b in blocks {
        if b.offset > cursor {
            gaps.push(GapInfo {
                start: cursor,
                end: b.offset,
                size: b.offset - cursor,
            });
        }
        if b.end_offset > cursor {
            cursor = b.end_offset;
        }
    }
    if cursor < file_size {
        gaps.push(GapInfo {
            start: cursor,
            end: file_size,
            size: file_size - cursor,
        });
    }
    gaps
}

fn render_tree_node(
    node: &BlockInfo,
    by_offset: &HashMap<u64, &BlockInfo>,
    visited: &mut HashSet<u64>,
    out: &mut String,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    link_label: Option<&str>,
) {
    let connector = if is_root {
        ""
    } else if is_last {
        "`-- "
    } else {
        "|-- "
    };
    let label = match link_label {
        Some(l) => format!("({}) ", l),
        None => String::new(),
    };
    let already = visited.contains(&node.offset);
    let _ = writeln!(
        out,
        "{}{}{}{} @ 0x{:010x} [{}]{}",
        prefix,
        connector,
        label,
        node.block_type,
        node.offset,
        node.description,
        if already { " (already listed)" } else { "" }
    );
    if already {
        return;
    }
    visited.insert(node.offset);

    let child_prefix = if is_root {
        String::new()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}|   ", prefix)
    };

    // Resolve children: keep only links that point at a block we actually
    // visited (so we render in the original link order).
    let children: Vec<(usize, &LinkInfo)> = node
        .links
        .iter()
        .enumerate()
        .filter(|(_, l)| l.target != 0 && by_offset.contains_key(&l.target))
        .collect();
    let count = children.len();
    for (idx, (_, link)) in children.into_iter().enumerate() {
        let child = by_offset[&link.target];
        let last_child = idx + 1 == count;
        render_tree_node(
            child,
            by_offset,
            visited,
            out,
            &child_prefix,
            last_child,
            false,
            Some(link.name.as_str()),
        );
    }
}

// ---------------------------------------------------------------------------
// Walker implementation
// ---------------------------------------------------------------------------

struct Walker<'a> {
    data: &'a [u8],
    visited: HashSet<u64>,
    blocks: Vec<BlockInfo>,
    dg_counter: usize,
    cg_counter: usize,
    cn_counter: usize,
}

impl<'a> Walker<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            visited: HashSet::new(),
            blocks: Vec::new(),
            dg_counter: 0,
            cg_counter: 0,
            cn_counter: 0,
        }
    }

    fn walk(&mut self) -> Result<(), MdfError> {
        // ##ID (not a standard block header - exactly 64 bytes at offset 0).
        let id = IdentificationBlock::from_bytes(&self.data[0..64])?;
        self.blocks.push(BlockInfo {
            offset: 0,
            end_offset: 64,
            size: 64,
            block_type: "##ID".to_string(),
            description: format!(
                "Identification: version='{}', program='{}'",
                id.version_identifier.trim(),
                id.program_identifier.trim_end()
            ),
            links: Vec::new(),
            extra: Some(format!(
                "version_number={}, standard_unfinalized_flags=0x{:04x}, custom_unfinalized_flags=0x{:04x}",
                id.version_number, id.standard_unfinalized_flags, id.custom_unfinalized_flags
            )),
        });
        self.visited.insert(0);

        self.walk_header(64)?;
        Ok(())
    }

    fn peek_id(&self, offset: u64) -> Option<String> {
        if offset == 0 {
            return None;
        }
        let o = offset as usize;
        if o + 4 > self.data.len() {
            return None;
        }
        Some(String::from_utf8_lossy(&self.data[o..o + 4]).to_string())
    }

    fn walk_header(&mut self, offset: u64) -> Result<(), MdfError> {
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let hd = HeaderBlock::from_bytes(&self.data[o..o + 104])?;

        let links = vec![
            self.make_link("first_dg_addr", hd.first_dg_addr),
            self.make_link("file_history_addr", hd.file_history_addr),
            self.make_link("channel_tree_addr", hd.channel_tree_addr),
            self.make_link("first_attachment_addr", hd.first_attachment_addr),
            self.make_link("first_event_addr", hd.first_event_addr),
            self.make_link("comment_addr", hd.comment_addr),
        ];
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + 104,
            size: 104,
            block_type: "##HD".to_string(),
            description: format!("Header Block (abs_time={} ns, tz={} min)", hd.abs_time, hd.tz_offset),
            links,
            extra: Some(format!(
                "flags=0x{:02x}, time_flags=0x{:02x}, time_quality=0x{:02x}",
                hd.flags, hd.time_flags, hd.time_quality
            )),
        });

        // Comment before data groups, so text blocks appear in file order.
        self.walk_text_like(hd.comment_addr)?;

        let mut dg_addr = hd.first_dg_addr;
        while dg_addr != 0 {
            let next = self.walk_data_group(dg_addr)?;
            dg_addr = next;
        }
        Ok(())
    }

    fn walk_data_group(&mut self, offset: u64) -> Result<u64, MdfError> {
        if !self.visited.insert(offset) {
            return Ok(0);
        }
        let o = offset as usize;
        let dg = DataGroupBlock::from_bytes(&self.data[o..])?;
        let size = dg.header.block_len;

        let links = vec![
            self.make_link("next_dg_addr", dg.next_dg_addr),
            self.make_link("first_cg_addr", dg.first_cg_addr),
            self.make_link("data_block_addr", dg.data_block_addr),
            self.make_link("comment_addr", dg.comment_addr),
        ];

        let index = self.dg_counter;
        self.dg_counter += 1;
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##DG".to_string(),
            description: format!("Data Group #{} (record_id_len={})", index, dg.record_id_len),
            links,
            extra: None,
        });

        self.walk_text_like(dg.comment_addr)?;

        // Walk all channel groups first so we know the record layout before we
        // describe the data block.
        let mut first_cg_record_size: Option<usize> = None;
        let mut first_cg_invalidation: u32 = 0;
        let mut cg_addr = dg.first_cg_addr;
        let mut cg_count = 0;
        while cg_addr != 0 {
            let (next, record_size, inval) =
                self.walk_channel_group(cg_addr, dg.record_id_len)?;
            if cg_count == 0 {
                first_cg_record_size = Some(record_size);
                first_cg_invalidation = inval;
            }
            cg_addr = next;
            cg_count += 1;
        }

        if dg.data_block_addr != 0 {
            self.walk_data_region(
                dg.data_block_addr,
                first_cg_record_size,
                first_cg_invalidation,
                dg.record_id_len,
            )?;
        }

        Ok(dg.next_dg_addr)
    }

    fn walk_channel_group(
        &mut self,
        offset: u64,
        record_id_len: u8,
    ) -> Result<(u64, usize, u32), MdfError> {
        if !self.visited.insert(offset) {
            return Ok((0, 0, 0));
        }
        let o = offset as usize;
        let cg = ChannelGroupBlock::from_bytes(&self.data[o..])?;
        let size = cg.header.block_len;
        let record_size = record_id_len as usize
            + cg.samples_byte_nr as usize
            + cg.invalidation_bytes_nr as usize;

        let links = vec![
            self.make_link("next_cg_addr", cg.next_cg_addr),
            self.make_link("first_ch_addr", cg.first_ch_addr),
            self.make_link("acq_name_addr", cg.acq_name_addr),
            self.make_link("acq_source_addr", cg.acq_source_addr),
            self.make_link("first_sample_reduction_addr", cg.first_sample_reduction_addr),
            self.make_link("comment_addr", cg.comment_addr),
        ];

        let index = self.cg_counter;
        self.cg_counter += 1;
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##CG".to_string(),
            description: format!(
                "Channel Group #{} (cycles={}, record={}B = {}B data + {}B inval + {}B record_id)",
                index,
                cg.cycles_nr,
                record_size,
                cg.samples_byte_nr,
                cg.invalidation_bytes_nr,
                record_id_len
            ),
            links,
            extra: Some(format!("flags=0x{:04x}, record_id={}", cg.flags, cg.record_id)),
        });

        self.walk_text_like(cg.acq_name_addr)?;
        self.walk_source(cg.acq_source_addr)?;
        self.walk_text_like(cg.comment_addr)?;

        let mut ch_addr = cg.first_ch_addr;
        while ch_addr != 0 {
            let next = self.walk_channel(ch_addr)?;
            ch_addr = next;
        }

        Ok((cg.next_cg_addr, record_size, cg.invalidation_bytes_nr))
    }

    fn walk_channel(&mut self, offset: u64) -> Result<u64, MdfError> {
        if !self.visited.insert(offset) {
            return Ok(0);
        }
        let o = offset as usize;
        let cn = ChannelBlock::from_bytes(&self.data[o..])?;
        let size = cn.header.block_len;

        let name = read_text_at(self.data, cn.name_addr).unwrap_or_default();

        let links = vec![
            self.make_link("next_ch_addr", cn.next_ch_addr),
            self.make_link("component_addr", cn.component_addr),
            self.make_link("name_addr", cn.name_addr),
            self.make_link("source_addr", cn.source_addr),
            self.make_link("conversion_addr", cn.conversion_addr),
            self.make_link("data", cn.data),
            self.make_link("unit_addr", cn.unit_addr),
            self.make_link("comment_addr", cn.comment_addr),
        ];

        let index = self.cn_counter;
        self.cn_counter += 1;
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##CN".to_string(),
            description: format!(
                "Channel #{} '{}' (ch_type={}, sync={}, dtype={:?}, {}b @ byte={} bit={})",
                index,
                name,
                cn.channel_type,
                cn.sync_type,
                cn.data_type,
                cn.bit_count,
                cn.byte_offset,
                cn.bit_offset
            ),
            links,
            extra: Some(format!(
                "flags=0x{:08x}, pos_invalidation_bit={}, precision={}",
                cn.flags, cn.pos_invalidation_bit, cn.precision
            )),
        });

        self.walk_text_like(cn.name_addr)?;
        self.walk_source(cn.source_addr)?;
        self.walk_conversion(cn.conversion_addr)?;
        self.walk_text_like(cn.unit_addr)?;
        self.walk_text_like(cn.comment_addr)?;

        // VLSD channels reuse the `data` field to point at SD/DL chains.
        if cn.channel_type == 1 && cn.data != 0 {
            self.walk_data_region(cn.data, None, 0, 0)?;
        }

        Ok(cn.next_ch_addr)
    }

    fn walk_conversion(&mut self, offset: u64) -> Result<(), MdfError> {
        if offset == 0 {
            return Ok(());
        }
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let cc = ConversionBlock::from_bytes(&self.data[o..])?;
        let size = cc.header.block_len;

        let mut links = vec![
            self.make_link("cc_tx_name", cc.cc_tx_name.unwrap_or(0)),
            self.make_link("cc_md_unit", cc.cc_md_unit.unwrap_or(0)),
            self.make_link("cc_md_comment", cc.cc_md_comment.unwrap_or(0)),
            self.make_link("cc_cc_inverse", cc.cc_cc_inverse.unwrap_or(0)),
        ];
        for (i, l) in cc.cc_ref.iter().enumerate() {
            links.push(self.make_link(&format!("cc_ref[{}]", i), *l));
        }

        let extra = format!(
            "cc_val_count={}, cc_ref_count={}, cc_flags=0x{:04x}",
            cc.cc_val_count, cc.cc_ref_count, cc.cc_flags
        );

        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##CC".to_string(),
            description: format!("Conversion ({:?})", cc.cc_type),
            links,
            extra: Some(extra),
        });

        self.walk_text_like(cc.cc_tx_name.unwrap_or(0))?;
        self.walk_text_like(cc.cc_md_unit.unwrap_or(0))?;
        self.walk_text_like(cc.cc_md_comment.unwrap_or(0))?;
        self.walk_conversion(cc.cc_cc_inverse.unwrap_or(0))?;
        for l in &cc.cc_ref {
            let id = self.peek_id(*l);
            match id.as_deref() {
                Some("##TX") | Some("##MD") => self.walk_text_like(*l)?,
                Some("##CC") => self.walk_conversion(*l)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn walk_source(&mut self, offset: u64) -> Result<(), MdfError> {
        if offset == 0 {
            return Ok(());
        }
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let si = SourceBlock::from_bytes(&self.data[o..])?;
        let size = si.header.block_len;

        let links = vec![
            self.make_link("name_addr", si.name_addr),
            self.make_link("path_addr", si.path_addr),
            self.make_link("comment_addr", si.comment_addr),
        ];
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##SI".to_string(),
            description: format!(
                "Source (type={}, bus_type={}, flags=0x{:02x})",
                si.source_type, si.bus_type, si.flags
            ),
            links,
            extra: None,
        });

        self.walk_text_like(si.name_addr)?;
        self.walk_text_like(si.path_addr)?;
        self.walk_text_like(si.comment_addr)?;
        Ok(())
    }

    fn walk_text_like(&mut self, offset: u64) -> Result<(), MdfError> {
        if offset == 0 {
            return Ok(());
        }
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        if o + 24 > self.data.len() {
            return Ok(());
        }
        let header = BlockHeader::from_bytes(&self.data[o..o + 24])?;
        match header.id.as_str() {
            "##TX" => {
                let tx = TextBlock::from_bytes(&self.data[o..])?;
                let preview = preview_string(&tx.text, 48);
                self.blocks.push(BlockInfo {
                    offset,
                    end_offset: offset + header.block_len,
                    size: header.block_len,
                    block_type: "##TX".to_string(),
                    description: format!("Text: \"{}\" ({} chars)", preview, tx.text.chars().count()),
                    links: Vec::new(),
                    extra: None,
                });
            }
            "##MD" => {
                let md = MetadataBlock::from_bytes(&self.data[o..])?;
                let preview = preview_string(&md.xml.replace('\n', " "), 48);
                self.blocks.push(BlockInfo {
                    offset,
                    end_offset: offset + header.block_len,
                    size: header.block_len,
                    block_type: "##MD".to_string(),
                    description: format!("Metadata (XML, {} chars): {}", md.xml.len(), preview),
                    links: Vec::new(),
                    extra: None,
                });
            }
            _ => {
                // Unknown ID - record the raw block so we don't pretend it
                // isn't there.
                self.blocks.push(BlockInfo {
                    offset,
                    end_offset: offset + header.block_len,
                    size: header.block_len,
                    block_type: header.id.clone(),
                    description: "Unknown block (not walked further)".to_string(),
                    links: Vec::new(),
                    extra: None,
                });
            }
        }
        Ok(())
    }

    fn walk_data_region(
        &mut self,
        offset: u64,
        record_size: Option<usize>,
        invalidation_bytes_nr: u32,
        record_id_len: u8,
    ) -> Result<(), MdfError> {
        if offset == 0 {
            return Ok(());
        }
        let id = self.peek_id(offset).unwrap_or_default();
        match id.as_str() {
            "##DT" | "##DV" | "##RD" | "##RV" => {
                self.record_simple_data_block(offset, &id, record_size, invalidation_bytes_nr, record_id_len)?;
            }
            "##SD" => {
                self.record_signal_data_block(offset)?;
            }
            "##DL" => {
                self.walk_data_list(offset, record_size, invalidation_bytes_nr, record_id_len)?;
            }
            "" => {}
            other => {
                // Unrecognised data block id - record it flat.
                if self.visited.insert(offset) {
                    let o = offset as usize;
                    if o + 24 <= self.data.len() {
                        let header = BlockHeader::from_bytes(&self.data[o..o + 24])?;
                        self.blocks.push(BlockInfo {
                            offset,
                            end_offset: offset + header.block_len,
                            size: header.block_len,
                            block_type: other.to_string(),
                            description: "Unknown data block".to_string(),
                            links: Vec::new(),
                            extra: None,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn record_simple_data_block(
        &mut self,
        offset: u64,
        id: &str,
        record_size: Option<usize>,
        invalidation_bytes_nr: u32,
        record_id_len: u8,
    ) -> Result<(), MdfError> {
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let header = BlockHeader::from_bytes(&self.data[o..o + 24])?;
        let size = header.block_len;
        let payload = size.saturating_sub(24);

        let extra = match record_size {
            Some(rs) if rs > 0 => {
                let records = payload as usize / rs;
                let leftover = payload as usize % rs;
                Some(format!(
                    "payload={} bytes, header=24 bytes, record layout = {}B (record_id={}B + data + invalidation={}B), records={}{}",
                    payload,
                    rs,
                    record_id_len,
                    invalidation_bytes_nr,
                    records,
                    if leftover > 0 {
                        format!(", trailing={}B", leftover)
                    } else {
                        String::new()
                    }
                ))
            }
            _ => Some(format!("payload={} bytes (header=24 bytes)", payload)),
        };

        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: id.to_string(),
            description: format!("Data block ({} payload bytes)", payload),
            links: Vec::new(),
            extra,
        });
        Ok(())
    }

    fn record_signal_data_block(&mut self, offset: u64) -> Result<(), MdfError> {
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let header = BlockHeader::from_bytes(&self.data[o..o + 24])?;
        let size = header.block_len;
        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##SD".to_string(),
            description: format!("Signal data (VLSD stream, {} payload bytes)", size.saturating_sub(24)),
            links: Vec::new(),
            extra: None,
        });
        Ok(())
    }

    fn walk_data_list(
        &mut self,
        offset: u64,
        record_size: Option<usize>,
        invalidation_bytes_nr: u32,
        record_id_len: u8,
    ) -> Result<(), MdfError> {
        if !self.visited.insert(offset) {
            return Ok(());
        }
        let o = offset as usize;
        let dl = DataListBlock::from_bytes(&self.data[o..])?;
        let size = dl.header.block_len;

        let mut links = vec![self.make_link("next", dl.next)];
        for (i, l) in dl.data_links.iter().enumerate() {
            links.push(self.make_link(&format!("data_links[{}]", i), *l));
        }

        let desc = format!(
            "Data List ({} fragments{})",
            dl.data_block_nr,
            if dl.flags & 1 != 0 {
                format!(", equal_length={}B", dl.data_block_len.unwrap_or(0))
            } else {
                String::new()
            }
        );

        self.blocks.push(BlockInfo {
            offset,
            end_offset: offset + size,
            size,
            block_type: "##DL".to_string(),
            description: desc,
            links,
            extra: Some(format!("flags=0x{:02x}", dl.flags)),
        });

        for l in dl.data_links.iter() {
            self.walk_data_region(*l, record_size, invalidation_bytes_nr, record_id_len)?;
        }
        if dl.next != 0 {
            self.walk_data_region(dl.next, record_size, invalidation_bytes_nr, record_id_len)?;
        }
        Ok(())
    }

    fn make_link(&self, name: &str, target: u64) -> LinkInfo {
        LinkInfo {
            name: name.to_string(),
            target,
            target_type: self.peek_id(target),
        }
    }
}

fn read_text_at(data: &[u8], addr: u64) -> Option<String> {
    if addr == 0 {
        return None;
    }
    let o = addr as usize;
    if o + 24 > data.len() {
        return None;
    }
    let id = &data[o..o + 4];
    if id != b"##TX" {
        return None;
    }
    let block_len = LittleEndian::read_u64(&data[o + 8..o + 16]);
    let end = o + block_len as usize;
    if end > data.len() {
        return None;
    }
    let raw = &data[o + 24..end];
    Some(
        String::from_utf8_lossy(raw)
            .trim_matches('\0')
            .to_string(),
    )
}

fn preview_string(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let taken: String = s.chars().take(max_chars).collect();
        format!("{}...", taken)
    }
}
