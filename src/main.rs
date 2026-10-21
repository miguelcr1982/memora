use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use base64::{
    engine::general_purpose::STANDARD,
    Engine,
};
use serde::Deserialize;

const DEFAULT_CAPTURE_DIR: &str = "captures";
const DEFAULT_ZSTD_LEVEL: i32 = 3;

const BLOCK_SIZE: usize = 64 * 1024;

const WARM_AFTER_TICKS: u64 = 50;
const COLD_AFTER_TICKS: u64 = 200;
const AGING_INTERVAL_TICKS: u64 = 25;

const SIMULATION_ACCESSES: u64 = 20_000;

const HOT_ACCESS_PERCENT: u64 = 80;
const HOT_SET_PERCENT: usize = 20;

//
// Modo seleccionado por el usuario
//

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
enum MemoryMode {
    Ram,
    Lz4,
    Zstd {
        level: i32,
    },
}

impl MemoryMode {
    fn parse(
        value: &str,
        zstd_level: i32,
    ) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "ram" => Ok(Self::Ram),

            "lz4" => Ok(Self::Lz4),

            "zstd" => Ok(Self::Zstd {
                level: zstd_level,
            }),

            _ => Err(format!(
                "Modo inválido '{}'. Utilice ram, lz4 o zstd.",
                value
            )),
        }
    }

    fn label(self) -> String {
        match self {
            Self::Ram => {
                "RAM: deduplicación sin compresión"
                    .to_string()
            }

            Self::Lz4 => {
                "LZ4: compresión rápida"
                    .to_string()
            }

            Self::Zstd { level } => {
                format!(
                    "Zstd: máxima compresión, nivel {}",
                    level
                )
            }
        }
    }

    fn codec_label(self) -> &'static str {
        match self {
            Self::Ram => "RAW",
            Self::Lz4 => "LZ4",
            Self::Zstd { .. } => "Zstd",
        }
    }
}

//
// HAR
//

#[derive(Debug, Deserialize)]
struct Har {
    log: HarLog,
}

#[derive(Debug, Deserialize)]
struct HarLog {
    entries: Vec<HarEntry>,
}

#[derive(Debug, Deserialize)]
struct HarEntry {
    request: HarRequest,
    response: HarResponse,
}

#[derive(Debug, Deserialize)]
struct HarRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
struct HarResponse {
    content: HarContent,
}

#[derive(Debug, Deserialize)]
struct HarContent {
    #[serde(rename = "mimeType", default)]
    mime_type: String,

    #[serde(default)]
    text: Option<String>,

    #[serde(default)]
    encoding: Option<String>,
}

//
// Recursos
//

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
)]
enum ResourceCategory {
    Html,
    JavaScript,
    Css,
    Json,
    Image,
    Font,
    Wasm,
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

    fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::JavaScript => "JavaScript",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Image => "Imágenes",
            Self::Font => "Fuentes",
            Self::Wasm => "WASM",
            Self::Other => "Otros",
        }
    }
}

#[derive(Debug)]
struct Resource {
    workspace: String,
    url: String,
    category: ResourceCategory,
    data: Vec<u8>,
}

//
// Temperatura lógica
//

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
)]
enum Temperature {
    Hot,
    Warm,
    Cold,
}

impl Temperature {
    fn label(self) -> &'static str {
        match self {
            Self::Hot => "HOT",
            Self::Warm => "WARM",
            Self::Cold => "COLD",
        }
    }
}

//
// Códec físico actual
//

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
)]
enum StorageCodec {
    Raw,
    Lz4,
    Zstd,
}

impl StorageCodec {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "RAW",
            Self::Lz4 => "LZ4",
            Self::Zstd => "Zstd",
        }
    }
}

//
// Datos almacenados
//

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
            } => *original_size,

            Self::Zstd {
                original_size,
                ..
            } => *original_size,
        }
    }

    fn restore(
        &self,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        match self {
            Self::Raw(data) => {
                Ok(data.clone())
            }

            Self::Lz4 {
                data,
                original_size,
            } => {
                let restored =
                    lz4_flex::decompress_size_prepended(
                        data,
                    )?;

                if restored.len() != *original_size {
                    return Err(
                        format!(
                            "LZ4 restauró {} bytes; se esperaban {}",
                            restored.len(),
                            original_size
                        )
                        .into(),
                    );
                }

                Ok(restored)
            }

            Self::Zstd {
                data,
                original_size,
            } => {
                let restored =
                    zstd::stream::decode_all(
                        Cursor::new(data),
                    )?;

                if restored.len() != *original_size {
                    return Err(
                        format!(
                            "Zstd restauró {} bytes; se esperaban {}",
                            restored.len(),
                            original_size
                        )
                        .into(),
                    );
                }

                Ok(restored)
            }
        }
    }
}

//
// Información aprendida por bloque
//

#[derive(Debug, Default)]
struct CompressionKnowledge {
    lz4_rejected: bool,
    zstd_rejected: bool,
}

//
// Bloque administrado
//

#[derive(Debug)]
struct ManagedBlock {
    hash: blake3::Hash,
    category: ResourceCategory,

    references: usize,
    workspace_references: HashMap<String, usize>,

    temperature: Temperature,

    last_access_tick: u64,
    access_count: u64,

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

    fn idle_ticks(
        &self,
        current_tick: u64,
    ) -> u64 {
        current_tick.saturating_sub(
            self.last_access_tick,
        )
    }

