// This example shows how to implement HTTP range reading for production use
// Run with: cargo add reqwest --features blocking,json
// Then: cargo run --example http_range_reader_example

use mf4_rs::index::{MdfIndex, ByteRangeReader};
use mf4_rs::error::MdfError;

/// HTTP Range Reader implementation
/// This would be used in production to read MDF files over HTTP
pub struct HttpRangeReader {
    // In real implementation you'd use reqwest or similar
    url: String,
    // Placeholder for demonstration
    _phantom: std::marker::PhantomData<()>,
}

impl HttpRangeReader {
    pub fn new(url: String) -> Self {
        Self {
            url,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl ByteRangeReader for HttpRangeReader {
    type Error = MdfError;
    
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        println!("HTTP Range Request: GET {} with Range: bytes={}-{}", 
                 self.url, offset, offset + length - 1);
        
        // In production, this would be:
        /*
        let client = reqwest::blocking::Client::new();
        let range_header = format!("bytes={}-{}", offset, offset + length - 1);
        
        let response = client
            .get(&self.url)
            .header("Range", range_header)
            .send()
            .map_err(|e| MdfError::BlockSerializationError(format!("HTTP error: {}", e)))?;
        
        if !response.status().is_success() && response.status() != 206 {
            return Err(MdfError::BlockSerializationError(
                format!("HTTP error: {} - {}", response.status(), response.status().canonical_reason().unwrap_or("Unknown"))
            ));
        }
        
        let bytes = response.bytes()
            .map_err(|e| MdfError::BlockSerializationError(format!("Response error: {}", e)))?;
        
        Ok(bytes.to_vec())
        */
        
        // For this demo, simulate reading from local file
        use mf4_rs::index::FileRangeReader;
        let mut file_reader = FileRangeReader::new("sample_for_indexing.mf4")
            .map_err(|_| MdfError::BlockSerializationError("Demo file not found - run mdf_index_example first".to_string()))?;
        
        println!("  -> Simulated: reading {} bytes from offset {}", length, offset);
        file_reader.read_range(offset, length)
    }
}

fn main() -> Result<(), MdfError> {
    println!("=== HTTP Range Reader Example ===");
    println!("NOTE: Run 'cargo run --example mdf_index_example' first to create test files");
    
    // Load an existing index (created by mdf_index_example)
    let index_path = "sample_index.json";
    let index = match MdfIndex::load_from_file(index_path) {
        Ok(idx) => idx,
        Err(_) => {
            println!("Error: Could not load index file. Please run 'mdf_index_example' first.");
            return Ok(());
        }
    };
    
    println!("Loaded index with {} channel groups", index.channel_groups.len());
    
    // Create HTTP range reader
    let mut http_reader = HttpRangeReader::new("https://example.com/data.mf4".to_string());
    
    if let Some(channels) = index.list_channels(0) {
        println!("\\nAvailable channels:");
        for (idx, name, data_type) in &channels {
            println!("  Channel {}: {} ({:?})", idx, name, data_type);
        }
        
        if !channels.is_empty() {
            let channel_idx = 0;
            let (_, channel_name, _) = &channels[channel_idx];
            
            println!("\\n=== Reading {} channel via HTTP ranges ===", channel_name);
            
            // Get byte ranges first
            let ranges = index.get_channel_byte_ranges(0, channel_idx)?;
            println!("Channel needs {} byte ranges:", ranges.len());
            for (i, (offset, length)) in ranges.iter().enumerate() {
                println!("  Range {}: {} bytes from offset {}", i, length, offset);
            }
            
            // Read the channel data using HTTP ranges
            match index.read_channel_values(0, channel_idx, &mut http_reader) {
                Ok(values) => {
                    println!("\\nSuccessfully read {} values via HTTP range requests:", values.len());
                    
                    // Show first few values
                    for (i, value) in values.iter().take(5).enumerate() {
                        println!("  Value {}: {:?}", i, value);
                    }
                    if values.len() > 5 {
                        println!("  ... ({} more values)", values.len() - 5);
                    }
                }
                Err(e) => {
                    println!("Error reading channel values: {}", e);
                }
            }
            
            // Demonstrate partial record reading
            println!("\\n=== Partial Record Reading (records 10-19) ===");
            let partial_ranges = index.get_channel_byte_ranges_for_records(0, channel_idx, 10, 10)?;
            println!("Partial read needs {} byte ranges:", partial_ranges.len());
            for (i, (offset, length)) in partial_ranges.iter().enumerate() {
                println!("  Range {}: {} bytes from offset {} (HTTP: bytes={}-{})", 
                         i, length, offset, offset, offset + length - 1);
            }
            
            let total_bytes: u64 = partial_ranges.iter().map(|(_, len)| len).sum();
            let full_total: u64 = ranges.iter().map(|(_, len)| len).sum();
            let savings = ((full_total - total_bytes) as f64 / full_total as f64) * 100.0;
            
            println!("Savings: {} bytes ({:.1}% less than reading all data)", 
                     full_total - total_bytes, savings);
        }
    }
    
    println!("\\n=== Production Usage Pattern ===");
    println!("1. Create index once: MdfIndex::from_file('local_file.mf4')");
    println!("2. Save index: index.save_to_file('metadata.json')");
    println!("3. Deploy index file to your application");
    println!("4. In production:");
    println!("   - Load index: MdfIndex::load_from_file('metadata.json')");
    println!("   - Create HTTP reader: HttpRangeReader::new(file_url)");
    println!("   - Read specific channels: index.read_channel_values(g, c, &mut reader)");
    println!("   - Or get byte ranges: index.get_channel_byte_ranges(g, c)");
    println!("5. Make HTTP Range requests only for the exact bytes you need!");
    
    Ok(())
}