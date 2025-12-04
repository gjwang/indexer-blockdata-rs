use std::env;
use std::path::Path;

use dotenv::dotenv;

use fetcher::s3_service::S3Service;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // S3 or MinIO configuration
    let endpoint = "http://localhost:9000"; // Use this for MinIO, comment out for S3
    let bucket_name = "my-bucket2";
    let region = "us-east-1";

    let aws_access_key_id = env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID must be set");
    let aws_secret_access_key =
        env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY must be set");

    println!(
        "endpoint={endpoint}, bucket_name={bucket_name}, aws_access_key_id={aws_access_key_id}"
    );

    let s3_service =
        S3Service::new(bucket_name, region, endpoint, &aws_access_key_id, &aws_secret_access_key)?;

    // // File to upload
    let filename = "0.jpeg";
    let file_path = Path::new(filename);

    s3_service.upload_file(file_path).await?;
    s3_service.download_object_to_file(filename, file_path).await?;

    Ok(())
}
