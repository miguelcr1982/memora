# Monster Memory Engine 👹

An experimental memory manager written in Rust that explores **cross-workspace deduplication** and **user-selectable in-memory compression** for web browser resources.

The long-term goal is to investigate whether Chromium-based browsers can reduce memory usage by combining:

- Content deduplication
- Shared immutable resources across isolated workspaces
- LZ4 or Zstd in-memory compression
- Hot / warm / cold resource tracking
- On-demand decompression
- Independent workspace isolation

> [!IMPORTANT]
> This project is currently a proof of concept.
>
> It is **not yet integrated with Chromium**, and the memory savings shown below refer to the web-resource dataset processed by the prototype, not total browser process memory.

> This is a **human-driven, AI-assisted** experimental project. Generative AI has been used during architecture exploration, Rust prototyping, refactoring, benchmarking discussions, and documentation. Technical decisions, execution, testing, and validation remain human-driven.

---

## Why?

Modern browsers isolate tabs, sites, and renderer processes for good reasons: security, stability, and fault isolation.

However, multiple browser workspaces may load identical resources such as:

- JavaScript bundles
- CSS
- Fonts
- Images
- JSON
- HTML
- WebAssembly

Monster Memory Engine explores a simple question:

> Can isolated browser workspaces share identical immutable resources while their private execution state remains isolated?

The second question is:

> Can inactive resources remain in compressed RAM and be restored quickly when needed?

Conceptually:

```text
          Workspace A
              │
              ├──────────┐
              │          │
          Workspace B    │
              │          ▼
              ├──► Shared Resource Store
              │          │
          Workspace C    ├── BLAKE3 deduplication
                         ├── reference tracking
                         └── compressed storage
```

Workspace-specific state remains independent:

```text
Workspace A
├── private state
├── session
├── runtime
└── references ──────┐

Workspace B          │
├── private state    │
├── session          ├──► Shared immutable resources
├── runtime          │
└── references ──────┘
```

---

# Memory modes

The user chooses the memory strategy based on the hardware available.

## RAM

```text
Deduplication: ON
Compression:   OFF
Storage:       RAW
```

Designed for systems where memory capacity is not a concern and maximum responsiveness is preferred.

The shared resource store still performs deduplication.

## LZ4

```text
Deduplication: ON
Compression:   LZ4
```

Inactive blocks are compressed using LZ4.

The goal is to significantly reduce memory consumption while keeping decompression latency low.

## Zstd

```text
Deduplication: ON
Compression:   Zstd
```

Designed for systems where memory reduction is more important and additional CPU work is acceptable.

The compression level can also be selected.

Example:

```bash
cargo run --release -- zstd captures 5
```

---

# Architecture

```text
                         Web resources
                              │
                              ▼
                       64 KiB blocks
                              │
                              ▼
                         BLAKE3 hash
                              │
                              ▼
                    Byte-level verification
                              │
                              ▼
                  ┌──────────────────────┐
                  │ Shared Resource Store│
                  └──────────┬───────────┘
                             │
                   Reference tracking
                             │
               ┌─────────────┼─────────────┐
               │             │             │
              HOT           WARM          COLD
               │             │             │
               └─────────────┴─────────────┘
                             │
                  User-selected mode
                             │
             ┌───────────────┼───────────────┐
             ▼               ▼               ▼
            RAM             LZ4             Zstd
            RAW          compressed       compressed
```

Temperature and physical storage are intentionally separate concepts.

A block can therefore be:

```text
Temperature: COLD
Storage:     RAW
```

For example, this happens when compression is disabled or when compressing the block would not reduce its size.

---

# Deduplication

Resources are split into 64 KiB blocks.

Each block is hashed using BLAKE3:

```text
resource
   │
   ▼
64 KiB blocks
   │
   ▼
BLAKE3
   │
   ▼
candidate lookup
   │
   ▼
byte comparison
   │
   ├── identical → reuse existing block
   │
   └── different → store new block
```

The hash is **not considered proof of equality**.

Candidate blocks are compared byte-for-byte before deduplication occurs.

---

# Workspace isolation

