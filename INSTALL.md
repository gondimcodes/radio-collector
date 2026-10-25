# Installation Guide: Radio Collector (Debian 13)

*Read this in [Português (Brasil)](INSTALL.pt-BR.md).*

This guide covers building, configuring, and installing **Radio Collector** (Rust + TUI) on **Debian 13 (Trixie)** as both a standalone native binary and a systemd background daemon.

---

## 1. Operating System Dependencies

Install Rust build dependencies, `ffmpeg`, the `nodejs` JavaScript runtime (required by modern `yt-dlp` for YouTube signature deciphering), and common utilities:

```bash
apt update && apt upgrade -y
apt install -y build-essential pkg-config libssl-dev libsqlite3-dev \
               ffmpeg nodejs curl wget git
```

> [!IMPORTANT]
> **JavaScript Runtime (`nodejs`)**: YouTube has implemented external JavaScript challenges (EJS / n-sig) across its audio streams. Without a supported JS runtime like `nodejs` installed, `yt-dlp` loses access to high-fidelity formats and crashes during post-processing with `ffprobe unable to obtain file audio codec`. The Debian package `nodejs` satisfies this requirement natively.

### Install Official Rust Toolchain (if not already installed):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustc --version
```

### Install Official `yt-dlp` Binary:
```bash
wget https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -O /usr/local/bin/yt-dlp
chmod a+rx /usr/local/bin/yt-dlp
yt-dlp --version
```

---

## 2. Compiling the Rust Binary

Navigate to the `radio-collector` project directory:

```bash
cd /path/to/radio-collector

# Compile with maximum release optimizations
cargo build --release
```

The optimized, self-contained binary will be generated at:
`target/release/radio-collector`

Install the binary to the system PATH:
```bash
install -m 0755 target/release/radio-collector /usr/local/bin/radio-collector
```

---

## 3. Directory Layout and Configuration

Create the configuration, database, and audio destination directories:

```bash
mkdir -p /etc/radio-collector
mkdir -p /var/lib/radio-collector
mkdir -p /var/musicas
```

Copy the sample configuration file:

```bash
cp config.toml /etc/radio-collector/config.toml
chmod 0644 /etc/radio-collector/config.toml
```

### Customize settings in `/etc/radio-collector/config.toml`:
* `stream.url`: URL of the Icecast/SHOUTcast stream to monitor.
* `storage.output_dir`: Path where MP3 files will be stored (defaults to `/var/musicas`).
* `filter.ignore_patterns`: List of regex strings used to drop station jingles and ads.

---

## 4. Setting up the Background Service (Systemd)

To run the collector autonomously 24/7 in the background:

1. Copy the systemd service unit:
```bash
cp radio-collector.service /etc/systemd/system/radio-collector.service
chmod 0644 /etc/systemd/system/radio-collector.service
```

> [!NOTE]
> **Filesystem Permissions under `/home/` (`ProtectHome=false`)**: If your `output_dir` resides within a user directory under `/home/` (e.g., `/home/gondim/Músicas/kakarecos`), ensure that `ProtectHome=false` is set in `/etc/systemd/system/radio-collector.service`. Otherwise, systemd will prevent `yt-dlp` from writing files to disk.

2. Reload systemd and enable the service:
```bash
systemctl daemon-reload
systemctl enable --now radio-collector.service
```

3. Follow real-time daemon logs:
```bash
journalctl -u radio-collector.service -f
```

---

## 5. Interactive Terminal UI (TUI)

You can launch the interactive interface at any time, even while the `systemd` service is actively running:

```bash
radio-collector
```

* **When the systemd service is active**: The TUI automatically enters **Monitor Mode**, observing downloads in real time without conflicting with the background daemon.
* **When the service is stopped**: The TUI runs as an independent collector and downloads tracks directly.
* **Navigation**: `↑`/`↓` arrows or `k`/`j` keys.
* **Safe Deletion**: Select a track and press `d` or `Delete` (opens a confirmation modal to remove it from both SQLite and the disk).
* **Quit**: Press `q`.

---

## 6. Automated `yt-dlp` Extractor Updates

YouTube frequently modifies its internal streaming signatures. Maintain `yt-dlp` automatically via a weekly `crontab` job:

```bash
(crontab -l 2>/dev/null; echo "0 4 * * 0 /usr/local/bin/yt-dlp -U >/dev/null 2>&1") | crontab -
```
