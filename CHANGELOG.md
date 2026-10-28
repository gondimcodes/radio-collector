# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.3.1] - 2026-10-27

### Fixed
- **Atomic Deletion in SQLite and Filesystem (`Database::delete_track`)**:
  - Fixed a logic bug where `Database::delete_track` was passing an overly normalized title string (with hyphens and parentheses stripped) to `WHERE LOWER(titulo) = ?1`, causing 0 records to be deleted from SQLite.
  - Fixed physical file deletion where `Database::normalize(&fname)` converted the file extension dot `.` into a space, permanently failing the `.ends_with(".mp3")` check and leaving the physical MP3 file untouched on disk.
  - Refactored `delete_track` and `delete_track_record_only` to perform exact case-insensitive matches (`LOWER(titulo) = LOWER(?1) OR titulo = ?1`) with normalized fallback, and to inspect physical file stems and extensions accurately via `path.extension()` and `path.file_stem()`.
  - Fixed filesystem deduplication check in `Database::is_already_downloaded` to properly check `.mp3` extension on paths instead of normalized filenames.
- **TUI Header and Footer Synchronization**:
  - Ensured that deleting a track via `[d]` immediately updates both the in-memory/table count (header) and the physical on-disk atomic counter (footer `Total MP3`) in lockstep.