    fn restore_verified(
        &self,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let restored = self.storage.restore()?;

        if blake3::hash(&restored) != self.hash {
            return Err(
                "BLAKE3 no coincide después de restaurar"
                    .into(),
            );
        }

        Ok(restored)
    }

    /*
     * Acceso al bloque.
     *
     * Todo acceso lleva la temperatura a HOT.
     *
     * RAW se accede directamente.
     * LZ4 y Zstd se restauran a RAW.
     */
    fn access(
        &mut self,
        current_tick: u64,
    ) -> Result<AccessResult, Box<dyn std::error::Error>> {
        let previous_temperature =
            self.temperature;

        let previous_codec =
            self.codec();

        let stored_before =
            self.stored_size();

        if previous_codec == StorageCodec::Raw {
            let start = Instant::now();

            let original_bytes =
                match &self.storage {
                    StoredData::Raw(data) => {
                        std::hint::black_box(
                            data.as_ptr(),
                        );

                        std::hint::black_box(
                            data.len(),
                        );

                        data.len()
                    }

                    _ => unreachable!(),
                };

            let elapsed =
                start.elapsed();

            self.temperature =
                Temperature::Hot;

            self.last_access_tick =
                current_tick;

            self.access_count += 1;

            return Ok(AccessResult {
                previous_temperature,
                previous_codec,
                original_bytes,
                stored_before,
                elapsed,
            });
        }

        let start = Instant::now();

        let restored =
            self.restore_verified()?;

        let elapsed =
            start.elapsed();

        let original_bytes =
            restored.len();

        self.storage =
            StoredData::Raw(restored);

        self.temperature =
            Temperature::Hot;

        self.last_access_tick =
            current_tick;

        self.access_count += 1;

        Ok(AccessResult {
            previous_temperature,
            previous_codec,
            original_bytes,
            stored_before,
            elapsed,
        })
    }

    /*
     * Aplica la temperatura y el modo elegidos
     * por el usuario.
     */
    fn transition(
        &mut self,
        target_temperature: Temperature,
        mode: MemoryMode,
    ) -> Result<TransitionResult, Box<dyn std::error::Error>> {
        self.temperature =
            target_temperature;

        /*
         * En modo RAM no se comprime.
         *
         * Si por alguna razón el bloque estuviera
         * comprimido, se restaura a RAW.
         */
        if mode == MemoryMode::Ram {
            if self.codec() == StorageCodec::Raw {
                return Ok(
                    TransitionResult::temperature_only(),
                );
            }

            let input_bytes =
                self.stored_size();

            let start =
                Instant::now();

            let restored =
                self.restore_verified()?;

            let elapsed =
                start.elapsed();

            let output_bytes =
                restored.len();

            self.storage =
                StoredData::Raw(restored);

            return Ok(TransitionResult {
                attempted: true,
                accepted: true,
                known_rejection: false,
                temperature_only: false,
                input_bytes,
                output_bytes,
                elapsed,
            });
        }

        /*
         * HOT siempre permanece RAW,
         * independientemente del modo.
         */
        if target_temperature == Temperature::Hot {
            if self.codec() == StorageCodec::Raw {
                return Ok(
                    TransitionResult::temperature_only(),
                );
            }

            let input_bytes =
                self.stored_size();

            let start =
                Instant::now();

            let restored =
                self.restore_verified()?;

            let elapsed =
                start.elapsed();

            let output_bytes =
                restored.len();

            self.storage =
                StoredData::Raw(restored);

            return Ok(TransitionResult {
                attempted: true,
                accepted: true,
                known_rejection: false,
                temperature_only: false,
                input_bytes,
                output_bytes,
                elapsed,
            });
        }

        match mode {
            MemoryMode::Ram => {
                unreachable!()
            }

            MemoryMode::Lz4 => {
                self.compress_lz4()
            }

            MemoryMode::Zstd { level } => {
                self.compress_zstd(
                    level
                )
            }
        }
    }

    fn compress_lz4(
        &mut self,
    ) -> Result<TransitionResult, Box<dyn std::error::Error>> {
        /*
         * Ya está comprimido con el códec elegido.
         * Solo pudo cambiar la temperatura.
         */
        if self.codec() == StorageCodec::Lz4 {
            return Ok(
                TransitionResult::temperature_only(),
            );
        }

        if self
            .compression_knowledge
            .lz4_rejected
        {
            return Ok(
                TransitionResult::known_rejection(),
            );
        }

        let original =
            self.restore_verified()?;

        let input_bytes =
            original.len();

        let start =
            Instant::now();

        let compressed =
            lz4_flex::compress_prepend_size(
                &original,
            );

        let elapsed =
            start.elapsed();

        if compressed.len()
            >= original.len()
        {
            self
                .compression_knowledge
                .lz4_rejected = true;

            self.storage =
                StoredData::Raw(original);

            return Ok(TransitionResult {
                attempted: true,
                accepted: false,
                known_rejection: false,
                temperature_only: false,
                input_bytes,
                output_bytes: input_bytes,
                elapsed,
            });
        }

        let original_size =
            original.len();

        let output_bytes =
            compressed.len();

        self.storage =
            StoredData::Lz4 {
                data: compressed,
                original_size,
            };

        Ok(TransitionResult {
            attempted: true,
            accepted: true,
            known_rejection: false,
            temperature_only: false,
            input_bytes,
            output_bytes,
            elapsed,
        })
    }

