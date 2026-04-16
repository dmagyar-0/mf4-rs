use crate::error::MdfError;
use crate::parsing::mdf_file::MdfFile;
use crate::api::channel_group::ChannelGroup;

#[derive(Debug)]
/// High level representation of an MDF file.
///
/// The struct stores the memory mapped file internally and lazily exposes
/// [`ChannelGroup`] wrappers for easy inspection.
pub struct MDF {
    raw: MdfFile,
}

impl MDF {
    /// Parse an MDF4 file from disk.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_bytes`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: &str) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_file(path)?;
        Ok(MDF { raw })
    }

    /// Parse an MDF4 file from an owned byte buffer.
    ///
    /// This is the primary entry point on `wasm32-unknown-unknown` where
    /// filesystem access is unavailable.  On native targets the caller can
    /// populate the buffer from `std::fs::read` or a memory-mapped file.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_bytes(data)?;
        Ok(MDF { raw })
    }

    /// Retrieve channel groups contained in the file.
    ///
    /// Each [`ChannelGroup`] is created lazily and does not decode any samples.
    pub fn channel_groups(&self) -> Vec<ChannelGroup<'_>> {
        let mut groups = Vec::new();

        for raw_data_group in &self.raw.data_groups {
            for raw_channel_group in &raw_data_group.channel_groups {
                groups.push(ChannelGroup::new(
                    raw_data_group,
                    raw_channel_group,
                    &self.raw.mmap,
                ));
            }
        }

        groups
    }

    /// Get the start time of the measurement in nanoseconds since epoch.
    ///
    /// This is the absolute timestamp stored in the MDF file header.
    /// Returns None if the start time is 0 (not set).
    pub fn start_time_ns(&self) -> Option<u64> {
        let time = self.raw.header.abs_time;
        if time == 0 {
            None
        } else {
            Some(time)
        }
    }
}
