use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use bytes::Bytes;
use std::path::{Component, Path, PathBuf};
use tracing::info;

/// Join an object key onto the local storage root, refusing any key that would
/// escape it (path traversal, absolute path, parent refs, NUL, Windows paths).
/// Object keys are server-constructed today (dataset-id slug + UUID + sanitized
/// filename), but this is a defense-in-depth containment check so a future caller
/// bug cannot read or write outside the asset store.
fn safe_local_join(base_path: &Path, key: &str) -> anyhow::Result<PathBuf> {
    if key.is_empty()
        || key.starts_with('/')
        || key.starts_with('\\')
        || key.contains("..")
        || key.contains('\0')
        || key.contains(":\\")
    {
        anyhow::bail!("invalid object key");
    }
    let path = base_path.join(key);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        anyhow::bail!("invalid object key");
    }
    if !path.starts_with(base_path) {
        anyhow::bail!("object key escapes storage root");
    }
    Ok(path)
}

enum Backend {
    S3 { client: S3Client, bucket: String },
    Local { base_path: PathBuf },
}

/// Asset storage — either S3/MinIO or local filesystem.
pub struct ObjectStore {
    backend: Backend,
}

impl Clone for ObjectStore {
    fn clone(&self) -> Self {
        match &self.backend {
            Backend::S3 { client, bucket } => Self {
                backend: Backend::S3 {
                    client: client.clone(),
                    bucket: bucket.clone(),
                },
            },
            Backend::Local { base_path } => Self {
                backend: Backend::Local {
                    base_path: base_path.clone(),
                },
            },
        }
    }
}

impl ObjectStore {
    /// Connect to an S3-compatible endpoint.
    pub async fn new(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> anyhow::Result<Self> {
        let creds = Credentials::new(access_key, secret_key, None, None, "env");
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .endpoint_url(endpoint)
            .region(Region::new(region.to_string()))
            .credentials_provider(creds)
            .force_path_style(true)
            .build();

        let client = S3Client::from_conf(config);

        // Ensure bucket exists
        match client.head_bucket().bucket(bucket).send().await {
            Ok(_) => {}
            Err(_) => {
                client
                    .create_bucket()
                    .bucket(bucket)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create bucket: {}", e))?;
                info!("Created S3 bucket: {}", bucket);
            }
        }

        Ok(Self {
            backend: Backend::S3 {
                client,
                bucket: bucket.to_string(),
            },
        })
    }

    /// Use a local directory as asset storage (no external service required).
    pub fn local(base_path: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&base_path)?;
        Ok(Self {
            backend: Backend::Local { base_path },
        })
    }

    /// No-op store (for tests — all operations return errors).
    pub fn noop() -> Self {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .build();
        Self {
            backend: Backend::S3 {
                client: S3Client::from_conf(config),
                bucket: String::new(),
            },
        }
    }

    pub fn is_configured(&self) -> bool {
        match &self.backend {
            Backend::S3 { bucket, .. } => !bucket.is_empty(),
            Backend::Local { .. } => true,
        }
    }

    /// Upload an asset.
    pub async fn upload(&self, key: &str, body: Bytes, content_type: &str) -> anyhow::Result<()> {
        match &self.backend {
            Backend::S3 { client, bucket } => {
                if bucket.is_empty() {
                    return Err(anyhow::anyhow!("S3 storage is not configured"));
                }
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(ByteStream::from(body))
                    .content_type(content_type)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 upload failed: {}", e))?;
            }
            Backend::Local { base_path } => {
                let path = safe_local_join(base_path, key)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &body)?;
                // Persist content-type as a sidecar so download can return it
                let ct_path = path.with_extension("ct");
                let _ = std::fs::write(&ct_path, content_type);
            }
        }
        Ok(())
    }

    /// Download an asset. Returns (bytes, content_type).
    pub async fn download(&self, key: &str) -> anyhow::Result<(Bytes, String)> {
        match &self.backend {
            Backend::S3 { client, bucket } => {
                if bucket.is_empty() {
                    return Err(anyhow::anyhow!("S3 storage is not configured"));
                }
                let resp = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 download failed: {}", e))?;
                let content_type = resp
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let body = resp
                    .body
                    .collect()
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 read failed: {}", e))?;
                Ok((body.into_bytes(), content_type))
            }
            Backend::Local { base_path } => {
                let path = safe_local_join(base_path, key)?;
                let data = std::fs::read(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to read asset {:?}: {}", path, e))?;
                // Content-type stored as a sidecar file
                let ct_path = path.with_extension("ct");
                let content_type = std::fs::read_to_string(&ct_path)
                    .unwrap_or_else(|_| "application/octet-stream".to_string());
                Ok((Bytes::from(data), content_type))
            }
        }
    }

    /// Delete an asset.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        match &self.backend {
            Backend::S3 { client, bucket } => {
                if bucket.is_empty() {
                    return Err(anyhow::anyhow!("S3 storage is not configured"));
                }
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 delete failed: {}", e))?;
            }
            Backend::Local { base_path } => {
                let path = safe_local_join(base_path, key)?;
                if path.exists() {
                    std::fs::remove_file(&path)?;
                }
                let ct_path = path.with_extension("ct");
                if ct_path.exists() {
                    let _ = std::fs::remove_file(&ct_path);
                }
            }
        }
        Ok(())
    }
}
