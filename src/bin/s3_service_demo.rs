use std::env;
use std::path::Path;

use dotenv::dotenv;
use s3::Bucket;
use s3::creds::Credentials;
use s3::Region;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // S3 or MinIO configuration
    let endpoint = "http://localhost:9000"; // Use this for MinIO, comment out for S3
    let bucket_name = "my-bucket2";
    let region = "us-east-1";

    let aws_access_key_id = env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY must be set");
    let aws_secret_access_key = env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_KEY must be set");

    // S3 or MinIO configuration
    let region = Region::Custom {
        region: region.to_owned(),
        endpoint: endpoint.to_owned(),
    };

    // Credentials
    let credentials = Credentials::new(
        Some(&aws_access_key_id),
        Some(&aws_secret_access_key),
        None,
        None,
        None,
    )?;

    // Create a bucket object
    let bucket = Bucket::new(bucket_name, region, credentials)?
        .with_path_style(); // Use path-style for MinIO


    // File to upload
    let filename = "0.jpeg";
    let file_path = Path::new(filename);
    let file_name = file_path.file_name().unwrap().to_str().unwrap();

    // Read the file content
    let content = std::fs::read(file_path)?;

    // Upload the file
    let response = bucket.put_object(file_name, &content).await?;

    if response.status_code() == 200 {
        println!("File uploaded successfully!");
    } else {
        println!("Failed to upload file. Status code: {}", response.status_code());
    }

    Ok(())
}