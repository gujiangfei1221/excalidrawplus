//! Tencent Cloud Object Storage (COS) client.
//!
//! COS exposes an S3-compatible REST API, so this module wraps the
//! `aws-sdk-s3` crate rather than implementing request signing by hand.
//! The SDK is configured for Tencent's virtual-hosted endpoint format:
//!
//! ```text
//! https://{bucket}.cos.{region}.myqcloud.com
//! ```
//!
//! `force_path_style(false)` is required so the SDK does not rewrite
//! requests to use `https://endpoint/{bucket}/{key}` (path-style) — Tencent
//! COS expects the bucket to appear in the host portion of the URL.
//!
//! All credentials are taken from the user-supplied [`CosConfig`] and are
//! never persisted outside the local SQLite database.

use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::{Credentials, Region},
    primitives::ByteStream,
    Client,
};
use tracing::{debug, info, warn};

use crate::models::CosConfig;

/// Provider name reported to the AWS credential chain. Purely informational
/// — the SDK uses it in error messages and tracing spans.
const CREDENTIALS_PROVIDER_NAME: &str = "tencent-cos";

/// Lightweight metadata returned by [`CosClient::head_object`].
///
/// We intentionally surface only the fields the sync engine cares about so
/// callers do not have to take a dependency on the SDK's response types.
#[derive(Debug, Clone)]
pub struct ObjectMetadata {
    /// Size of the object in bytes, when reported by COS.
    pub content_length: Option<i64>,
    /// ETag of the object, with surrounding quote characters stripped.
    pub etag: Option<String>,
    /// Last-modified timestamp as Unix epoch milliseconds, when reported.
    pub last_modified_ms: Option<i64>,
}

/// S3-compatible client targeting a single Tencent COS bucket.
#[derive(Clone, Debug)]
pub struct CosClient {
    client: Client,
    bucket: String,
}