    fn compress_zstd(
        &mut self,
        level: i32,
    ) -> Result<TransitionResult, Box<dyn std::error::Error>> {
        if self.codec() == StorageCodec::Zstd {
            return Ok(
                TransitionResult::temperature_only(),
            );
        }

        if self
            .compression_knowledge
            .zstd_rejected
        {
            return Ok(
                TransitionResult::known_rejection(),
            );
        }

        let original =
            self.restore_verified()?;

        let input_bytes =
            original.len();

        let start =
            Instant::now();

        let compressed =
            zstd::stream::encode_all(
                Cursor::new(&original),
                level,
            )?;

        let elapsed =
            start.elapsed();

        if compressed.len()
            >= original.len()
        {
            self
                .compression_knowledge
                .zstd_rejected = true;

            self.storage =
                StoredData::Raw(original);

            return Ok(TransitionResult {
                attempted: true,
                accepted: false,
                known_rejection: false,
                temperature_only: false,
                input_bytes,
                output_bytes: input_bytes,
                elapsed,
            });
        }

        let original_size =
            original.len();

        let output_bytes =
            compressed.len();

        self.storage =
            StoredData::Zstd {
                data: compressed,
                original_size,
            };

        Ok(TransitionResult {
            attempted: true,
            accepted: true,
            known_rejection: false,
            temperature_only: false,
            input_bytes,
            output_bytes,
            elapsed,
        })
    }
}

//
// Resultados
//

#[derive(Debug)]
struct AccessResult {
    previous_temperature: Temperature,
    previous_codec: StorageCodec,

    original_bytes: usize,
    stored_before: usize,

    elapsed: Duration,
}

#[derive(Debug)]
struct TransitionResult {
    attempted: bool,
    accepted: bool,

    known_rejection: bool,
    temperature_only: bool,

    input_bytes: usize,
    output_bytes: usize,

    elapsed: Duration,
}