Every logical browser workspace maintains its own references.

Conceptually:

```text
Workspace A ────────┐
                    │
Workspace B ────────┼──► shared resource block
                    │
Workspace C ────────┘
```

Closing or crashing one workspace should eventually only remove that workspace's references.

Shared resources remain available while other workspaces still reference them.

Full browser-level lifecycle management is not implemented yet.

---

# Thermal model

Blocks maintain an independent logical temperature:

```text
HOT
 │
 │ inactivity
 ▼
WARM
 │
 │ longer inactivity
 ▼
COLD
```

Accessing an inactive block promotes it back to HOT:

```text
COLD
 │
 │ access
 ▼
decompress if necessary
 │
 ▼
verify BLAKE3
 │
 ▼
HOT
```

The current prototype uses logical ticks rather than wall-clock time.

Default experimental thresholds:

```text
WARM after:      50 ticks
COLD after:     200 ticks
Aging interval:  25 ticks
```

These values are experimental and are not recommendations for a production browser.

## Temperature vs storage

Temperature does **not** select the compression algorithm.

The user selects a global memory mode:

```text
RAM
LZ4
Zstd
```

The engine independently decides whether a block is:

```text
HOT
WARM
COLD
```

For example, in LZ4 mode:

```text
HOT       → RAW
WARM      → LZ4 when beneficial
COLD      → LZ4 when beneficial
```

In Zstd mode:

```text
HOT       → RAW
WARM      → Zstd when beneficial
COLD      → Zstd when beneficial
```

And in RAM mode:

```text
HOT       → RAW
WARM      → RAW
COLD      → RAW
```

---

# Compression learning

Not every resource benefits from compression.

Already compressed formats, fonts, images, and other binary data may become larger when compressed again.

Monster Memory Engine therefore tests whether compression is actually useful:

```text
original block
      │
      ▼
   compress
      │
      ▼
compressed < original?
     /               \
   yes                no
    │                  │
 store compressed    keep RAW
                       │
                       ▼
                 remember result
```

Because resource blocks are immutable in the current model, a failed compression attempt does not need to be repeated.

This prevents repeatedly spending CPU on blocks known to be incompressible.

---

# Current benchmark

The current test dataset contains resources captured from several web workspaces via HAR files.

Multimedia responses such as video and audio are excluded from the resource benchmark.

Dataset used during the current experiment:

```text
Logical content:       91.25 MiB
Unique after dedupe:   58.58 MiB
Deduplication saved:   32.67 MiB
Deduplication ratio:   35.81 %

Logical blocks:        1,751
Unique blocks:         1,143
Duplicate blocks:        608
Block size:            64 KiB
```

## Memory mode comparison

Results from the same dataset and deterministic 20,000-access simulation:

| Mode         | Final managed memory | Total saving |
| ------------ | -------------------: | -----------: |
| RAM          |            58.58 MiB |       35.81% |
| LZ4          |            25.71 MiB |       71.83% |
| Zstd level 3 |            20.35 MiB |       77.70% |

The RAM mode still benefits from deduplication but performs no compression.

### Observed access latency

```text
RAW access       ~35-38 ns*
LZ4 restore      ~34 µs
Zstd restore     ~95 µs
```

`*` RAW access measures the overhead of the simulation's direct-access path. It is **not a system memory bandwidth benchmark**.

During the tested workload:

```text
RAM
Final managed memory:  58.58 MiB

LZ4
Final managed memory:  25.71 MiB

Zstd level 3
Final managed memory:  20.35 MiB
```

The results illustrate the intended trade-off:

```text
                CPU / latency     Memory usage

RAM               lowest           highest

LZ4               medium           lower

Zstd              highest          lowest
```

> [!WARNING]
> These numbers must not be interpreted as "Chromium uses 77% less RAM".
>
> The current benchmark operates on captured web-resource bodies. It does not yet manage V8 heaps, Blink DOM state, decoded image surfaces, GPU resources, renderer allocations, or total Chromium process memory.

---

# Real-world resource observations

In the current dataset, text-oriented resources responded particularly well to compression.

Examples include:

- HTML
- JavaScript
- CSS
- JSON

