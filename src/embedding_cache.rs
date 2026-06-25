//
// Copyright 2025-2026 Hans W. Uhlig. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

//! External compressed embedding cache for VDS 2.0.
//!
//! Embeddings are expensive to compute and model-specific. This cache stores ALL
//! embeddings for a workspace in a single compressed binary file using:
//! - `postcard` for efficient binary serialization
//! - `zstd` for compression (better ratio + faster than gzip)
//! - Built-in CRC checksums via postcard's `use-crc` feature
//!
//! Cache location: `{platform_cache_dir}/vds/workspaces/{workspace_id}/embeddings.zst`

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::document::{SectionId, TextEmbedding};

/// All cached embeddings for one workspace, keyed by (section_id, content_hash, model).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddingCacheData {
    /// Format version for future migrations.
    pub version: u32,
    /// Workspace ID this cache belongs to.
    pub workspace_id: String,
    /// All cached embeddings.
    pub embeddings: BTreeMap<CacheKey, CachedEmbedding>,
    /// Last update timestamp.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Cache key uniquely identifies one embedding.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    pub section_id: SectionId,
    pub content_hash: String,
    pub model: String,
}

/// One cached embedding with metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedEmbedding {
    /// The embedding vector (typically 384-1536 floats).
    pub vector: Vec<f32>,
    /// When this was cached.
    pub cached_at: chrono::DateTime<chrono::Utc>,
}

/// External compressed embedding cache for one workspace.
pub struct EmbeddingCache {
    cache_file: PathBuf,
    workspace_id: String,
}

impl EmbeddingCache {
    /// Opens or creates an embedding cache for the given workspace.
    ///
    /// Cache location is platform-specific:
    /// - macOS: `~/Library/Caches/vds/workspaces/{workspace_id}/embeddings.zst`
    /// - Linux: `~/.cache/vds/workspaces/{workspace_id}/embeddings.zst`
    /// - Windows: `%LOCALAPPDATA%/vds/workspaces/{workspace_id}/embeddings.zst`
    pub fn open(workspace_root: &Path) -> Result<Self, CacheError> {
        // Read workspace.json to get workspace_id
        let workspace_json = workspace_root.join(".vds/workspace.json");
        let workspace_manifest: serde_json::Value = if workspace_json.exists() {
            let content = fs::read_to_string(&workspace_json)
                .map_err(|e| CacheError::Io(workspace_json.clone(), e))?;
            serde_json::from_str(&content)
                .map_err(|e| CacheError::Json(workspace_json.clone(), e))?
        } else {
            return Err(CacheError::NoWorkspaceManifest(workspace_json));
        };

        let workspace_id = workspace_manifest["workspace_id"]
            .as_str()
            .ok_or_else(|| CacheError::MissingWorkspaceId)?
            .to_owned();

        let cache_dir = platform_cache_dir()?
            .join("vds")
            .join("workspaces")
            .join(&workspace_id);

        fs::create_dir_all(&cache_dir)
            .map_err(|e| CacheError::Io(cache_dir.clone(), e))?;

        let cache_file = cache_dir.join("embeddings.zst");

        Ok(Self {
            cache_file,
            workspace_id,
        })
    }

    /// Loads all cached embeddings from disk.
    fn load_data(&self) -> Result<EmbeddingCacheData, CacheError> {
        if !self.cache_file.exists() {
            return Ok(EmbeddingCacheData {
                version: 1,
                workspace_id: self.workspace_id.clone(),
                embeddings: BTreeMap::new(),
                updated_at: chrono::Utc::now(),
            });
        }

        let compressed = fs::read(&self.cache_file)
            .map_err(|e| CacheError::Io(self.cache_file.clone(), e))?;

        // Decompress
        let decompressed = zstd::decode_all(&compressed[..])
            .map_err(|e| CacheError::Decompress(self.cache_file.clone(), e))?;

        // Deserialize with CRC verification (postcard automatically verifies)
        let data: EmbeddingCacheData = postcard::from_bytes(&decompressed)
            .map_err(|e| CacheError::Deserialize(self.cache_file.clone(), e))?;

        if data.workspace_id != self.workspace_id {
            return Err(CacheError::WorkspaceIdMismatch {
                expected: self.workspace_id.clone(),
                actual: data.workspace_id,
            });
        }

        Ok(data)
    }

