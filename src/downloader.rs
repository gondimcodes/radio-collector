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

        // Tentativa 1: Busca nativa no catálogo do YouTube Music (sem censura de títulos como 'Making Love')
        let music_query = format!("https://music.youtube.com/search?q={}", Self::urlencode(title));
        if self.run_ytdlp_attempt(title, &music_query, &output_template).await.is_ok() {
            return Ok(());
        }

        // Tentativa 2: Busca multi-candidato priorizando áudio oficial de estúdio
        let primary_query = format!("ytsearch5:{} official audio", title);
        let first_err = match self.run_ytdlp_attempt(title, &primary_query, &output_template).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        // Tentativa 3 (Fallback): Busca multi-candidato ampla (Artista - Música)
        let fallback_query = format!("ytsearch5:{}", title);
        match self.run_ytdlp_attempt(title, &fallback_query, &output_template).await {
            Ok(()) => Ok(()),
            Err(second_err) => {
                let msg1 = first_err.to_string();
                let msg2 = second_err.to_string();
                if msg2.contains("Sign in") || msg2.contains("ERROR:") {
                    bail!("{}", msg2);
                } else if msg1.contains("Sign in") || msg1.contains("ERROR:") {
                    bail!("{}", msg1);
                } else {
                    bail!("Falha em todas as tentativas de busca: {} | {}", msg1, msg2);
                }
            }
        }
    }

    fn urlencode(s: &str) -> String {
        let mut encoded = String::new();
        for b in s.bytes() {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(b as char);
                }
                b' ' => encoded.push('+'),
                _ => {
                    encoded.push_str(&format!("%{:02X}", b));
                }
            }
        }
        encoded
    }

    async fn run_ytdlp_attempt(
        &self,
        title: &str,
        search_query: &str,
        output_template: &str,
    ) -> Result<()> {
        let mut cmd = Command::new(&self.cfg.ytdlp_path);
        // Previne processos zumbis do yt-dlp em caso de timeout ou descarte da future (CWE-400)
        cmd.kill_on_drop(true);

        cmd.arg("-x")
            .arg("--audio-format")
            .arg(&self.cfg.audio_format)
            .arg("--audio-quality")
            .arg(&self.cfg.audio_quality)
            .arg("--max-downloads")
            .arg("1")
            .arg("--no-abort-on-error")
            .arg("--ignore-errors")
            .arg("-o")
            .arg(output_template);

        if self.cfg.embed_metadata {
            cmd.arg("--embed-metadata");
        }
        if self.cfg.embed_thumbnail {
            cmd.arg("--embed-thumbnail");
            // Converte thumbnails WebP para JPG nativo, evitando quebra do ffprobe/ID3 no MP3
            cmd.arg("--convert-thumbnails").arg("jpg");
        }

        // Usa cliente alternativo da API (android,web) para contornar restrições etárias em streaming
        cmd.arg("--extractor-args")
            .arg("youtube:player_client=android,web");

        // Repassa arquivo de cookies se configurado e existente
        if let Some(ref cookies_path) = self.cfg.cookies_file {
            if Path::new(cookies_path).exists() {
                cmd.arg("--cookies").arg(cookies_path);
            }
        }

        // Auto-detecta JavaScript runtime suportado (Deno ou Node.js) para decifrar assinaturas do YouTube
        if Path::new("/usr/bin/deno").exists() || Path::new("/usr/local/bin/deno").exists() {
            cmd.arg("--js-runtimes").arg("deno");
        } else if Path::new("/usr/bin/node").exists() || Path::new("/usr/local/bin/node").exists() {
            cmd.arg("--js-runtimes").arg("node");
        }

        cmd.arg(search_query);

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
        let lines = stderr_lines.lock().await.clone();

        // Prioriza linhas contendo erro real (yt-dlp e ffmpeg usam "ERROR:")
        let errors: Vec<String> = lines
            .iter()
            .filter(|l| l.contains("ERROR:") || l.contains("Error:"))
            .cloned()
            .collect();

        // 3. Verificação Estrita de Integridade no Disco (Barreira Atômica)
        match Self::verify_file_integrity(title, &self.dest_dir) {
            Ok(()) => Ok(()),
            Err(verify_err) => {
                Self::cleanup_leftovers(title, &self.dest_dir);
                // Código 101 indica que o yt-dlp encerrou devido a --max-downloads 1
                let is_ok_exit = status.success() || status.code() == Some(101);
                if !errors.is_empty() {
                    bail!("yt-dlp erro: {}", errors.join(" | "));
                } else if !is_ok_exit {
                    let non_warnings: Vec<String> = lines
                        .into_iter()
                        .filter(|l| !l.starts_with("WARNING:") && !l.trim().is_empty())
                        .collect();
                    if !non_warnings.is_empty() {
                        bail!("yt-dlp erro: {}", non_warnings.join(" | "));
                    } else {
                        bail!("Código de saída: {:?}", status.code());
                    }
                } else {
                    bail!("{}", verify_err);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode() {
        assert_eq!(
            Downloader::urlencode("Roberta Flack - Making Love"),
            "Roberta+Flack+-+Making+Love"
        );
        assert_eq!(
            Downloader::urlencode("Rex Taylor - Valerie"),
            "Rex+Taylor+-+Valerie"
        );
    }
}
