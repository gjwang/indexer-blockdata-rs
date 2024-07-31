use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use dotenv::dotenv;
use s3::Bucket;
use s3::creds::Credentials;
use s3::Region;

async fn upload_object(bucket: &Bucket, file_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    let content = std::fs::read(file_path)?;

    let response = bucket.put_object(file_name, &content).await?;

    if response.status_code() == 200 {
        println!("File uploaded successfully!");
    } else {
        println!("Failed to upload file. Status code: {}", response.status_code());
    }

    Ok(())
}

async fn download_object(bucket: &Bucket, object_key: &str, download_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let response = bucket.get_object(object_key).await?;

    if response.status_code() == 200 {
        let data = response.bytes();
        let mut file = File::create(download_path)?;
        file.write_all(&data)?;
        println!("File downloaded successfully to {:?}", download_path);
    } else {
        println!("Failed to download file. Status code: {}", response.status_code());
    }

    Ok(())
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // S3 or MinIO configuration
    let endpoint = "http://localhost:9000"; // Use this for MinIO, comment out for S3
    let bucket_name = "my-bucket2";
    let region = "us-east-1";

    let aws_access_key_id = env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID must be set");
    let aws_secret_access_key = env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY must be set");

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

    // Upload example
    let upload_path = Path::new(file_path);
    upload_object(&bucket, &upload_path).await?;

    // Download example
    let object_key = "0.jpeg";  // The name of the file in S3/MinIO
    let download_path = Path::new(file_path);
    download_object(&bucket, object_key, &download_path).await?;


    Ok(())
}