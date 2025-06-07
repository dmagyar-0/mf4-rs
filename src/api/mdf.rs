use crate::error::MdfError;
use crate::parsing::mdf_file::MdfFile;
use crate::api::channel_group::ChannelGroup;

#[derive(Debug)]
pub struct MDF {
    raw: MdfFile,
}

impl MDF {
    /// Parse and hold the raw MDF4 file (with mmap, DataGroup & ChannelGroup blocks).
    pub fn from_file(path: &str) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_file(path)?;
        Ok(MDF { raw })
    }

    /// One `ChannelGroup<'_>` per RawChannelGroup, all lazy.
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
}
