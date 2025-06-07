use crate::blocks::common::read_string_block;
use crate::blocks::source_block::{read_source_block, SourceBlock};
use crate::error::MdfError;

/// Ergonomic view of an SIBLOCK: human‐readable source name, path, comment.
#[derive(Debug)]
pub struct SourceInfo {
    /// The “source name” (si_tx_name)
    pub name:    Option<String>,
    /// The “source path” (si_tx_path)
    pub path:    Option<String>,
    /// Any extended comment/XML (si_md_comment)
    pub comment: Option<String>,
}

impl SourceInfo {
    /// Parse a SourceBlock from the mmap at `address` and read its strings.
    pub fn from_mmap(mmap: &[u8], address: u64) -> Result<Option<Self>, MdfError> {
        // 0 means “no SIBLOCK”
        if address == 0 {
            return Ok(None);
        }
        // read the raw block first
        let sb: SourceBlock = read_source_block(mmap, address)?;
        // now read each link as a String (may return None if addr == 0)
        let name: Option<String>    = read_string_block(mmap, sb.name_addr)?;
        let path: Option<String>    = read_string_block(mmap, sb.path_addr)?;
        let comment: Option<String> = read_string_block(mmap, sb.comment_addr)?;
        Ok(Some(SourceInfo { name, path, comment }))
    }
}
