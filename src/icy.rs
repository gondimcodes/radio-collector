use anyhow::{Context, Result};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct IcyResult {
    pub title: Option<String>,
    pub bitrate: Option<String>,
}

pub struct IcyClient {
    url: String,
    user_agent: String,
    timeout_duration: Duration,
}

impl IcyClient {
    pub fn new(url: &str, user_agent: &str, timeout_secs: u64) -> Self {
        Self {
            url: url.to_string(),
            user_agent: user_agent.to_string(),
            timeout_duration: Duration::from_secs(timeout_secs),
        }
    }

    pub async fn fetch_current_title(&self) -> Result<IcyResult> {
        timeout(self.timeout_duration, self.do_fetch())
            .await
            .context("Tempo limite ao conectar no stream ICY")?
    }

    async fn do_fetch(&self) -> Result<IcyResult> {
        let (host, port, path) = parse_stream_url(&self.url)?;
        let addr = format!("{}:{}", host, port);

        let mut stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("Falha de socket TCP ao conectar em {}", addr))?;

        let request = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}:{}\r\n\
             User-Agent: {}\r\n\
             Icy-MetaData: 1\r\n\
             Connection: close\r\n\r\n",
            path, host, port, self.user_agent
        );

        stream.write_all(request.as_bytes()).await?;

        let mut reader = BufReader::new(stream);
        let mut metaint: Option<usize> = None;
        let mut bitrate: Option<String> = None;

        // 1. Ler cabeçalhos HTTP/ICY (com suporte seguro a Latin-1 / ISO-8859-1 sem quebrar UTF-8)
        loop {
            let mut line_bytes = Vec::new();
            let bytes_read = reader.read_until(b'\n', &mut line_bytes).await?;
            if bytes_read == 0 {
                break;
            }

            let line = String::from_utf8_lossy(&line_bytes);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }

            let lower = trimmed.to_lowercase();
            if lower.starts_with("icy-metaint:") {
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 2 {
                    if let Ok(val) = parts[1].trim().parse::<usize>() {
                        metaint = Some(val);
                    }
                }
            } else if lower.starts_with("icy-br:") {
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 2 {
                    bitrate = Some(format!("{} kbps", parts[1].trim()));
                }
            }
        }

        let interval = match metaint {
            Some(i) if i > 0 => i,
            _ => anyhow::bail!("Servidor não retornou o cabeçalho 'icy-metaint'"),
        };

        // Validação de segurança: limita o tamanho do buffer para evitar OOM DoS (CWE-400)
        // Streams legítimas usam entre 8192 e 32768 bytes. Limitamos a 256 KB.
        if interval > 256 * 1024 {
            anyhow::bail!(
                "Cabeçalho 'icy-metaint' excedeu o limite máximo seguro de 256 KB: {} bytes",
                interval
            );
        }

        // 2. Loop de leitura de até 5 blocos caso o primeiro venha sem tag nova
        for _ in 0..5 {
            let mut audio_buf = vec![0u8; interval];
            reader.read_exact(&mut audio_buf).await?;

            let mut len_byte = [0u8; 1];
            reader.read_exact(&mut len_byte).await?;
            let meta_len = (len_byte[0] as usize) * 16;

            if meta_len > 0 {
                let mut meta_buf = vec![0u8; meta_len];
                reader.read_exact(&mut meta_buf).await?;

                let meta_str = String::from_utf8_lossy(&meta_buf);
                if let Some(start) = meta_str.find("StreamTitle='") {
                    let rest = &meta_str[start + 13..];
                    if let Some(end) = rest.find("';") {
                        let title = rest[..end].trim().to_string();
                        if !title.is_empty() {
                            return Ok(IcyResult {
                                title: Some(title),
                                bitrate,
                            });
                        }
                    }
                }
            }
        }

        Ok(IcyResult {
            title: None,
            bitrate,
        })
    }
}

fn parse_stream_url(url: &str) -> Result<(String, u16, String)> {
    let raw = url.trim_start_matches("http://").trim_start_matches("https://");
    let parts: Vec<&str> = raw.splitn(2, '/').collect();
    let host_port = parts[0];
    let path = if parts.len() > 1 {
        format!("/{}", parts[1])
    } else {
        "/".to_string()
    };

    let hp: Vec<&str> = host_port.split(':').collect();
    let host = hp[0].to_string();
    let port = if hp.len() > 1 {
        hp[1].parse::<u16>().unwrap_or(80)
    } else {
        80
    };

    Ok((host, port, path))
}