impl TransitionResult {
    fn temperature_only() -> Self {
        Self {
            attempted: false,
            accepted: false,
            known_rejection: false,
            temperature_only: true,
            input_bytes: 0,
            output_bytes: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn known_rejection() -> Self {
        Self {
            attempted: false,
            accepted: false,
            known_rejection: true,
            temperature_only: false,
            input_bytes: 0,
            output_bytes: 0,
            elapsed: Duration::ZERO,
        }
    }
}

//
// Métricas
//

#[derive(Debug, Default)]
struct TransitionMetrics {
    attempts: usize,
    accepted: usize,
    rejected: usize,

    known_rejections_skipped: usize,
    temperature_only: usize,

    input_bytes: usize,
    output_bytes: usize,

    elapsed: Duration,
}

impl TransitionMetrics {
    fn add(
        &mut self,
        result: TransitionResult,
    ) {
        if result.known_rejection {
            self.known_rejections_skipped += 1;
            return;
        }

        if result.temperature_only {
            self.temperature_only += 1;
            return;
        }

        if !result.attempted {
            return;
        }

        self.attempts += 1;

        self.input_bytes +=
            result.input_bytes;

        self.output_bytes +=
            result.output_bytes;

        self.elapsed +=
            result.elapsed;

        if result.accepted {
            self.accepted += 1;
        } else {
            self.rejected += 1;
        }
    }
}

#[derive(Debug, Default)]
struct AgingMetrics {
    warm: TransitionMetrics,
    cold: TransitionMetrics,
}

#[derive(Debug, Default)]
struct AccessMetrics {
    accesses: u64,

    original_bytes: usize,
    stored_before: usize,

    elapsed: Duration,
}

impl AccessMetrics {
    fn add(
        &mut self,
        result: &AccessResult,
    ) {
        self.accesses += 1;

        self.original_bytes +=
            result.original_bytes;

        self.stored_before +=
            result.stored_before;

        self.elapsed +=
            result.elapsed;
    }
}

#[derive(Debug, Default)]
struct SimulationMetrics {
    total_accesses: u64,
    aging_cycles: u64,

    raw_accesses: AccessMetrics,
    lz4_accesses: AccessMetrics,
    zstd_accesses: AccessMetrics,

    hot_accesses: u64,
    warm_accesses: u64,
    cold_accesses: u64,

    aging: AgingMetrics,

    minimum_physical_bytes: usize,
    maximum_physical_bytes: usize,

    accumulated_physical_bytes: u128,
    physical_samples: u64,
}

impl SimulationMetrics {
    fn record_physical(
        &mut self,
        bytes: usize,
    ) {
        if self.physical_samples == 0 {
            self.minimum_physical_bytes =
                bytes;

            self.maximum_physical_bytes =
                bytes;
        } else {
            self.minimum_physical_bytes =
                self
                    .minimum_physical_bytes
                    .min(bytes);

            self.maximum_physical_bytes =
                self
                    .maximum_physical_bytes
                    .max(bytes);
        }

        self.accumulated_physical_bytes +=
            bytes as u128;

        self.physical_samples += 1;
    }

    fn average_physical_bytes(
        &self,
    ) -> usize {
        if self.physical_samples == 0 {
            return 0;
        }

        (
            self.accumulated_physical_bytes
                / self.physical_samples as u128
        ) as usize
    }
}

#[derive(Debug, Default)]
struct StateStats {
    blocks: usize,
    original_bytes: usize,
    stored_bytes: usize,
}

//
// Gestor
//

#[derive(Default)]
struct MemoryManager {
    index:
        HashMap<blake3::Hash, Vec<usize>>,

    blocks:
        Vec<ManagedBlock>,

    logical_bytes: usize,
    unique_bytes: usize,

    logical_blocks: usize,
    duplicate_blocks: usize,
}

impl MemoryManager {
    fn insert(
        &mut self,
        workspace: &str,
        category: ResourceCategory,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        if data.is_empty() {
            return Ok(());
        }

        self.logical_bytes +=
            data.len();

        self.logical_blocks += 1;

        let hash =
            blake3::hash(data);

        if let Some(candidates) =
            self.index.get(&hash)
        {
            for index in candidates {
                let block =
                    &self.blocks[*index];

                if block.original_size()
                    != data.len()
                {
                    continue;
                }

                let existing =
                    block.restore_verified()?;

                if existing.as_slice()
                    == data
                {
                    let block =
                        &mut self.blocks[*index];

                    block.references += 1;

                    *block
                        .workspace_references
                        .entry(
                            workspace.to_string()
                        )
                        .or_insert(0) += 1;

                    self.duplicate_blocks += 1;

                    return Ok(());
                }
            }
        }

        let mut workspace_references =
            HashMap::new();

        workspace_references.insert(
            workspace.to_string(),
            1,
        );

        let index =
            self.blocks.len();

        self.blocks.push(
            ManagedBlock {
                hash,
                category,

                references: 1,
                workspace_references,

                temperature:
                    Temperature::Hot,

                last_access_tick: 0,
                access_count: 0,

                compression_knowledge:
                    CompressionKnowledge::default(),

                storage:
                    StoredData::Raw(
                        data.to_vec(),
                    ),
            },
        );

        self.index
            .entry(hash)
            .or_default()
            .push(index);

        self.unique_bytes +=
            data.len();

        Ok(())
    }

    fn block_count(&self) -> usize {
        self.blocks.len()
    }

    fn physical_bytes(&self) -> usize {
        self.blocks
            .iter()
            .map(|block| {
                block.stored_size()
            })
            .sum()
    }

    fn deduplicated_bytes(&self) -> usize {
        self.logical_bytes
            .saturating_sub(
                self.unique_bytes
            )
    }

    fn access_block(
        &mut self,
        index: usize,
        tick: u64,
    ) -> Result<AccessResult, Box<dyn std::error::Error>> {
        let block =
            self.blocks
                .get_mut(index)
                .ok_or(
                    "Índice de bloque inválido"
                )?;

        block.access(tick)
    }

    fn age(
        &mut self,
        current_tick: u64,
        mode: MemoryMode,
    ) -> Result<AgingMetrics, Box<dyn std::error::Error>> {
        let mut metrics =
            AgingMetrics::default();

        for block in &mut self.blocks {
            let idle =
                block.idle_ticks(
                    current_tick
                );

            if idle >= COLD_AFTER_TICKS {
                metrics.cold.add(
                    block.transition(
                        Temperature::Cold,
                        mode,
                    )?,
                );
            } else if idle >= WARM_AFTER_TICKS {
                metrics.warm.add(
                    block.transition(
                        Temperature::Warm,
                        mode,
                    )?,
                );
            }
        }

        Ok(metrics)
    }

    fn temperature_summary(
        &self,
    ) -> BTreeMap<Temperature, StateStats> {
        let mut result =
            BTreeMap::new();

        for block in &self.blocks {
            let entry =
                result
                    .entry(
                        block.temperature
                    )
                    .or_insert_with(
                        StateStats::default
                    );

            entry.blocks += 1;

            entry.original_bytes +=
                block.original_size();

            entry.stored_bytes +=
                block.stored_size();
        }

        result
    }

    fn codec_summary(
        &self,
    ) -> BTreeMap<StorageCodec, StateStats> {
        let mut result =
            BTreeMap::new();

        for block in &self.blocks {
            let entry =
                result
                    .entry(
                        block.codec()
                    )
                    .or_insert_with(
                        StateStats::default
                    );

            entry.blocks += 1;

            entry.original_bytes +=
                block.original_size();

            entry.stored_bytes +=
                block.stored_size();
        }

        result
    }

    fn category_summary(
        &self,
    ) -> BTreeMap<ResourceCategory, StateStats> {
        let mut result =
            BTreeMap::new();

        for block in &self.blocks {
            let entry =
                result
                    .entry(
                        block.category
                    )
                    .or_insert_with(
                        StateStats::default
                    );

            entry.blocks += 1;

            entry.original_bytes +=
                block.original_size();

            entry.stored_bytes +=
                block.stored_size();
        }

        result
    }

    fn verify_all(
        &self,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for block in &self.blocks {
            block.restore_verified()?;
        }

        Ok(())
    }
}

//
// Generador determinista
//

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state =
            self.state
                .wrapping_mul(
                    6_364_136_223_846_793_005
                )
                .wrapping_add(1);

        self.state
    }

    fn range(
        &mut self,
        maximum: usize,
    ) -> usize {
        if maximum == 0 {
            return 0;
        }

        self.next_u64() as usize
            % maximum
    }

    fn percent(&mut self) -> u64 {
        self.next_u64() % 100
    }
}

//
// HAR
//

#[derive(Debug, Default)]
struct HarStats {
    multimedia_excluded: usize,
    missing_body: usize,
    invalid_base64: usize,
}

