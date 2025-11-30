#![allow(dead_code)]
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use std::io::{self, Read, Write};

// Function to compress JSON data
pub fn compress_json(json_data: &Value) -> io::Result<Vec<u8>> {
    // Convert JSON data to string
    let json_str = serde_json::to_string(json_data).unwrap();

    // Compress JSON data to GZ
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json_str.as_bytes())?;
    let compressed_data = encoder.finish()?;

    Ok(compressed_data)
}

// Function to decompress JSON data
pub fn decompress_json(compressed_data: &[u8]) -> io::Result<Value> {
    // Decompress JSON data from GZ
    let mut decoder = GzDecoder::new(compressed_data);
    let mut decompressed_data = Vec::new();
    decoder.read_to_end(&mut decompressed_data)?;
    // Convert decompressed data back to JSON string
    let decompressed_json_str = String::from_utf8(decompressed_data).unwrap();
    let decompressed_json: Value = serde_json::from_str(&decompressed_json_str).unwrap();

    Ok(decompressed_json)
}

// fn main() -> io::Result<()> {
//     // Sample JSON data
//     let json_data = serde_json::json!({
//         "name": "John Doe",
//         "age": 30,
//         "is_student": false,
//         "courses": ["Rust", "Web3", "Blockchain"]
//     });
//
//     // Convert JSON data to string
//     let json_str = serde_json::to_string(&json_data).unwrap();
//     let original_len = json_str.len();
//     println!("Original data length: {}", original_len);
//
//     // Compress JSON data
//     let compressed_data = compress_json(&json_data)?;
//     let compressed_len = compressed_data.len();
//     println!("Compressed data length: {}", compressed_len);
//
//     // Calculate compression ratio as a float
//     let compression_ratio = original_len as f64 / compressed_len as f64;
//     println!("Compression ratio: {:.2}", compression_ratio);
//
//     // Decompress JSON data
//     let decompressed_json = decompress_json(&compressed_data)?;
//     println!("Decompressed JSON: {}", decompressed_json);
//
//     // Compare original and decompressed JSON
//     assert_eq!(json_data, decompressed_json);
//     println!("Original and decompressed JSON are equal.");
//
//     Ok(())
// }