Already compressed or binary resources generally showed considerably smaller gains, including some:

- Images
- Fonts
- Other binary resources

The engine therefore makes compression decisions at block level instead of assuming that every resource should be compressed.

---

# Running the prototype

## Requirements

- Rust
- Cargo
- On Windows: the MSVC Rust toolchain and corresponding C++ build tools

Build:

```bash
cargo build --release
```

---

## Capture directory

Create:

```text
captures/
```

Place HAR files containing response bodies inside it:

```text
captures/
├── workspace-1.har
├── workspace-2.har
├── workspace-3.har
└── ...
```

Each HAR file is treated as an independent logical workspace.

Invalid or truncated HAR files are skipped instead of terminating the complete experiment.

Do **not** commit HAR files captured from private, authenticated, corporate, or sensitive websites.

---

# RAM mode

```bash
cargo run --release -- ram
```

Behavior:

```text
deduplication → yes
compression   → no
storage       → RAW
```

---

# LZ4 mode

```bash
cargo run --release -- lz4
```

Behavior:

```text
deduplication → yes
compression   → LZ4
```

---

# Zstd mode

```bash
cargo run --release -- zstd
```

Default Zstd level:

```text
3
```

Custom level:

```bash
cargo run --release -- zstd captures 5
```

---

# Integrity verification

Every unique block retains its original BLAKE3 hash.

After decompression:

```text
compressed block
       │
       ▼
decompression
       │
       ▼
restored bytes
       │
       ▼
BLAKE3
       │
       ▼
compare with original hash
```

A successful run ends with:

```text
BLAKE3: OK
```

---

# AI-assisted development

Monster Memory Engine is a **human-driven, AI-assisted research and development project**.

Generative AI has been used as a development tool during the evolution of the prototype, including:

- Exploring architectural approaches
- Discussing browser memory-management strategies
- Generating initial Rust implementations
- Refactoring experimental code
- Reviewing implementation ideas
- Designing benchmark experiments
- Analysing benchmark outputs
- Identifying performance bottlenecks
- Improving project documentation

AI-generated suggestions are treated as engineering proposals, not as authoritative technical conclusions.

The development process follows an iterative workflow:

```text
idea
 │
 ▼
AI-assisted exploration
 │
 ▼
implementation
 │
 ▼
compile
 │
 ▼
real execution
 │
 ▼
benchmark
 │
 ▼
analyse results
 │
 ▼
correct / redesign
 │
 └────────────► repeat
```

The project's technical direction, experiments, execution, validation, and final implementation decisions remain human-driven.

Generated or AI-assisted code is reviewed, compiled, executed, and iteratively validated before being incorporated into the prototype.

In particular, claims involving:

- Chromium internals
- V8
- Blink
- Browser security boundaries
- Memory-management behaviour
- Performance characteristics

should be validated experimentally or against authoritative technical documentation before being considered reliable.

AI assistance is part of the development process, not part of the runtime architecture of the current memory engine.

---

# Development history

The prototype evolved incrementally.

```text
v0.1
Synthetic deduplication + LZ4/Zstd benchmark

v0.2
Real JS/CSS/HTML/JSON/image/font resources

v0.3
HAR-based browser resource ingestion

v0.4
Shared resource store and workspace reference analysis

v0.5
HOT/WARM/COLD memory model

v0.6
Dynamic access simulation

v0.6.1
Compression-result learning and optimized HOT access

v0.7
User-selectable RAM/LZ4/Zstd memory modes
Temperature separated from physical storage codec
```

---

# Current limitations

The project currently operates as a **userspace simulation over captured browser resources**.

It does not currently:

- Embed Chromium or CEF
- Intercept browser resources live
- Replace Chromium's memory allocator
- Compress arbitrary V8 heap pages
- Deduplicate mutable DOM/runtime state
- Manage GPU memory
- Provide transparent virtual-memory paging
- Implement true OS-level zram behavior
- Provide copy-on-write memory pages
- Measure complete browser RSS/working set
- Provide production-grade browser security boundaries

The prototype intentionally focuses on validating the memory-management model before attempting deep browser integration.