- **Automated Regression Tests**:
  - Added dedicated unit test `test_delete_track_and_physical_file` in [`src/db.rs`](file:///home/gondim/projetos/radio-collector/src/db.rs) validating concurrent SQLite row removal and physical file unlinking.

---

## [1.3.0] - 2026-10-27

### Added
- **Interactive Track History Search and Real-Time Filter in TUI**:
  - Added dedicated `'/'` or `'f'` / `'F'` shortcut in TUI to open an interactive search modal for querying tracks by artist or title.
  - Instant live filtering as you type, using accent- and case-insensitive normalization (`Database::normalize`).
  - Seamless integration with table navigation (`↑`/`↓`, `PageUp`/`PageDown`, `Home`/`End`) and track actions (retry `[t]`, approve `[b]`, delete `[d]`), accurately resolving the underlying track index.
  - Clear filter shortcuts via `'c'` / `'C'` or `'Esc'`.
- **Anti-Album & Anti-Compilation Download Guard (`--match-filter`)**:
  - Integrated active semantic filtering in `yt-dlp` to reject videos matching album/compilation title patterns: `full album`, `complete album`, `álbum completo`, `album completo`, `disco completo`, `compacto completo`, `discografia`, `discography`, `greatest hits`.
  - Added a strict 20-minute physical duration ceiling (`duration <=? 1200`) and live stream exclusion (`!is_live`), preventing multi-hour concert broadcasts or full album rips from being saved as individual songs.
- **YouTube Music Official Catalog Pinpointing (`#songs` Protobuf Filter)**:
  - Enforced the canonical `&sp=EgWKAQIIAWoKEAoQAxAEEAkQBQ%3D%3D` Protobuf filter on YouTube Music queries, restricting results exclusively to the "Songs" section (official single master tracks / Art Tracks).
- **Playlist and Collection Expansion Prevention (`--no-playlist`)**:
  - Added `--no-playlist` to prevent `yt-dlp` from entering matching albums or community playlists and downloading the opening track.
- **4-Stage Resilient Search Pipeline**:
  - Upgraded download pipeline to a 4-tier fallback: YouTube Music `#songs` $\to$ YouTube Music broad search with match filter $\to$ `ytsearch5:{} official audio` $\to$ `ytsearch5:{}` broad search.
- **Automated Regression Tests**:
  - Added `test_forbidden_album_titles_regex` in `downloader` and `test_tui_filtered_indices` in `tui`.

---

## [1.2.0] - 2026-10-27

### Added
- **Primary YouTube Music Catalog Search**:
  - Integrated direct search via `https://music.youtube.com/search?q=...` prior to standard YouTube search. Bypasses adult/sensitive keyword censorship (e.g. titles with "Making Love") that caused YouTube's standard web search to return 0 items.
- **Interactive Retry (`[t]`) in TUI for Failed and Pending Tracks**:
  - Added dedicated `'t'` / `'T'` shortcut in TUI to immediately re-attempt downloading any selected track marked as `FALHA` or `PENDENTE`.
  - Dispatches an asynchronous download task with the sanitized clean track name, resets the state to `BAIXANDO`, and updates SQLite status and counters atomically upon success (`SUCESSO`) or failure (`FALHA`).
  - Added ability to dismiss/delete failed entries with `'d'` / `Delete`.
- **Full History Pre-Loading & Keyboard Pagination (PageUp / PageDown / Home / End)**:
  - Removed the 100-record cap: `Database::get_all_history` loads all registered tracks from the database so the operator can scroll through the entire catalog from top to bottom.
  - Added `PageUp` and `PageDown` shortcuts jumping 15 rows per keystroke, plus `Home` and `End` to jump directly to the top or bottom of the history.
  - Replaced cyclical wrap-around navigation with strict boundary stops: reaching the bottom or top of the history no longer jumps to the opposite extreme.
- **Full Date & Time Display (`DATA / HORA`)**:
  - Replaced the time-only column with a full timestamp formatted as `DD/MM/YYYY HH:MM:SS`, both for existing database records and real-time stream detections.
- **Multi-Candidate Resilient Search (`ytsearch5:` + `--max-downloads 1`)**:
  - Upgraded YouTube search strategy from single-candidate (`ytsearch1:`) to multi-candidate (`ytsearch5:`): if the top search result is age-restricted (Age-Gate), geo-blocked, or unavailable, `yt-dlp` automatically skips to the next valid studio audio candidate without failing the entire download pipeline.
  - Implemented 3-stage fallback: YouTube Music direct search, followed by `ytsearch5:{} official audio`, and finally broad `ytsearch5:{}`.
- **YouTube API Android Client Emulation (`player_client=android,web`)**:
  - Configured `--extractor-args "youtube:player_client=android,web"` to bypass desktop web age restrictions on audio streaming formats without requiring authentication.
- **Optional YouTube Cookies Support (`cookies_file`)**:
  - Added optional `cookies_file` configuration under `[download]` in `config.toml`, forwarding `--cookies <path>` to `yt-dlp` for full session authentication on strictly age-restricted catalog tracks.
- **Dedicated Track Details Architecture in TUI**:
  - Decoupled `title` from runtime error messages (`details: Option<String>`), preventing error strings from corrupting track titles during retries or re-downloads.

### Fixed
- **yt-dlp Exit Code 101 Treated as Failure**:
  - Fixed an issue where `yt-dlp`'s standard exit code `101` (denoting successful termination upon hitting `--max-downloads 1`) was treated as a process error by `status.success()`, triggering premature leftover cleanups and deleting downloaded MP3 files.
- **Stuck `BAIXANDO` Status on TUI Retries**:
  - Fixed an IPC race condition where spawned download tasks inside the TUI did not directly signal completion back to the local event channel, leaving retried tracks stuck in `BAIXANDO`.
  - Added direct channel notification (`event_tx.send(CollectorEvent::DownloadFinished)`) and robust case-insensitive/normalized track matching so the TUI immediately transitions retried tracks to `BAIXADO` (green) and decrements `total_failed`.

---

## [1.1.1] - 2026-10-25

### Added
- **Native TLS/HTTPS Stream Support**:
  - Integrated `tokio-rustls`, `rustls` (using `ring`), and `webpki-roots` for secure, asynchronous TLS connections to ICY streams without external C OpenSSL dependencies.
  - Automatically identifies `https://` schemas and defaults to port 443 with TLS encryption.
- **HTTP 3xx Redirection Handling**:
  - Implemented automatic redirection resolution (HTTP 301, 302, 303, 307, 308) with a safety cap of 5 hops.
  - Supports modern radio CDNs and cloud aggregators (e.g., Zeno.FM, Surfer Network) that balance traffic via dynamic HTTP 302 redirects with query token authentication.

---

## [1.1.0] - 2026-10-25

### Added
- **Manual / Supervised Collection Mode (`auto_download = false`)**:
  - Added `auto_download` boolean configuration under `[download]` in `config.toml` (defaults to `true`).
  - When disabled (`auto_download = false`), detected tracks that pass filters and deduplication are tagged with status `PENDENTE` (cyan) in SQLite and TUI.
  - Interactive Action Modal for pending tracks: selecting a `PENDENTE` track allows the operator to approve (`[B]` / `[Enter]` to trigger high-fidelity studio download) or reject (`[R]` / `[Delete]` to discard from the queue without touching disk).

---

## [1.0.1] - 2026-10-25

### Added
- **Inter-Process Event Bus for TUI & Daemon Synchronization**:
  - Implemented lightweight SQLite event queue table (`eventos_monitor`) enabling bidirectional event sharing between the background systemd daemon and interactive TUI monitor sessions.
  - Daemon now commits real-time operational events (`CACHE`, `IGNORADO`, `DOWNLOAD_START`, `DOWNLOAD_FINISH`) as they occur.
  - TUI observer automatically ingests new daemon events with sub-second latency (500ms polling), ensuring `Cache`, `Ignoradas`, and `BAIXANDO` progress states update faithfully across distinct processes.
- **Discrete State Transition for Downloads in TUI**:
  - Simplified download visualization: active downloads appear with status `BAIXANDO` (highlighted in bold yellow) without an artificial progress gauge or percentage label.
  - Upon completion, the entry cleanly transitions to `BAIXADO` (green) with its accurate download time or `FALHA` (red) with error details.
- **Real-Time Deduplication & Ignored Track Tracking in Monitor Mode**:
  - `run_observer` now runs `TrackFilter` and `Database::is_processed` on every detected ICY stream title.
  - Songs already present in the collection trigger `CollectorEvent::TrackCached`, highlighting them as `CACHE` in the TUI table and incrementing the footer cache counter in real time even while the systemd daemon is handling downloads.
  - Radio commercials and station promos trigger `CollectorEvent::TrackIgnored`, dynamically incrementing the `Ignoradas` counter.

### Fixed
- **Accent & Conjunction Invariant String Normalization (Deduplication)**:
  - Upgraded `Database::normalize` to strip Latin diacritics/accents (`á`, `é`, `í`, `ó`, `ú`, `ç`, etc.) and map conjunctions (`&` $\leftrightarrow$ `e`).
  - Solved database duplicate key insertions caused by station metadata variations (e.g., `"Kleiton e Kledir"` vs `"Kleiton & Kledir"` or `"Emilio"` vs `"Emílio"`).

### Security
- **[SEC-01] Safe Substring Match in File Integrity & Cleanup (CWE-73)**:
  - Enforced simultaneous Artista AND Faixa match (`norm_fname.contains(&artist) && norm_fname.contains(&track)`) across database verification (`is_processed`, `reconcile_with_disk`, `delete_track`) and downloader cleanup (`cleanup_leftovers`).
  - Completely eliminated risk of mass accidental deletion or false-positive deduplication collisions for short single-word song titles (e.g., "Love", "You", "One").
- **[SEC-02] Process Termination on Timeout (CWE-400)**:
  - Configured `cmd.kill_on_drop(true)` on `tokio::process::Command` in `downloader.rs`.
  - Ensures immediate SIGKILL delivery to hung `yt-dlp` or `ffmpeg` child processes if the 180s timeout fires.
- **[SEC-03] ICY Metadata Size Upper Bound (CWE-400)**:
  - Enforced strict upper bound of 256 KB on `icy-metaint` in `icy.rs`.
  - Rejects malicious or corrupted radio server headers that attempt memory exhaustion / DoS crashes.
- **[SEC-04] Restrictive PID Lockfile Location (CWE-377)**:
  - Hardened worker lockfile protocol to prioritize `/var/lib/radio-collector/worker.pid` (restricted directory, 0750) over shared world-writable `/tmp/`.
- **[SEC-05] Filename Length Truncation Barrier (CWE-1284)**:
  - Truncated generated filenames to a maximum of 150 characters in `downloader.rs` before invoking `yt-dlp`.
  - Eliminates filesystem `ENAMETOOLONG` errors under Linux ext4/btrfs 255-byte limits.

---

## [1.0.0] - 2026-10-25

### Added
- **Core Architecture & High-Fidelity Downloader**:
  - Implemented asynchronous stream monitor in **Rust (2021 Edition)** via Tokio runtime.
  - Zero-bandwidth stream listener: connects to Icecast/SHOUTcast endpoints, parses ICY metadata (`StreamTitle`), and terminates socket immediately (~1 KB per check).
  - High-fidelity studio download engine powered by `yt-dlp` targeting 320 kbps MP3 (`-x --audio-format mp3 --audio-quality 0`) directly from official catalog releases.
  - Embedded ID3v2 tagging and album cover injection (`--embed-metadata`, `--embed-thumbnail`).
  - Added `--convert-thumbnails jpg` to automatically transcode YouTube's native WebP images to JPEG before ID3 injection, preventing `ffprobe` post-processing crashes.
- **Interactive Terminal User Interface (TUI)**:
  - Full-screen dashboard built with `ratatui 0.29` and `crossterm 0.28`.
  - Real-time download progress bar with live percentage gauge, download speed, and ETA.
  - Interactive history table with cursor navigation (`↑`/`↓` and `k`/`j`) and active row indicator (`▶`).
  - Interactive deletion with floating confirmation modal (`d`/`Delete`): prompts user with `[S]` to confirm and cleanly removes track from SQLite and disk.
  - Real-time footer stats with atomic filesystem verification (`Total no Acervo (MP3)`).
- **Persistent Storage & Self-Healing Deduplication**:
  - Embedded SQLite database via `rusqlite 0.32 (bundled)` configured with WAL mode (`PRAGMA journal_mode = WAL`) and thread-safe Send + Sync mutexes.
  - Startup filesystem reconciliation routine (`reconcile_with_disk`): scans destination folder and automatically promotes or repairs database records for valid MP3s ($\ge 500\text{ KB}$).
  - 2-tier deduplication checking SQLite history first and local disk files second.
- **Dynamic Leader Election & Dual-Mode Coordination**:
  - Implemented autonomous worker lockfile protocol (`/tmp/radio-collector-worker.pid`).
  - Systemd daemon (`radio-collector --daemon`) operates 24/7 as master downloader.
  - Interactive TUI (`radio-collector`) automatically switches to **Monitor Mode** when the daemon is running, eliminating duplicate downloads and race conditions.
  - Dynamic handover: if the daemon is started while the TUI is running standalone, the TUI yields worker leadership and transitions to Monitor Mode in real time.
- **Packaging, Service & Documentation**:
  - Dedicated systemd unit file (`radio-collector.service`) with `ProtectHome=false` to permit downloads under `/home/...`.
  - Comprehensive documentation suite: `README.md`, `README.pt-BR.md`, `INSTALL.md`, `INSTALL.pt-BR.md`, `TECH_STACK.md`, `DEVELOPMENT_PHILOSOPHY.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, and `LICENSE` (GPLv3).

### Fixed
- **Streamripper / ICY 403 Forbidden**: Injected VLC User-Agent (`VLC/3.0.18 LibVLC/3.0.18`) to bypass radio stream connection drops.
- **Lossy Character Sequences in ICY Headers**: Resolved UTF-8 decode panics on Latin-1/ISO-8859-1 stream bytes using `read_until(b'\n')` with `String::from_utf8_lossy`.
- **Systemd Write Permissions**: Fixed `ProtectHome=read-only` error 1 in `radio-collector.service` by setting `ProtectHome=false`.
- **Orphan File Leaks**: Added physical cleanup barrier in `downloader.rs` to purge intermediate `.webm`, `.webp`, `.m4a`, and `.part` files immediately upon download errors.
- **Race Condition on Concurrent Runs**: Prevented filesystem collisions between concurrent TUI and systemd daemon processes using the PID lock protocol.