fn discover_har_files(
    directory: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files =
        Vec::new();

    for entry in
        fs::read_dir(directory)?
    {
        let entry = match entry {
            Ok(entry) => entry,

            Err(error) => {
                eprintln!(
                    "[WARN] Entrada inválida: {}",
                    error
                );

                continue;
            }
        };

        let path =
            entry.path();

        if !path.is_file() {
            continue;
        }

        let is_har =
            path.extension()
                .and_then(|extension| {
                    extension.to_str()
                })
                .map(|extension| {
                    extension
                        .eq_ignore_ascii_case(
                            "har"
                        )
                })
                .unwrap_or(false);

        if is_har {
            files.push(path);
        }
    }

    files.sort();

    Ok(files)
}

fn load_har(
    path: &Path,
) -> Result<(Vec<Resource>, HarStats), String> {
    let metadata =
        fs::metadata(path)
            .map_err(|error| {
                format!(
                    "No se pudo leer metadata: {}",
                    error
                )
            })?;

    if metadata.len() == 0 {
        return Err(
            "HAR vacío".to_string()
        );
    }

    let text =
        fs::read_to_string(path)
            .map_err(|error| {
                format!(
                    "No se pudo leer: {}",
                    error
                )
            })?;

    if text.trim().is_empty() {
        return Err(
            "HAR vacío".to_string()
        );
    }

    let har: Har =
        serde_json::from_str(&text)
            .map_err(|error| {
                format!(
                    "HAR inválido: {}",
                    error
                )
            })?;

    let workspace =
        path.file_stem()
            .and_then(|name| {
                name.to_str()
            })
            .unwrap_or("workspace")
            .to_string();

    Ok(
        extract_resources(
            har,
            &workspace,
        )
    )
}

fn extract_resources(
    har: Har,
    workspace: &str,
) -> (Vec<Resource>, HarStats) {
    let mut resources =
        Vec::new();

    let mut stats =
        HarStats::default();

    for entry in har.log.entries {
        let content =
            entry.response.content;

        let mime =
            content.mime_type
                .to_ascii_lowercase();

        if is_multimedia(&mime) {
            stats.multimedia_excluded += 1;
            continue;
        }

        let Some(text) =
            content.text
        else {
            stats.missing_body += 1;
            continue;
        };

        let data =
            if content
                .encoding
                .as_deref()
                .is_some_and(|encoding| {
                    encoding
                        .eq_ignore_ascii_case(
                            "base64"
                        )
                })
            {
                match STANDARD.decode(
                    text.as_bytes()
                ) {
                    Ok(data) => data,

                    Err(_) => {
                        stats.invalid_base64 += 1;
                        continue;
                    }
                }
            } else {
                text.into_bytes()
            };

        if data.is_empty() {
            continue;
        }

        resources.push(
            Resource {
                workspace:
                    workspace.to_string(),

                url:
                    entry.request.url,

                category:
                    ResourceCategory::from_mime(
                        &mime
                    ),

                data,
            },
        );
    }

    (resources, stats)
}

fn is_multimedia(
    mime: &str,
) -> bool {
    mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime.contains("mpegurl")
        || mime.contains("dash")
}

//
// Simulación
//

fn run_simulation(
    manager: &mut MemoryManager,
    mode: MemoryMode,
) -> Result<SimulationMetrics, Box<dyn std::error::Error>> {
    let mut metrics =
        SimulationMetrics::default();

    let mut rng =
        SimpleRng::new(
            0xC0FFEE
        );

    let block_count =
        manager.block_count();

    if block_count == 0 {
        return Ok(metrics);
    }

    let hot_set_size =
        (
            block_count
                * HOT_SET_PERCENT
                / 100
        )
        .max(1);

    let initial_tick =
        COLD_AFTER_TICKS + 1;

    let initial_aging =
        manager.age(
            initial_tick,
            mode,
        )?;

    merge_aging(
        &mut metrics.aging,
        initial_aging,
    );

    metrics.aging_cycles += 1;

    metrics.record_physical(
        manager.physical_bytes()
    );

    for access_number in
        1..=SIMULATION_ACCESSES
    {
        let current_tick =
            initial_tick
                + access_number;

        let index =
            if rng.percent()
                < HOT_ACCESS_PERCENT
            {
                rng.range(
                    hot_set_size
                )
            } else {
                rng.range(
                    block_count
                )
            };

        let access =
            manager.access_block(
                index,
                current_tick,
            )?;

        match access.previous_codec {
            StorageCodec::Raw => {
                metrics
                    .raw_accesses
                    .add(&access);
            }

            StorageCodec::Lz4 => {
                metrics
                    .lz4_accesses
                    .add(&access);
            }

            StorageCodec::Zstd => {
                metrics
                    .zstd_accesses
                    .add(&access);
            }
        }

        match access.previous_temperature {
            Temperature::Hot => {
                metrics.hot_accesses += 1;
            }

            Temperature::Warm => {
                metrics.warm_accesses += 1;
            }

            Temperature::Cold => {
                metrics.cold_accesses += 1;
            }
        }

        metrics.total_accesses += 1;

        if current_tick
            % AGING_INTERVAL_TICKS
            == 0
        {
            let aging =
                manager.age(
                    current_tick,
                    mode,
                )?;

            merge_aging(
                &mut metrics.aging,
                aging,
            );

            metrics.aging_cycles += 1;

            metrics.record_physical(
                manager.physical_bytes()
            );
        }
    }

    metrics.record_physical(
        manager.physical_bytes()
    );

    Ok(metrics)
}

