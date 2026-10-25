# Technology Stack & Architecture — Radio Collector

This document provides a comprehensive overview of the technology stack, external crates, system libraries, and architectural decisions powering **Radio Collector** (v1.0.0).

---

## 1. Core Language & Async Runtime

* **Language**: **Rust (2021 Edition)**
  * **Rationale**: Guarantees memory safety and zero data races during high-concurrency stream polling and background downloads. Delivers sub-millisecond execution times and minimal CPU/memory footprint (~15–30 MB resident memory).
* **Async Runtime**: `tokio 1.x` (`features = ["full"]`)
  * **Usage**: Non-blocking asynchronous TCP stream polling (`tokio::net::TcpStream`), process orchestration (`tokio::process::Command`), bounded channels (`tokio::sync::mpsc`) for cross-thread event distribution, and structured timeouts (`tokio::time::timeout`).

---

## 2. Interactive Terminal User Interface (TUI)

* **TUI Rendering Engine**: `ratatui 0.29`
  * **Usage**: Terminal dashboard layout orchestration, real-time download progress gauges, dynamic split constraints, styled history table with row highlighting (`▶`), floating confirmation modal dialogs, and auto-updating footer stats.
* **Terminal Control & Event Stream**: `crossterm 0.28` (`features = ["event-stream"]`)
  * **Usage**: Raw terminal mode initialization, alternate screen buffer switching, and non-blocking asynchronous keyboard event handling (`↑`/`↓`, `k`/`j`, `d`/`Delete`, `q`/`Esc`).

---

## 3. Persistent Storage, Deduplication & Atomic Reconciliation

* **Storage Engine**: **SQLite 3** via `rusqlite 0.32` (`features = ["bundled"]`)
  * **Concurrency & Safety**: WAL mode (`PRAGMA journal_mode = WAL`) and normal synchronization (`PRAGMA synchronous = NORMAL`) wrapped in `std::sync::Mutex<Connection>` to guarantee thread-safe Send + Sync execution across async Tokio tasks.
  * **Deduplication Engine**: Case-insensitive and whitespace-stripped indexing on track titles (`LOWER(titulo)`).
  * **Physical Filesystem Reconciliation (`reconcile_with_disk`)**: Self-healing routine executed on startup. Scans the destination directory (`output_dir`) and automatically reconciles valid `.mp3` files ($\ge 500\text{ KB}$) into SQLite with `SUCESSO` status, resolving discrepancies between filesystem state and database logs.

---

## 4. Audio Pipeline & Media Transcoding Engine

* **Download Orchestrator**: `yt-dlp` (invoked via `tokio::process::Command`)
  * **Audio Transcoding**: Direct extraction and conversion into high-fidelity studio MP3 at 320 kbps VBR (`-x --audio-format mp3 --audio-quality 0`).
  * **ID3 Tag & Artwork Injection**: Injects ID3v2 tags (artist, song title, album) and embeds high-resolution album artwork.
  * **WebP-to-JPEG Thumbnail Transcoding**: Invokes `--convert-thumbnails jpg` to guarantee that YouTube's native `.webp` artwork is transcoded into JPEG before MP3 `APIC` frame insertion, eliminating `ffprobe` post-processing crashes.
  * **Orphan Cleanup Barrier**: Automatically purges intermediate or incomplete files (`.webm`, `.webp`, `.m4a`, `.part`, `.ytdl`) immediately if a download operation fails or is interrupted.
* **JavaScript Engine**: `nodejs`
  * **Usage**: Required external JS runtime detected by the downloader to solve YouTube EJS (External JavaScript) challenge tokens and decipher n-sig signatures without format degradation.
* **Media Inspection**: `ffmpeg` / `ffprobe`
  * **Usage**: Audio stream demuxing, bitrate validation, and artwork container encoding.

---

## 5. Radio Stream Networking & ICY Protocol Client

* **Custom Client**: Internal module `src/icy.rs`
  * **Zero Bandwidth Waste**: Connects directly to SHOUTcast/Icecast HTTP endpoints with `Icy-MetaData: 1`, extracts the `icy-metaint` interval, captures the stream metadata chunk, and closes the socket immediately (~1 KB bandwidth per check).
  * **Encoding Resilience**: Handles both UTF-8 and Latin-1/ISO-8859-1 character sequences using raw byte buffer inspection (`read_until(b'\n')`) and lossy UTF-8 decoding (`String::from_utf8_lossy`).
  * **Bitrate Parsing**: Captures and exposes streaming bitrate headers (`icy-br`).

---

## 6. Command Line Interface, Filtering & Configuration

* **Argument Parsing**: `clap 4.x` (`features = ["derive"]`)
  * **Usage**: Declarative CLI parsing supporting custom configuration files (`-c`, `--config`) and background daemon execution (`-d`, `--daemon`).
* **Configuration Deserialization**: `toml 0.8` and `serde 1.0` (`features = ["derive"]`)
  * **Usage**: Strongly-typed schema parser for `/etc/radio-collector/config.toml`.
* **Station Filter Engine**: `regex 1.x`
  * **Usage**: Regex pattern matching to discard station jingles, radio promotional ads, and malformed track titles.
* **Timestamps & Localization**: `chrono 0.4` (`features = ["serde"]`)
  * **Usage**: Local system timezone resolution (`localtime`) for SQLite timestamps and TUI event logs.

---

## 7. Process Lifecycle & Dynamic Worker Leader Election

* **Lockfile Protocol**: `/tmp/radio-collector-worker.pid` (`<pid>:<role>`)
  * **Autonomous Role Negotiation**:
    * **Systemd Daemon (`role: daemon`)**: Holds unconditional master authority for 24/7 background downloading.
    * **Interactive TUI (`role: tui` / `monitor`)**: If the daemon is already running, the TUI starts in lightweight **Monitor Mode** (observing stream and SQLite changes without executing duplicate downloads). If the daemon starts while the TUI is running as the worker, the TUI dynamically yields worker authority and transitions into Monitor Mode seamlessly.
* **Graceful Shutdown**: Captures system signals (`SIGINT` / `SIGTERM`) to release locks and terminate child `yt-dlp` processes cleanly.