    /// Saves all cached embeddings to disk.
    fn save_data(&self, data: &EmbeddingCacheData) -> Result<(), CacheError> {
        // Ensure parent directory exists (may have been removed by parallel test cleanup)
        if let Some(parent) = self.cache_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CacheError::Io(parent.to_path_buf(), e))?;
        }

        // Serialize with CRC (postcard automatically adds checksum)
        let serialized = postcard::to_stdvec(data)
            .map_err(|e| CacheError::Serialize(self.cache_file.clone(), e))?;

        // Compress (level 3 = fast, good ratio)
        let compressed = zstd::encode_all(&serialized[..], 3)
            .map_err(|e| CacheError::Compress(self.cache_file.clone(), e))?;

        // Atomic write via temp file
        let temp_file = self.cache_file.with_extension("zst.tmp");
        fs::write(&temp_file, compressed)
            .map_err(|e| CacheError::Io(temp_file.clone(), e))?;
        fs::rename(&temp_file, &self.cache_file)
            .map_err(|e| CacheError::Io(self.cache_file.clone(), e))?;

        Ok(())
    }

    /// Gets a cached embedding if it exists and matches the content hash.
    pub fn get(
        &self,
        section_id: &SectionId,
        content_hash: &str,
        model: &str,
    ) -> Result<Option<TextEmbedding>, CacheError> {
        let data = self.load_data()?;
        let key = CacheKey {
            section_id: section_id.clone(),
            content_hash: content_hash.to_owned(),
            model: model.to_owned(),
        };

        Ok(data.embeddings.get(&key).map(|cached| TextEmbedding {
            model: Some(model.to_owned()),
            vector: cached.vector.clone(),
        }))
    }

    /// Saves an embedding to the cache.
    pub fn put(
        &self,
        section_id: &SectionId,
        content_hash: &str,
        embedding: &TextEmbedding,
    ) -> Result<(), CacheError> {
        let mut data = self.load_data()?;
        let model = embedding.model.as_deref().unwrap_or("unknown");
        let key = CacheKey {
            section_id: section_id.clone(),
            content_hash: content_hash.to_owned(),
            model: model.to_owned(),
        };

        data.embeddings.insert(
            key,
            CachedEmbedding {
                vector: embedding.vector.clone(),
                cached_at: chrono::Utc::now(),
            },
        );
        data.updated_at = chrono::Utc::now();

        self.save_data(&data)
    }

    /// Batch-puts multiple embeddings (more efficient than individual puts).
    pub fn put_batch(
        &self,
        embeddings: &[(SectionId, String, TextEmbedding)],
    ) -> Result<(), CacheError> {
        let mut data = self.load_data()?;

        for (section_id, content_hash, embedding) in embeddings {
            let model = embedding.model.as_deref().unwrap_or("unknown");
            let key = CacheKey {
                section_id: section_id.clone(),
                content_hash: content_hash.clone(),
                model: model.to_owned(),
            };

            data.embeddings.insert(
                key,
                CachedEmbedding {
                    vector: embedding.vector.clone(),
                    cached_at: chrono::Utc::now(),
                },
            );
        }

        data.updated_at = chrono::Utc::now();
        self.save_data(&data)
    }

    /// Removes all cached embeddings for a section (all models/content hashes).
    pub fn remove_section(&self, section_id: &SectionId) -> Result<(), CacheError> {
        let mut data = self.load_data()?;
        data.embeddings
            .retain(|key, _| &key.section_id != section_id);
        data.updated_at = chrono::Utc::now();
        self.save_data(&data)
    }

    /// Loads all cached embeddings into a map.
    pub fn load_all(&self) -> Result<BTreeMap<CacheKey, TextEmbedding>, CacheError> {
        let data = self.load_data()?;
        Ok(data
            .embeddings
            .into_iter()
            .map(|(key, cached)| {
                let embedding = TextEmbedding {
                    model: Some(key.model.clone()),
                    vector: cached.vector,
                };
                (key, embedding)
            })
            .collect())
    }

    /// Returns cache statistics.
    pub fn stats(&self) -> Result<CacheStats, CacheError> {
        let data = self.load_data()?;
        let file_size = if self.cache_file.exists() {
            fs::metadata(&self.cache_file)
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };

        let models: std::collections::BTreeSet<String> =
            data.embeddings.keys().map(|k| k.model.clone()).collect();

        // Calculate uncompressed size
        let uncompressed_bytes: usize = data
            .embeddings
            .values()
            .map(|e| e.vector.len() * 4) // f32 = 4 bytes
            .sum();

        Ok(CacheStats {
            entry_count: data.embeddings.len(),
            model_count: models.len(),
            models: models.into_iter().collect(),
            uncompressed_bytes: uncompressed_bytes as u64,
            compressed_bytes: file_size,
            compression_ratio: if file_size > 0 {
                uncompressed_bytes as f64 / file_size as f64
            } else {
                0.0
            },
            updated_at: data.updated_at,
        })
    }

    /// Clears all cached embeddings.
    pub fn clear(&self) -> Result<(), CacheError> {
        if self.cache_file.exists() {
            fs::remove_file(&self.cache_file)
                .map_err(|e| CacheError::Io(self.cache_file.clone(), e))?;
        }
        Ok(())
    }

    /// Returns the cache file path.
    pub fn cache_path(&self) -> &Path {
        &self.cache_file
    }
}

