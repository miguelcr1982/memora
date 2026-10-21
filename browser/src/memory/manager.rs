use std::{
    collections::HashMap,
    error::Error,
};

use super::resource::MemoryResource;

const BLOCK_SIZE: usize = 64 * 1024;
const WARM_AFTER_TICKS: u64 = 50;
const COLD_AFTER_TICKS: u64 = 200;
const AGING_INTERVAL_TICKS: u64 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    Ram,
    Lz4,
    Zstd { level: i32 },
}

impl Default for MemoryMode {
    fn default() -> Self {
        Self::Lz4
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceCategory {
    Html,
    JavaScript,
    Css,
    Json,
    Image,
    Font,
    Wasm,
    Audio,
    Video,
    Other,
}

impl ResourceCategory {
    fn from_mime(mime: &str) -> Self {
        let mime = mime.to_ascii_lowercase();

        if mime.contains("text/html") {
            Self::Html
        } else if mime.contains("javascript")
            || mime.contains("ecmascript")
        {
            Self::JavaScript
        } else if mime.contains("text/css") {
            Self::Css
        } else if mime.contains("json") {
            Self::Json
        } else if mime.starts_with("image/") {
            Self::Image
        } else if mime.starts_with("audio/") {
            Self::Audio
        } else if mime.starts_with("video/") {
            Self::Video
        } else if mime.contains("font")
            || mime.contains("woff")
            || mime.contains("truetype")
            || mime.contains("opentype")
        {
            Self::Font
        } else if mime.contains("wasm") {
            Self::Wasm
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Temperature {
    Hot,
    Warm,
    Cold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCodec {
    Raw,
    Lz4,
    Zstd,
}

#[derive(Debug)]
enum StoredData {
    Raw(Vec<u8>),

    Lz4 {
        data: Vec<u8>,
        original_size: usize,
    },

    Zstd {
        data: Vec<u8>,
        original_size: usize,
    },
}

impl StoredData {
    fn codec(&self) -> StorageCodec {
        match self {
            Self::Raw(_) => StorageCodec::Raw,
            Self::Lz4 { .. } => StorageCodec::Lz4,
            Self::Zstd { .. } => StorageCodec::Zstd,
        }
    }

    fn stored_size(&self) -> usize {
        match self {
            Self::Raw(data) => data.len(),
            Self::Lz4 { data, .. } => data.len(),
            Self::Zstd { data, .. } => data.len(),
        }
    }

    fn original_size(&self) -> usize {
        match self {
            Self::Raw(data) => data.len(),

            Self::Lz4 {
                original_size,
                ..
            }
            | Self::Zstd {
                original_size,
                ..
            } => *original_size,
        }
    }

    fn restore(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        match self {
            Self::Raw(data) => Ok(data.clone()),

            Self::Lz4 {
                data,
                original_size,
            } => {
                let restored =
                    lz4_flex::decompress_size_prepended(data)?;

                if restored.len() != *original_size {
                    return Err(format!(
                        "LZ4 restauró {} bytes; se esperaban {}",
                        restored.len(),
                        original_size
                    )
                    .into());
                }

                Ok(restored)
            }

            Self::Zstd {
                data,
                original_size,
            } => {
                let restored =
                    zstd::stream::decode_all(
                        std::io::Cursor::new(data),
                    )?;

                if restored.len() != *original_size {
                    return Err(format!(
                        "Zstd restauró {} bytes; se esperaban {}",
                        restored.len(),
                        original_size
                    )
                    .into());
                }

                Ok(restored)
            }
        }
    }
}

#[derive(Debug, Default)]
struct CompressionKnowledge {
    lz4_rejected: bool,
    zstd_rejected: bool,
}

#[derive(Debug)]
struct ManagedBlock {
    hash: blake3::Hash,
    category: ResourceCategory,

    references: usize,
    workspace_references: HashMap<String, usize>,

    temperature: Temperature,
    last_access_tick: u64,

    compression_knowledge: CompressionKnowledge,
    storage: StoredData,
}

impl ManagedBlock {
    fn original_size(&self) -> usize {
        self.storage.original_size()
    }

    fn stored_size(&self) -> usize {
        self.storage.stored_size()
    }

    fn codec(&self) -> StorageCodec {
        self.storage.codec()
    }

    fn restore_verified(
        &self,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let restored = self.storage.restore()?;

        if blake3::hash(&restored) != self.hash {
            return Err(
                "BLAKE3 no coincide después de restaurar".into()
            );
        }

        Ok(restored)
    }

    fn age(
        &mut self,
        current_tick: u64,
        mode: MemoryMode,
    ) -> Result<(), Box<dyn Error>> {
        let idle =
            current_tick.saturating_sub(self.last_access_tick);

        if idle >= COLD_AFTER_TICKS {
            self.temperature = Temperature::Cold;
            self.apply_mode(mode)?;
        } else if idle >= WARM_AFTER_TICKS {
            self.temperature = Temperature::Warm;
            self.apply_mode(mode)?;
        }

        Ok(())
    }

    fn apply_mode(
        &mut self,
        mode: MemoryMode,
    ) -> Result<(), Box<dyn Error>> {
        if self.temperature == Temperature::Hot {
            return Ok(());
        }

        match mode {
            MemoryMode::Ram => Ok(()),
            MemoryMode::Lz4 => self.compress_lz4(),
            MemoryMode::Zstd { level } => {
                self.compress_zstd(level)
            }
        }
    }

    fn compress_lz4(&mut self) -> Result<(), Box<dyn Error>> {
        if self.codec() == StorageCodec::Lz4
            || self.compression_knowledge.lz4_rejected
        {
            return Ok(());
        }

        let original = self.restore_verified()?;
        let compressed =
            lz4_flex::compress_prepend_size(&original);

        if compressed.len() >= original.len() {
            self.compression_knowledge.lz4_rejected = true;
            self.storage = StoredData::Raw(original);
            return Ok(());
        }

        self.storage = StoredData::Lz4 {
            original_size: original.len(),
            data: compressed,
        };

        Ok(())
    }

    fn compress_zstd(
        &mut self,
        level: i32,
    ) -> Result<(), Box<dyn Error>> {
        if self.codec() == StorageCodec::Zstd
            || self.compression_knowledge.zstd_rejected
        {
            return Ok(());
        }

        let original = self.restore_verified()?;
        let compressed =
            zstd::stream::encode_all(
                std::io::Cursor::new(&original),
                level,
            )?;

        if compressed.len() >= original.len() {
            self.compression_knowledge.zstd_rejected = true;
            self.storage = StoredData::Raw(original);
            return Ok(());
        }

        self.storage = StoredData::Zstd {
            original_size: original.len(),
            data: compressed,
        };

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub resources: usize,
    pub logical_blocks: usize,
    pub unique_blocks: usize,
    pub duplicate_blocks: usize,
    pub logical_bytes: usize,
    pub unique_bytes: usize,
    pub physical_bytes: usize,
}

impl MemoryStats {
    pub fn saved_bytes(&self) -> usize {
        self.logical_bytes
            .saturating_sub(self.physical_bytes)
    }
}

#[derive(Debug)]
pub struct MemoryManager {
    mode: MemoryMode,

    index: HashMap<blake3::Hash, Vec<usize>>,
    blocks: Vec<ManagedBlock>,

    resources: usize,

    logical_bytes: usize,
    unique_bytes: usize,

    logical_blocks: usize,
    duplicate_blocks: usize,

    current_tick: u64,
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new(MemoryMode::default())
    }
}

impl MemoryManager {
    pub fn new(mode: MemoryMode) -> Self {
        Self {
            mode,
            index: HashMap::new(),
            blocks: Vec::new(),
            resources: 0,
            logical_bytes: 0,
            unique_bytes: 0,
            logical_blocks: 0,
            duplicate_blocks: 0,
            current_tick: 0,
        }
    }

    pub fn store(
        &mut self,
        resource: MemoryResource,
    ) -> Result<MemoryStats, Box<dyn Error>> {
        if resource.body.is_empty() {
            return Ok(self.stats());
        }

        self.resources += 1;
        self.current_tick =
            self.current_tick.saturating_add(1);

        let category =
            ResourceCategory::from_mime(&resource.mime_type);

        for chunk in resource.body.chunks(BLOCK_SIZE) {
            self.insert_block(
                &resource.workspace,
                category,
                chunk,
            )?;
        }

        if self.current_tick % AGING_INTERVAL_TICKS == 0 {
            self.age()?;
        }

        Ok(self.stats())
    }

    fn insert_block(
        &mut self,
        workspace: &str,
        category: ResourceCategory,
        data: &[u8],
    ) -> Result<(), Box<dyn Error>> {
        if data.is_empty() {
            return Ok(());
        }

        self.logical_bytes += data.len();
        self.logical_blocks += 1;

        let hash = blake3::hash(data);

        if let Some(candidates) = self.index.get(&hash) {
            for index in candidates {
                let block = &self.blocks[*index];

                if block.original_size() != data.len() {
                    continue;
                }

                let existing = block.restore_verified()?;

                if existing.as_slice() == data {
                    let block = &mut self.blocks[*index];

                    block.references += 1;

                    *block
                        .workspace_references
                        .entry(workspace.to_string())
                        .or_insert(0) += 1;

                    block.last_access_tick =
                        self.current_tick;

                    self.duplicate_blocks += 1;

                    return Ok(());
                }
            }
        }

        let mut workspace_references = HashMap::new();
        workspace_references.insert(workspace.to_string(), 1);

        let index = self.blocks.len();

        self.blocks.push(ManagedBlock {
            hash,
            category,
            references: 1,
            workspace_references,
            temperature: Temperature::Hot,
            last_access_tick: self.current_tick,
            compression_knowledge:
                CompressionKnowledge::default(),
            storage: StoredData::Raw(data.to_vec()),
        });

        self.index.entry(hash).or_default().push(index);
        self.unique_bytes += data.len();

        Ok(())
    }

    fn age(&mut self) -> Result<(), Box<dyn Error>> {
        for block in &mut self.blocks {
            block.age(self.current_tick, self.mode)?;
        }

        Ok(())
    }

    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            resources: self.resources,
            logical_blocks: self.logical_blocks,
            unique_blocks: self.blocks.len(),
            duplicate_blocks: self.duplicate_blocks,
            logical_bytes: self.logical_bytes,
            unique_bytes: self.unique_bytes,
            physical_bytes: self.physical_bytes(),
        }
    }

    pub fn len(&self) -> usize {
        self.resources
    }

    pub fn is_empty(&self) -> bool {
        self.resources == 0
    }

    pub fn physical_bytes(&self) -> usize {
        self.blocks
            .iter()
            .map(ManagedBlock::stored_size)
            .sum()
    }

    pub fn deduplicated_bytes(&self) -> usize {
        self.logical_bytes
            .saturating_sub(self.unique_bytes)
    }

    pub fn verify_all(&self) -> Result<(), Box<dyn Error>> {
        for block in &self.blocks {
            block.restore_verified()?;
        }

        Ok(())
    }
}