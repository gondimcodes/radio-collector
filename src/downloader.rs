use crate::config::DownloadConfig;
use crate::db::Database;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

pub struct Downloader {
    cfg: DownloadConfig,
    dest_dir: String,
}

impl Downloader {
    pub fn new(cfg: &DownloadConfig, dest_dir: &str) -> Self {
        Self {
            cfg: cfg.clone(),
            dest_dir: dest_dir.to_string(),
        }
    }

    pub async fn download(
        &self,
        title: &str,
    ) -> Result<()> {
        let timeout_dur = Duration::from_secs(self.cfg.timeout_seconds);
        timeout(timeout_dur, self.execute_download(title))
            .await
            .context("Tempo limite de download excedido")?
    }

    async fn execute_download(
        &self,
        title: &str,
    ) -> Result<()> {
        // Sanitiza e trunca o título para nome de arquivo físico padronizado e seguro (CWE-1284)
        let safe_title = title
            .replace(['/', '\\'], "-")
            .replace('\0', "")
            .replace('"', "'");
        let safe_title: String = safe_title.trim().chars().take(150).collect();
        let safe_title = safe_title.trim();

        let output_template = format!(
            "{}/{}.%(ext)s",
            self.dest_dir.trim_end_matches('/'),
            safe_title
        );

        let search_query = format!("ytsearch1:{} official audio", title);

        let mut cmd = Command::new(&self.cfg.ytdlp_path);
        // Previne processos zumbis do yt-dlp em caso de timeout ou descarte da future (CWE-400)
        cmd.kill_on_drop(true);

        cmd.arg("-x")
            .arg("--audio-format")
            .arg(&self.cfg.audio_format)
            .arg("--audio-quality")
            .arg(&self.cfg.audio_quality)
            .arg("--no-playlist")
            .arg("-o")
            .arg(&output_template);

        if self.cfg.embed_metadata {
            cmd.arg("--embed-metadata");
        }
        if self.cfg.embed_thumbnail {
            cmd.arg("--embed-thumbnail");
            // Converte thumbnails WebP para JPG nativo, evitando quebra do ffprobe/ID3 no MP3
            cmd.arg("--convert-thumbnails").arg("jpg");
        }

        // Auto-detecta JavaScript runtime suportado (Deno ou Node.js) para decifrar assinaturas do YouTube
        if Path::new("/usr/bin/deno").exists() || Path::new("/usr/local/bin/deno").exists() {
            cmd.arg("--js-runtimes").arg("deno");
        } else if Path::new("/usr/bin/node").exists() || Path::new("/usr/local/bin/node").exists() {
            cmd.arg("--js-runtimes").arg("node");
        }

        cmd.arg(&search_query);

        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Falha ao executar yt-dlp em '{}'", self.cfg.ytdlp_path))?;

        let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(stderr) = child.stderr.take() {
            let err_lines = Arc::clone(&stderr_lines);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut lock = err_lines.lock().await;
                    if lock.len() < 50 {
                        lock.push(line);
                    }
                }
            });
        }

        let status = child.wait().await?;
        if !status.success() {
            // Remove imediatamente sobras parciais (.webm, .webp, .part) em caso de falha
            Self::cleanup_leftovers(title, &self.dest_dir);

            let lines = stderr_lines.lock().await.clone();
            // 1. Prioriza linhas contendo erro real (yt-dlp e ffmpeg usam "ERROR:")
            let errors: Vec<String> = lines
                .iter()
                .filter(|l| l.contains("ERROR:") || l.contains("Error:"))
                .cloned()
                .collect();

            let msg = if !errors.is_empty() {
                errors.join(" | ")
            } else {
                // 2. Se não houver ERROR explícito, descarta WARNINGs inofensivos
                let non_warnings: Vec<String> = lines
                    .into_iter()
                    .filter(|l| !l.starts_with("WARNING:") && !l.trim().is_empty())
                    .collect();

                if !non_warnings.is_empty() {
                    non_warnings.join(" | ")
                } else {
                    format!("Código de saída: {:?}", status.code())
                }
            };
            bail!("yt-dlp erro: {}", msg.trim());
        }

        // 3. Verificação Estrita de Integridade no Disco (Barreira Atômica)
        Self::verify_file_integrity(title, &self.dest_dir)?;

        Ok(())
    }

    fn cleanup_leftovers(title: &str, dest_dir: &str) {
        let parts: Vec<&str> = title.splitn(2, '-').collect();
        let (artist_part, track_part) = if parts.len() == 2 {
            (Database::normalize(parts[0]), Database::normalize(parts[1]))
        } else {
            (String::new(), Database::normalize(title))
        };
        let norm_full_title = Database::normalize(title);

        let dest_path = Path::new(dest_dir);
        if let Ok(entries) = fs::read_dir(dest_path) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let lower = fname.to_lowercase();
                let norm_fname = Database::normalize(&fname);
                let is_match = (!artist_part.is_empty()
                    && norm_fname.contains(&artist_part)
                    && norm_fname.contains(&track_part))
                    || norm_fname.contains(&norm_full_title);

                if (lower.ends_with(".webm")
                    || lower.ends_with(".webp")
                    || lower.ends_with(".m4a")
                    || lower.ends_with(".part")
                    || lower.ends_with(".ytdl"))
                    && is_match
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    fn verify_file_integrity(title: &str, dest_dir: &str) -> Result<()> {
        let parts: Vec<&str> = title.splitn(2, '-').collect();
        let (artist_part, track_part) = if parts.len() == 2 {
            (Database::normalize(parts[0]), Database::normalize(parts[1]))
        } else {
            (String::new(), Database::normalize(title))
        };
        let norm_full_title = Database::normalize(title);

        let dest_path = Path::new(dest_dir);
        if !dest_path.exists() {
            bail!("Diretório de destino não existe: {}", dest_dir);
        }

        // Limpeza de arquivos temporários (.part ou .ytdl) que possam ter ficado
        if let Ok(entries) = fs::read_dir(dest_path) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let lower = fname.to_lowercase();
                let norm_fname = Database::normalize(&fname);
                let is_match = (!artist_part.is_empty()
                    && norm_fname.contains(&artist_part)
                    && norm_fname.contains(&track_part))
                    || norm_fname.contains(&norm_full_title);

                if (lower.ends_with(".part") || lower.ends_with(".ytdl")) && is_match {
                    let _ = fs::remove_file(entry.path());
                    bail!("Download incompleto (arquivo temporário .part detectado e removido)");
                }
            }
        }

        // Validação física de tamanho mínimo (áudio MP3 real deve ter >= 500 KB)
        const MIN_VALID_MP3_SIZE: u64 = 500 * 1024;
        let mut found_valid = false;

        if let Ok(entries) = fs::read_dir(dest_path) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let norm_fname = Database::normalize(&fname);
                let is_match = (!artist_part.is_empty()
                    && norm_fname.contains(&artist_part)
                    && norm_fname.contains(&track_part))
                    || norm_fname.contains(&norm_full_title);

                if fname.to_lowercase().ends_with(".mp3") && is_match {
                    if let Ok(meta) = entry.metadata() {
                        if meta.len() >= MIN_VALID_MP3_SIZE {
                            found_valid = true;
                            break;
                        } else {
                            let _ = fs::remove_file(entry.path());
                            bail!(
                                "Arquivo MP3 corrompido ou truncado (< 500 KB, tamanho: {} bytes)",
                                meta.len()
                            );
                        }
                    }
                }
            }
        }

        if !found_valid {
            // Remove sobras de formatos intermediários ou incompletos (.webm, .webp, .m4a, .part)
            Self::cleanup_leftovers(title, dest_dir);
            bail!("Nenhum arquivo MP3 válido correspondente encontrado no disco após o download");
        }

        Ok(())
    }
}