fn merge_transition(
    target: &mut TransitionMetrics,
    source: TransitionMetrics,
) {
    target.attempts +=
        source.attempts;

    target.accepted +=
        source.accepted;

    target.rejected +=
        source.rejected;

    target.known_rejections_skipped +=
        source.known_rejections_skipped;

    target.temperature_only +=
        source.temperature_only;

    target.input_bytes +=
        source.input_bytes;

    target.output_bytes +=
        source.output_bytes;

    target.elapsed +=
        source.elapsed;
}

fn merge_aging(
    target: &mut AgingMetrics,
    source: AgingMetrics,
) {
    merge_transition(
        &mut target.warm,
        source.warm,
    );

    merge_transition(
        &mut target.cold,
        source.cold,
    );
}

//
// Salida
//

fn print_resource_summary(
    resources: &[Resource],
) {
    let mut workspaces:
        BTreeMap<String, (usize, usize)> =
        BTreeMap::new();

    let mut categories:
        BTreeMap<ResourceCategory, (usize, usize)> =
        BTreeMap::new();

    let mut total_bytes =
        0usize;

    for resource in resources {
        total_bytes +=
            resource.data.len();

        let workspace =
            workspaces
                .entry(
                    resource.workspace.clone()
                )
                .or_insert((0, 0));

        workspace.0 += 1;

        workspace.1 +=
            resource.data.len();

        let category =
            categories
                .entry(resource.category)
                .or_insert((0, 0));

        category.0 += 1;

        category.1 +=
            resource.data.len();
    }

    println!(
        "--- Recursos capturados ---"
    );

    println!(
        "Workspaces:            {}",
        workspaces.len()
    );

    println!(
        "Recursos:              {}",
        resources.len()
    );

    println!(
        "Contenido lógico:      {}",
        format_bytes(total_bytes)
    );

    println!();
    println!("Por workspace:");

    for (
        workspace,
        (count, bytes),
    ) in workspaces
    {
        println!(
            "  {:<32} {:>5} recursos {:>12}",
            truncate(
                &workspace,
                32
            ),
            count,
            format_bytes(bytes)
        );
    }

    println!();
    println!("Por categoría:");

    for (
        category,
        (count, bytes),
    ) in categories
    {
        println!(
            "  {:<12} {:>6} recursos {:>12}",
            category.label(),
            count,
            format_bytes(bytes)
        );
    }

    println!();
    println!("Ejemplos:");

    for resource in
        resources.iter().take(8)
    {
        println!(
            "  {:<16} {:<11} {:>10} {}",
            truncate(
                &resource.workspace,
                16
            ),
            resource.category.label(),
            format_bytes(
                resource.data.len()
            ),
            truncate(
                &resource.url,
                68
            )
        );
    }
}

fn print_manager_summary(
    manager: &MemoryManager,
) {
    println!(
        "--- Carga inicial HOT ---"
    );

    println!(
        "Bloques lógicos:       {}",
        manager.logical_blocks
    );

    println!(
        "Bloques únicos:        {}",
        manager.block_count()
    );

    println!(
        "Bloques duplicados:    {}",
        manager.duplicate_blocks
    );

    println!(
        "Contenido lógico:      {}",
        format_bytes(
            manager.logical_bytes
        )
    );

    println!(
        "Contenido único:       {}",
        format_bytes(
            manager.unique_bytes
        )
    );

    println!(
        "Ahorro dedupe:         {} ({:.2} %)",
        format_bytes(
            manager.deduplicated_bytes()
        ),
        saved_percent(
            manager.logical_bytes,
            manager.unique_bytes,
        )
    );

    println!(
        "Físico administrado:   {}",
        format_bytes(
            manager.physical_bytes()
        )
    );
}

fn print_access_metrics(
    label: &str,
    metrics: &AccessMetrics,
) {
    println!("{}:", label);

    println!(
        "  Accesos:             {}",
        metrics.accesses
    );

    println!(
        "  Datos atendidos:     {}",
        format_bytes(
            metrics.original_bytes
        )
    );

    println!(
        "  Físico previo:       {}",
        format_bytes(
            metrics.stored_before
        )
    );

    println!(
        "  Tiempo acumulado:    {:.3} ms",
        metrics.elapsed
            .as_secs_f64()
            * 1000.0
    );

    let latency_ns =
        if metrics.accesses == 0 {
            0.0
        } else {
            metrics.elapsed
                .as_secs_f64()
                * 1_000_000_000.0
                / metrics.accesses as f64
        };

    println!(
        "  Latencia media:      {:.2} ns/acceso",
        latency_ns
    );

    println!(
        "  Rendimiento:         {:.2} MiB/s",
        throughput_mib(
            metrics.original_bytes,
            metrics.elapsed,
        )
    );
}

fn print_transition_metrics(
    label: &str,
    metrics: &TransitionMetrics,
) {
    println!("{}:", label);

    println!(
        "  Intentos reales:     {}",
        metrics.attempts
    );

    println!(
        "  Aceptados:           {}",
        metrics.accepted
    );

    println!(
        "  Rechazados:          {}",
        metrics.rejected
    );

    println!(
        "  Reintentos evitados: {}",
        metrics.known_rejections_skipped
    );

    println!(
        "  Solo temperatura:    {}",
        metrics.temperature_only
    );

    println!(
        "  Entrada procesada:   {}",
        format_bytes(
            metrics.input_bytes
        )
    );

    println!(
        "  Salida generada:     {}",
        format_bytes(
            metrics.output_bytes
        )
    );

    println!(
        "  Tiempo acumulado:    {:.2} ms",
        metrics.elapsed
            .as_secs_f64()
            * 1000.0
    );
}

