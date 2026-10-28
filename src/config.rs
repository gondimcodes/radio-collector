use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub stream: StreamConfig,
    pub storage: StorageConfig,
    pub download: DownloadConfig,
    pub filter: FilterConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamConfig {
    pub url: String,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    #[serde(default = "default_check_interval")]
    pub check_interval_seconds: u64,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub output_dir: String,
    pub db_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadConfig {
    #[serde(default = "default_ytdlp_path")]
    pub ytdlp_path: String,
    #[serde(default = "default_audio_format")]
    pub audio_format: String,
    #[serde(default = "default_audio_quality")]
    pub audio_quality: String,
    #[serde(default = "default_download_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub embed_metadata: bool,
    #[serde(default = "default_true")]
    pub embed_thumbnail: bool,
    #[serde(default = "default_true")]
    pub auto_download: bool,
    #[serde(default)]
    pub cookies_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    #[serde(default = "default_min_length")]
    pub min_title_length: usize,
    #[serde(default = "default_true")]
    pub require_hyphen: bool,
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub tui: bool,
}

fn default_user_agent() -> String {
    "VLC/3.0.18 LibVLC/3.0.18".to_string()
}

fn default_check_interval() -> u64 {
    30
}

fn default_timeout() -> u64 {
    10
}

fn default_ytdlp_path() -> String {
    "/usr/local/bin/yt-dlp".to_string()
}

fn default_audio_format() -> String {
    "mp3".to_string()
}

fn default_audio_quality() -> String {
    "0".to_string()
}

fn default_download_timeout() -> u64 {
    180
}

fn default_min_length() -> usize {
    4
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load(custom_path: Option<&Path>) -> Result<(Self, PathBuf)> {
        let candidate_paths = if let Some(path) = custom_path {
            vec![path.to_path_buf()]
        } else {
            vec![
                PathBuf::from("/etc/radio-collector/config.toml"),
                PathBuf::from("./config.toml"),
            ]
        };

        for path in &candidate_paths {
            if path.exists() {
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("Falha ao ler arquivo de configuração: {:?}", path))?;
                let cfg: Config = toml::from_str(&content)
                    .with_context(|| format!("Falha ao parsear TOML em: {:?}", path))?;
                return Ok((cfg, path.clone()));
            }
        }

        anyhow::bail!(
            "Nenhum arquivo de configuração encontrado. Procurado em: {:?}",
            candidate_paths
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_config_defaults_and_cookies() {
        let toml_str = r#"
            ytdlp_path = "/usr/bin/yt-dlp"
            cookies_file = "/etc/radio-collector/cookies.txt"
        "#;
        let cfg: DownloadConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.ytdlp_path, "/usr/bin/yt-dlp");
        assert_eq!(cfg.audio_format, "mp3");
        assert_eq!(cfg.audio_quality, "0");
        assert!(cfg.embed_metadata);
        assert!(cfg.embed_thumbnail);
        assert!(cfg.auto_download);
        assert_eq!(
            cfg.cookies_file,
            Some("/etc/radio-collector/cookies.txt".to_string())
        );
    }
}