/// Cache statistics for monitoring and debugging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheStats {
    pub entry_count: usize,
    pub model_count: usize,
    pub models: Vec<String>,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    pub compression_ratio: f64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn platform_cache_dir() -> Result<PathBuf, CacheError> {
    let dir = if cfg!(target_os = "macos") {
        dirs::home_dir()
            .ok_or(CacheError::NoCacheDir)?
            .join("Library/Caches")
    } else if cfg!(target_os = "windows") {
        dirs::cache_dir().ok_or(CacheError::NoCacheDir)?
    } else {
        // Linux and others
        dirs::cache_dir().ok_or(CacheError::NoCacheDir)?
    };
    Ok(dir)
}

/// Errors that can occur when working with the embedding cache.
#[derive(Debug)]
pub enum CacheError {
    Io(PathBuf, io::Error),
    Json(PathBuf, serde_json::Error),
    Compress(PathBuf, io::Error),
    Decompress(PathBuf, io::Error),
    Serialize(PathBuf, postcard::Error),
    Deserialize(PathBuf, postcard::Error),
    NoCacheDir,
    NoWorkspaceManifest(PathBuf),
    MissingWorkspaceId,
    WorkspaceIdMismatch { expected: String, actual: String },
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "{}: {}", path.display(), e),
            Self::Json(path, e) => write!(f, "{}: JSON error: {}", path.display(), e),
            Self::Compress(path, e) => write!(f, "{}: compression error: {}", path.display(), e),
            Self::Decompress(path, e) => {
                write!(f, "{}: decompression error: {}", path.display(), e)
            }
            Self::Serialize(path, e) => {
                write!(f, "{}: serialization error: {}", path.display(), e)
            }
            Self::Deserialize(path, e) => {
                write!(f, "{}: deserialization error: {}", path.display(), e)
            }
            Self::NoCacheDir => write!(f, "platform cache directory not found"),
            Self::NoWorkspaceManifest(path) => {
                write!(f, "workspace manifest not found: {}", path.display())
            }
            Self::MissingWorkspaceId => write!(f, "workspace_id not found in manifest"),
            Self::WorkspaceIdMismatch { expected, actual } => write!(
                f,
                "workspace ID mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, e) | Self::Compress(_, e) | Self::Decompress(_, e) => Some(e),
            Self::Json(_, e) => Some(e),
            Self::Serialize(_, e) | Self::Deserialize(_, e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestWorkspace {
        root: PathBuf,
        workspace_id: String,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let workspace_id = format!("test-{}", nonce);
            let root = std::env::temp_dir().join(format!("vds-embedding-cache-{nonce}"));
            fs::create_dir_all(&root).unwrap();

            // Create a workspace manifest
            fs::create_dir_all(root.join(".vds")).unwrap();
            let manifest = serde_json::json!({
                "format_version": 1,
                "workspace_id": workspace_id,
                "created_at": "2026-06-24T00:00:00Z"
            });
            fs::write(
                root.join(".vds/workspace.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .unwrap();

            Self { root, workspace_id }
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            // Clean up workspace
            let _ = fs::remove_dir_all(&self.root);
            // Clean up cache - construct path directly instead of opening cache
            if let Ok(cache_dir) = platform_cache_dir() {
                let workspace_cache = cache_dir
                    .join("vds")
                    .join("workspaces")
                    .join(&self.workspace_id);
                let _ = fs::remove_dir_all(&workspace_cache);
            }
        }
    }

    #[test]
    fn opens_cache_for_workspace() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();
        assert!(cache.cache_file.to_string_lossy().contains(&workspace.workspace_id));
    }

    #[test]
    fn caches_and_retrieves_embedding() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section_id = SectionId::new("test-section");
        let content_hash = "sha256:abc123";
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: vec![0.1, 0.2, 0.3, 0.4],
        };

        cache.put(&section_id, content_hash, &embedding).unwrap();

        let retrieved = cache
            .get(&section_id, content_hash, "test-model")
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.vector, embedding.vector);
        assert_eq!(retrieved.model, embedding.model);
    }

    #[test]
    fn returns_none_for_hash_mismatch() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section_id = SectionId::new("test-section");
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: vec![0.1, 0.2, 0.3],
        };

        cache
            .put(&section_id, "sha256:abc", &embedding)
            .unwrap();

        let result = cache
            .get(&section_id, "sha256:different", "test-model")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn compresses_embeddings_efficiently() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section_id = SectionId::new("test-section");
        let content_hash = "sha256:abc123";
        // Realistic embedding: 768 floats (typical BERT size)
        let vector: Vec<f32> = (0..768).map(|i| (i as f32) * 0.001).collect();
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector,
        };

        cache.put(&section_id, content_hash, &embedding).unwrap();

        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 1);
        // Uncompressed: 768 floats * 4 bytes = 3072 bytes
        assert_eq!(stats.uncompressed_bytes, 3072);
        // Compressed should be smaller (postcard + zstd overhead means ratio varies)
        assert!(stats.compressed_bytes < stats.uncompressed_bytes);
        assert!(stats.compression_ratio > 1.0, "compression_ratio = {}", stats.compression_ratio);
        println!(
            "Compression: {:.2}:1 ({} → {} bytes)",
            stats.compression_ratio, stats.uncompressed_bytes, stats.compressed_bytes
        );
    }

    #[test]
    fn batch_puts_multiple_embeddings() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let embeddings = vec![
            (
                SectionId::new("section-1"),
                "sha256:a".to_owned(),
                TextEmbedding {
                    model: Some("model-1".to_owned()),
                    vector: vec![0.1, 0.2],
                },
            ),
            (
                SectionId::new("section-2"),
                "sha256:b".to_owned(),
                TextEmbedding {
                    model: Some("model-1".to_owned()),
                    vector: vec![0.3, 0.4],
                },
            ),
        ];

        cache.put_batch(&embeddings).unwrap();

        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 2);
    }

    #[test]
    fn removes_section_embeddings() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section_id = SectionId::new("test-section");
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: vec![0.1, 0.2],
        };

        cache.put(&section_id, "sha256:a", &embedding).unwrap();
        cache.put(&section_id, "sha256:b", &embedding).unwrap();

        assert_eq!(cache.stats().unwrap().entry_count, 2);

        cache.remove_section(&section_id).unwrap();

        assert_eq!(cache.stats().unwrap().entry_count, 0);
    }

    #[test]
    fn loads_all_embeddings() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section1 = SectionId::new("section-1");
        let section2 = SectionId::new("section-2");
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: vec![0.1, 0.2],
        };

        cache.put(&section1, "sha256:a", &embedding).unwrap();
        cache.put(&section2, "sha256:b", &embedding).unwrap();

        let all = cache.load_all().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn verifies_crc_on_load() {
        let workspace = TestWorkspace::new();
        let cache = EmbeddingCache::open(&workspace.root).unwrap();

        let section_id = SectionId::new("test-section");
        let embedding = TextEmbedding {
            model: Some("test-model".to_owned()),
            vector: vec![0.1, 0.2, 0.3],
        };

        cache.put(&section_id, "sha256:abc", &embedding).unwrap();

        // Corrupt the cache file
        let corrupted = vec![0u8; 100]; // Invalid data
        fs::write(&cache.cache_file, corrupted).unwrap();

        // Should fail to load with deserialization error
        let result = cache.get(&section_id, "sha256:abc", "test-model");
        assert!(result.is_err());
    }
}