fn print_simulation(
    manager: &MemoryManager,
    metrics: &SimulationMetrics,
    mode: MemoryMode,
) {
    println!(
        "=== Resultado modo {} ===",
        mode.codec_label()
    );

    println!(
        "Accesos simulados:     {}",
        metrics.total_accesses
    );

    println!(
        "Accesos HOT:           {}",
        metrics.hot_accesses
    );

    println!(
        "Accesos WARM:          {}",
        metrics.warm_accesses
    );

    println!(
        "Accesos COLD:          {}",
        metrics.cold_accesses
    );

    println!(
        "Ciclos de aging:       {}",
        metrics.aging_cycles
    );

    println!();

    print_access_metrics(
        "Accesos desde RAW",
        &metrics.raw_accesses,
    );

    println!();

    print_access_metrics(
        "Accesos desde LZ4",
        &metrics.lz4_accesses,
    );

    println!();

    print_access_metrics(
        "Accesos desde Zstd",
        &metrics.zstd_accesses,
    );

    println!();

    print_transition_metrics(
        "Transiciones hacia WARM",
        &metrics.aging.warm,
    );

    println!();

    print_transition_metrics(
        "Transiciones hacia COLD",
        &metrics.aging.cold,
    );

    println!();
    println!("--- Memoria física ---");

    println!(
        "Mínima:                {}",
        format_bytes(
            metrics.minimum_physical_bytes
        )
    );

    println!(
        "Máxima:                {}",
        format_bytes(
            metrics.maximum_physical_bytes
        )
    );

    println!(
        "Media:                 {}",
        format_bytes(
            metrics.average_physical_bytes()
        )
    );

    println!(
        "Final:                 {}",
        format_bytes(
            manager.physical_bytes()
        )
    );

    println!(
        "Ahorro final:          {:.2} %",
        saved_percent(
            manager.logical_bytes,
            manager.physical_bytes(),
        )
    );
}

fn print_temperature_summary(
    manager: &MemoryManager,
) {
    println!(
        "--- Temperatura final ---"
    );

    println!(
        "{:<8} {:>10} {:>14} {:>14}",
        "Estado",
        "Bloques",
        "Original",
        "Físico"
    );

    let summary =
        manager.temperature_summary();

    for temperature in [
        Temperature::Hot,
        Temperature::Warm,
        Temperature::Cold,
    ] {
        let stats =
            summary.get(&temperature);

        let blocks =
            stats
                .map(|value| value.blocks)
                .unwrap_or(0);

        let original =
            stats
                .map(|value| {
                    value.original_bytes
                })
                .unwrap_or(0);

        let stored =
            stats
                .map(|value| {
                    value.stored_bytes
                })
                .unwrap_or(0);

        println!(
            "{:<8} {:>10} {:>14} {:>14}",
            temperature.label(),
            blocks,
            format_bytes(original),
            format_bytes(stored)
        );
    }
}

fn print_codec_summary(
    manager: &MemoryManager,
) {
    println!(
        "--- Almacenamiento físico final ---"
    );

    println!(
        "{:<8} {:>10} {:>14} {:>14} {:>10}",
        "Códec",
        "Bloques",
        "Original",
        "Físico",
        "Ahorro"
    );

    let summary =
        manager.codec_summary();

    for codec in [
        StorageCodec::Raw,
        StorageCodec::Lz4,
        StorageCodec::Zstd,
    ] {
        let stats =
            summary.get(&codec);

        let blocks =
            stats
                .map(|value| value.blocks)
                .unwrap_or(0);

        let original =
            stats
                .map(|value| {
                    value.original_bytes
                })
                .unwrap_or(0);

        let stored =
            stats
                .map(|value| {
                    value.stored_bytes
                })
                .unwrap_or(0);

        println!(
            "{:<8} {:>10} {:>14} {:>14} {:>9.2}%",
            codec.label(),
            blocks,
            format_bytes(original),
            format_bytes(stored),
            saved_percent(
                original,
                stored,
            )
        );
    }
}

fn print_category_summary(
    manager: &MemoryManager,
) {
    println!(
        "--- Estado por categoría ---"
    );

    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>10}",
        "Categoría",
        "Bloques",
        "Original",
        "Físico",
        "Ahorro"
    );

    for (
        category,
        stats,
    ) in manager.category_summary()
    {
        println!(
            "{:<14} {:>10} {:>14} {:>14} {:>9.2}%",
            category.label(),
            stats.blocks,
            format_bytes(
                stats.original_bytes
            ),
            format_bytes(
                stats.stored_bytes
            ),
            saved_percent(
                stats.original_bytes,
                stats.stored_bytes,
            )
        );
    }
}

