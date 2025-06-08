pub mod blocks;
pub mod error;
pub mod writer;
pub mod cut;

pub mod merge;

pub mod parsing {
    pub mod decoder;
    pub mod mdf_file;
    pub mod raw_channel_group;
    pub mod raw_data_group;
    pub mod raw_channel;
    pub mod source_info;
}

pub mod api {
    pub mod mdf;
    pub mod channel_group;
    pub mod channel;
}