impl CosClient {
    /// Build a COS client from a user-supplied [`CosConfig`].
    ///
    /// The endpoint is constructed from the regional COS host and the SDK
    /// is allowed to apply the bucket as a virtual-hosted prefix.
    pub fn new(config: &CosConfig) -> Result<Self, String> {
        info!(
            bucket = %config.bucket,
            region = %config.region,
            "building COS client"
        );
        if config.bucket.trim().is_empty() {
            return Err("COS bucket is empty".to_string());
        }
        if config.region.trim().is_empty() {
            return Err("COS region is empty".to_string());
        }
        if config.secret_id.trim().is_empty() {
            return Err("COS secret_id is empty".to_string());
        }
        if config.secret_key.trim().is_empty() {
            return Err("COS secret_key is empty".to_string());
        }

        let endpoint = format!("https://cos.{region}.myqcloud.com", region = config.region);

        let credentials = Credentials::new(
            config.secret_id.clone(),
            config.secret_key.clone(),
            None,
            None,
            CREDENTIALS_PROVIDER_NAME,
        );

        let s3_config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .endpoint_url(endpoint)
            .credentials_provider(credentials)
            .force_path_style(false)
            .build();

        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket: config.bucket.clone(),
        })
    }

    /// The bucket this client is bound to.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Upload `body` to the bucket under `key`, replacing any existing
    /// object with the same key.
    pub async fn put_object(&self, key: &str, body: Vec<u8>) -> Result<(), String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body))
            .send()
            .await
            .map_err(|e| format!("COS put_object({key}) failed: {e}"))?;
        Ok(())
    }

    /// Download the object stored under `key` and return its raw bytes.
    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("COS get_object({key}) failed: {e}"))?;

        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| format!("COS get_object({key}) body read failed: {e}"))?
            .into_bytes()
            .to_vec();

        Ok(bytes)
    }

    /// Delete the object stored under `key`. The operation is idempotent on
    /// the COS side — deleting a non-existent key returns success.
    pub async fn delete_object(&self, key: &str) -> Result<(), String> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("COS delete_object({key}) failed: {e}"))?;
        Ok(())
    }

    /// Fetch metadata for the object stored under `key` without
    /// downloading its body.
    pub async fn head_object(&self, key: &str) -> Result<ObjectMetadata, String> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("COS head_object({key}) failed: {e}"))?;

        Ok(ObjectMetadata {
            content_length: resp.content_length(),
            etag: resp.e_tag().map(|s| s.trim_matches('"').to_string()),
            last_modified_ms: resp
                .last_modified()
                .and_then(|t| t.to_millis().ok()),
        })
    }

    /// Verify that the configured credentials can reach the bucket.
    ///
    /// We attempt a `list_objects_v2` with `max_keys=1` because Tencent COS
    /// historically did not implement `HeadBucket` consistently across
    /// regions, while listing one object exercises the same auth and
    /// network path with negligible overhead. A successful response
    /// returns `Ok(true)`; any error is surfaced to the caller so the
    /// configuration form can show a useful message.
    pub async fn test_connection(&self) -> Result<bool, String> {
        debug!(bucket = %self.bucket, "testing COS connection");
        self.client
            .list_objects_v2()
            .bucket(&self.bucket)
            .max_keys(1)
            .send()
            .await
            .map_err(|e| {
                warn!(bucket = %self.bucket, error = %e, "COS connection test failed");
                format!("COS test_connection failed: {e}")
            })?;
        info!(bucket = %self.bucket, "COS connection test succeeded");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CosConfig;

    /// Helper to create a valid CosConfig for testing.
    fn valid_config() -> CosConfig {
        CosConfig {
            secret_id: "AKIDxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".to_string(),
            secret_key: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".to_string(),
            bucket: "my-bucket-1250000000".to_string(),
            region: "ap-guangzhou".to_string(),
        }
    }

    #[test]
    fn regional_endpoint_does_not_include_bucket_name() {
        let config = valid_config();
        let endpoint = format!("https://cos.{region}.myqcloud.com", region = config.region);
        assert_eq!(endpoint, "https://cos.ap-guangzhou.myqcloud.com");
    }

    // --- Validation tests: empty fields should return Err ---

    #[test]
    fn new_with_empty_bucket_returns_err() {
        let mut config = valid_config();
        config.bucket = "".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS bucket is empty");
    }

    #[test]
    fn new_with_whitespace_only_bucket_returns_err() {
        let mut config = valid_config();
        config.bucket = "   ".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS bucket is empty");
    }

    #[test]
    fn new_with_empty_region_returns_err() {
        let mut config = valid_config();
        config.region = "".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS region is empty");
    }

    #[test]
    fn new_with_whitespace_only_region_returns_err() {
        let mut config = valid_config();
        config.region = "\t\n".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS region is empty");
    }

    #[test]
    fn new_with_empty_secret_id_returns_err() {
        let mut config = valid_config();
        config.secret_id = "".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS secret_id is empty");
    }

    #[test]
    fn new_with_whitespace_only_secret_id_returns_err() {
        let mut config = valid_config();
        config.secret_id = "  ".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS secret_id is empty");
    }

    #[test]
    fn new_with_empty_secret_key_returns_err() {
        let mut config = valid_config();
        config.secret_key = "".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS secret_key is empty");
    }

    #[test]
    fn new_with_whitespace_only_secret_key_returns_err() {
        let mut config = valid_config();
        config.secret_key = " \t ".to_string();
        let result = CosClient::new(&config);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "COS secret_key is empty");
    }

    // --- Valid construction tests ---

    #[test]
    fn new_with_valid_config_succeeds() {
        let config = valid_config();
        let result = CosClient::new(&config);
        assert!(result.is_ok());
    }

    // --- Accessor tests ---

    #[test]
    fn bucket_accessor_returns_correct_bucket_name() {
        let config = valid_config();
        let client = CosClient::new(&config).unwrap();
        assert_eq!(client.bucket(), "my-bucket-1250000000");
    }

    #[test]
    fn bucket_accessor_preserves_original_value() {
        let mut config = valid_config();
        config.bucket = "another-bucket-9876543210".to_string();
        let client = CosClient::new(&config).unwrap();
        assert_eq!(client.bucket(), "another-bucket-9876543210");
    }
}
