use wasm_bindgen::prelude::*;
use mf4_rs::api::mdf::MDF;
use serde::Serialize;

#[derive(Serialize)]
pub struct ChannelSummary {
    pub name: String,
    pub unit: Option<String>,
}

#[derive(Serialize)]
pub struct GroupSummary {
    pub group_index: usize,
    pub name: Option<String>,
    pub channel_count: usize,
    pub sample_count: u64,
    pub channels: Vec<ChannelSummary>,
}

#[derive(Serialize)]
pub struct FileSummary {
    pub start_time_ns: Option<u64>,
    pub group_count: usize,
    pub groups: Vec<GroupSummary>,
}

/// Parse an MF4 file from a `Uint8Array` and return channel names + sample counts.
///
/// Called from a Web Worker as:
/// ```js
/// const result = open_from_bytes(new Uint8Array(await blob.arrayBuffer()));
/// ```
#[wasm_bindgen]
pub fn open_from_bytes(data: &[u8]) -> Result<JsValue, JsValue> {
    let mdf = MDF::from_bytes(data.to_vec())
        .map_err(|e| JsValue::from_str(&format!("MDF parse error: {e}")))?;

    let start_time_ns = mdf.start_time_ns();
    let groups_vec = mdf.channel_groups();

    let mut groups = Vec::with_capacity(groups_vec.len());
    for (idx, group) in groups_vec.iter().enumerate() {
        let name = group.name()
            .unwrap_or(None);
        let channels_vec = group.channels();
        let sample_count = group.raw_channel_group().block.cycles_nr;

        let mut channels = Vec::with_capacity(channels_vec.len());
        for ch in &channels_vec {
            let name = ch.name().unwrap_or(None).unwrap_or_else(|| "<unnamed>".into());
            let unit = ch.unit().unwrap_or(None);
            channels.push(ChannelSummary { name, unit });
        }

        groups.push(GroupSummary {
            group_index: idx,
            name,
            channel_count: channels.len(),
            sample_count,
            channels,
        });
    }

    let summary = FileSummary {
        start_time_ns,
        group_count: groups.len(),
        groups,
    };

    serde_wasm_bindgen::to_value(&summary)
        .map_err(|e| JsValue::from_str(&format!("Serialization error: {e}")))
}