//
// Main
//

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments:
        Vec<String> =
        env::args().collect();

    let mode_argument =
        arguments
            .get(1)
            .map(String::as_str)
            .unwrap_or("lz4");

    let capture_dir =
        arguments
            .get(2)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    DEFAULT_CAPTURE_DIR
                )
            });

    let zstd_level =
        arguments
            .get(3)
            .and_then(|value| {
                value.parse::<i32>().ok()
            })
            .unwrap_or(
                DEFAULT_ZSTD_LEVEL
            );

    let mode =
        MemoryMode::parse(
            mode_argument,
            zstd_level,
        )
        .map_err(|error| {
            format!(
                "{}\n\nUso:\n  cargo run --release -- ram\n  cargo run --release -- lz4\n  cargo run --release -- zstd\n  cargo run --release -- zstd captures 5",
                error
            )
        })?;

    println!(
        "=== Monster Memory Manager v0.7.0 ==="
    );

    println!();

    println!(
        "Modo elegido: {}",
        mode.label()
    );

    println!(
        "Directorio: {}",
        capture_dir.display()
    );

    println!(
        "Bloque: {}",
        format_bytes(
            BLOCK_SIZE
        )
    );

    println!(
        "Simulación: {} accesos",
        SIMULATION_ACCESSES
    );

    println!();

    if !capture_dir.is_dir() {
        return Err(
            format!(
                "El directorio '{}' no existe",
                capture_dir.display()
            )
            .into(),
        );
    }

    let har_files =
        discover_har_files(
            &capture_dir
        )?;

    println!(
        "HAR encontrados: {}",
        har_files.len()
    );

    println!();

    let mut resources =
        Vec::new();

    let mut valid_hars =
        0usize;

    let mut skipped_hars =
        0usize;

    for har_path in har_files {
        println!(
            "-> {}",
            har_path.display()
        );

        match load_har(&har_path) {
            Ok((
                mut loaded,
                stats,
            )) => {
                println!(
                    "   Multimedia excluida: {}",
                    stats.multimedia_excluded
                );

                println!(
                    "   Sin cuerpo: {}",
                    stats.missing_body
                );

                println!(
                    "   Base64 inválido: {}",
                    stats.invalid_base64
                );

                println!(
                    "   Recursos: {}",
                    loaded.len()
                );

                resources.append(
                    &mut loaded
                );

                valid_hars += 1;
            }

            Err(error) => {
                println!(
                    "   [SKIP] {}",
                    error
                );

                skipped_hars += 1;
            }
        }

        println!();
    }

    println!(
        "HAR válidos:  {}",
        valid_hars
    );

    println!(
        "HAR omitidos: {}",
        skipped_hars
    );

    println!();

    if resources.is_empty() {
        println!(
            "No hay recursos utilizables."
        );

        return Ok(());
    }

    print_resource_summary(
        &resources
    );

    println!();

    let load_start =
        Instant::now();

    let mut manager =
        MemoryManager::default();

    for resource in &resources {
        for chunk in
            resource
                .data
                .chunks(BLOCK_SIZE)
        {
            manager.insert(
                &resource.workspace,
                resource.category,
                chunk,
            )?;
        }
    }

    let load_time =
        load_start.elapsed();

    print_manager_summary(
        &manager
    );

    println!(
        "Tiempo de carga:       {:.2} ms",
        load_time
            .as_secs_f64()
            * 1000.0
    );

    println!();

    println!(
        "=== Ejecutando simulación en modo {} ===",
        mode.codec_label()
    );

    let simulation_start =
        Instant::now();

    let simulation =
        run_simulation(
            &mut manager,
            mode,
        )?;

    let simulation_time =
        simulation_start.elapsed();

    println!();

    print_simulation(
        &manager,
        &simulation,
        mode,
    );

    println!();

    println!(
        "Tiempo simulación:     {:.2} ms",
        simulation_time
            .as_secs_f64()
            * 1000.0
    );

    println!();

    print_temperature_summary(
        &manager
    );

    println!();

    print_codec_summary(
        &manager
    );

    println!();

    print_category_summary(
        &manager
    );

    println!();

    println!(
        "=== Verificación final ==="
    );

    let verification_start =
        Instant::now();

    manager.verify_all()?;

    let verification_time =
        verification_start.elapsed();

    println!(
        "BLAKE3:                OK"
    );

    println!(
        "Bloques verificados:   {}",
        manager.block_count()
    );

    println!(
        "Tiempo:                {:.2} ms",
        verification_time
            .as_secs_f64()
            * 1000.0
    );

    Ok(())
}

//
// Utilidades
//

fn throughput_mib(
    bytes: usize,
    duration: Duration,
) -> f64 {
    let seconds =
        duration.as_secs_f64();

    if seconds <= f64::EPSILON {
        return 0.0;
    }

    bytes as f64
        / 1024.0
        / 1024.0
        / seconds
}

fn saved_percent(
    original: usize,
    result: usize,
) -> f64 {
    if original == 0 {
        return 0.0;
    }

    (
        1.0
        - result as f64
            / original as f64
    ) * 100.0
}

fn format_bytes(
    bytes: usize,
) -> String {
    const KIB: f64 =
        1024.0;

    const MIB: f64 =
        1024.0 * 1024.0;

    const GIB: f64 =
        1024.0 * 1024.0 * 1024.0;

    let value =
        bytes as f64;

    if value >= GIB {
        format!(
            "{:.2} GiB",
            value / GIB
        )
    } else if value >= MIB {
        format!(
            "{:.2} MiB",
            value / MIB
        )
    } else if value >= KIB {
        format!(
            "{:.2} KiB",
            value / KIB
        )
    } else {
        format!(
            "{} B",
            bytes
        )
    }
}

fn truncate(
    value: &str,
    maximum: usize,
) -> String {
    if value.chars().count()
        <= maximum
    {
        return value.to_string();
    }

    let shortened: String =
        value
            .chars()
            .take(
                maximum
                    .saturating_sub(3)
            )
            .collect();

    format!(
        "{}...",
        shortened
    )
}