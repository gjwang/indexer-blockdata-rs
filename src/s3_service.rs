use std::error::Error;
use std::fs::File;
use std::io::{Write};
use std::path::Path;

use bytes::Bytes;
use s3::{Bucket, Region};
use s3::creds::Credentials;

pub struct S3Service {
    bucket: Bucket,
}

impl S3Service {
    pub fn new(
        bucket_name: &str,
        region: &str,
        endpoint: &str,
        aws_access_key_id: &str,
        aws_secret_access_key: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let region = Region::Custom {
            region: region.to_owned(),
            endpoint: endpoint.to_owned(),
        };

        let credentials = Credentials::new(
            Some(&aws_access_key_id),
            Some(&aws_secret_access_key),
            None,
            None,
            None,
        )?;

        // Create a bucket object
        let bucket = Bucket::new(bucket_name, region, credentials)?.with_path_style(); // Use path-style for MinIO

        Ok(S3Service { bucket })
    }

    pub async fn upload_object(
        &self,
        key: &str,
        data: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.bucket.put_object(key, &data).await?;
        if response.status_code() == 200 {
            println!("File uploaded successfully!");
        } else {
            println!(
                "Failed to upload file. Status code: {}",
                response.status_code()
            );
        }

        Ok(())
    }

    pub async fn upload_file(&self, file_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let file_name = file_path.file_name().unwrap().to_str().unwrap();
        let content = std::fs::read(file_path)?;
        self.upload_object(file_name, content).await?;

        Ok(())
    }

    pub async fn get_object(
        &self,
        object_key: &str,
    ) -> Result<Bytes, Box<dyn std::error::Error>> {
        let response = self.bucket.get_object(object_key).await?;

        if response.status_code() == 200 {
            println!("get_object {object_key} successfully");
            let data = response.bytes();
            Ok(data.clone())
        } else {
            println!(
                "Failed to get_object:{object_key} Status code: {}",
                response.status_code()
            );
            Ok(Bytes::new())
        }
    }

    pub async fn download_object_to_file(
        &self,
        object_key: &str,
        download_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let data = self.get_object(object_key).await?;
        let mut file = File::create(download_path)?;
        file.write_all(&data)?;
        println!("File downloaded successfully to {:?}", download_path);
        Ok(())
    }
}