---

# Roadmap

## Stage 1: Memory engine

- [x] Block-based resource storage
- [x] BLAKE3 deduplication
- [x] Byte-level collision verification
- [x] Cross-workspace references
- [x] HOT / WARM / COLD tracking
- [x] LZ4 compression
- [x] Zstd compression
- [x] Compression usefulness detection
- [x] Compression-result learning
- [x] On-demand decompression
- [x] Integrity verification
- [x] RAM / LZ4 / Zstd user modes
- [x] Dynamic workload simulation

## Stage 2: Chromium browser prototype

- [ ] Embed Chromium/CEF from Rust
- [ ] Basic browser window
- [ ] URL navigation
- [ ] Multiple isolated workspaces
- [ ] Live resource interception
- [ ] Replace HAR input with live browser resources
- [ ] Connect live resources to the shared resource store
- [ ] Expose RAM / LZ4 / Zstd selection through browser settings

## Stage 3: Browser memory research

- [ ] Measure total browser working set
- [ ] Compare against unmodified Chromium
- [ ] Track resource lifetime per workspace
- [ ] Release resources when reference count reaches zero
- [ ] Investigate decoded image sharing
- [ ] Investigate JavaScript/WASM resource sharing
- [ ] Measure CPU cost under realistic browsing workloads
- [ ] Measure workspace restoration latency
- [ ] Benchmark different processor classes

## Stage 4: Deeper Chromium integration

Depending on the results of the CEF prototype:

```text
CEF sufficient
      │
      └── continue using CEF

or

deeper control required
      │
      └── investigate a Chromium fork
```

Deep integration with Blink, V8, Chromium allocators, renderer memory, or GPU resources is intentionally considered a later research stage.

---

# Security model

Deduplicating physical storage must never imply sharing logical permissions.

Two workspaces referencing the same immutable block should still behave as isolated browser contexts.

The intended model is:

```text
Physical storage
      │
      └── may be shared

Logical ownership
      │
      └── remains isolated
```

Future browser integration must preserve Chromium's origin isolation, sandboxing, process isolation, and other security boundaries.

---

# Privacy

HAR files may contain:

- Private URLs
- Request parameters
- Authentication-related information
- API responses
- Internal hostnames
- Personal data
- Corporate resources

For this reason, HAR files should **never be committed by default**.

Recommended `.gitignore`:

```gitignore
/target/
/captures/
*.har

# Editors / OS
.vscode/
.idea/
.DS_Store
Thumbs.db
```

Before publishing changes, check the staged content carefully:

```bash
git status
git diff --cached
```

The repository should contain benchmark descriptions and aggregate measurements rather than private HAR captures.

---

# Project status

**Experimental / Proof of Concept**

The current memory engine demonstrates that:

1. Web-resource blocks can be deduplicated across logical workspaces.
2. Deduplication can coexist with workspace-level reference tracking.
3. RAM mode can retain deduplication without introducing compression.
4. LZ4 can provide a memory/latency trade-off.
5. Zstd can provide more aggressive memory reduction at greater CPU cost.
6. The memory strategy can be selected by the user according to available hardware.
7. Logical resource temperature can remain independent from its physical storage codec.
8. Blocks that do not benefit from compression can remain RAW.
9. Compression failures can be remembered to avoid repeated CPU work.
10. Compressed blocks can be restored and verified successfully using BLAKE3.

The next major milestone is replacing HAR-based simulation with a real Chromium/CEF browser prototype.

---

# Disclaimer

Monster Memory Engine is experimental research software.

Current benchmarks are intended to evaluate the prototype's internal memory-management model. They should not be interpreted as production-browser performance claims.

Results may vary significantly depending on:

- Workload
- Resource types
- Resource duplication
- CPU architecture
- Compression mode
- Zstd compression level
- Block size
- Access patterns

The project should be considered experimental until measurements are performed against an actual Chromium-based browser implementation.

---

# License

A license has not been selected yet.

Before accepting external contributions or distributing binaries, the project should define a license and review the licenses and redistribution requirements of its dependencies and future Chromium/CEF integration.
