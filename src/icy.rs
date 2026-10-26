use anyhow::{Context, Result};
use rustls_pki_types::ServerName;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone)]
pub struct IcyResult {
    pub title: Option<String>,
    pub bitrate: Option<String>,
}

enum StreamConnection {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for StreamConnection {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamConnection::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            StreamConnection::Tls(s) => std::pin::Pin::new(&mut **s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for StreamConnection {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            StreamConnection::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            StreamConnection::Tls(s) => std::pin::Pin::new(&mut **s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamConnection::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            StreamConnection::Tls(s) => std::pin::Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamConnection::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            StreamConnection::Tls(s) => std::pin::Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}

pub struct IcyClient {
    url: String,
    user_agent: String,
    timeout_duration: Duration,
    tls_connector: TlsConnector,
}

impl IcyClient {
    pub fn new(url: &str, user_agent: &str, timeout_secs: u64) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let tls_connector = TlsConnector::from(Arc::new(config));

        Self {
            url: url.to_string(),
            user_agent: user_agent.to_string(),
            timeout_duration: Duration::from_secs(timeout_secs),
            tls_connector,
        }
    }

    pub async fn fetch_current_title(&self) -> Result<IcyResult> {
        timeout(self.timeout_duration, self.do_fetch())
            .await
            .context("Tempo limite ao conectar no stream ICY")?
    }

    async fn connect_stream(
        &self,
        is_tls: bool,
        host: &str,
        port: u16,
    ) -> Result<StreamConnection> {
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("Falha de socket TCP ao conectar em {}", addr))?;

        if is_tls {
            let server_name = ServerName::try_from(host.to_string())
                .map_err(|e| anyhow::anyhow!("Nome de domínio inválido para TLS: {}", e))?
                .to_owned();

            let tls_stream = self
                .tls_connector
                .connect(server_name, stream)
                .await
                .with_context(|| format!("Falha no handshake TLS com {}", host))?;

            Ok(StreamConnection::Tls(Box::new(tls_stream)))
        } else {
            Ok(StreamConnection::Plain(stream))
        }
    }

    async fn do_fetch(&self) -> Result<IcyResult> {
        let mut current_url = self.url.clone();

        for _ in 0..5 {
            let (is_tls, host, port, path) = parse_stream_url(&current_url)?;
            let stream = self.connect_stream(is_tls, &host, port).await?;

            let request = format!(
                "GET {} HTTP/1.1\r\n\
                 Host: {}:{}\r\n\
                 User-Agent: {}\r\n\
                 Icy-MetaData: 1\r\n\
                 Connection: close\r\n\r\n",
                path, host, port, self.user_agent
            );

            let mut reader = BufReader::new(stream);
            reader.get_mut().write_all(request.as_bytes()).await?;

            let mut metaint: Option<usize> = None;
            let mut bitrate: Option<String> = None;
            let mut redirect_location: Option<String> = None;
            let mut status_code: Option<u16> = None;
            let mut is_first_line = true;

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

                if is_first_line {
                    is_first_line = false;
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(code) = parts[1].parse::<u16>() {
                            status_code = Some(code);
                        }
                    }
                }

                let lower = trimmed.to_lowercase();
                if lower.starts_with("location:") {
                    let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        redirect_location = Some(parts[1].trim().to_string());
                    }
                } else if lower.starts_with("icy-metaint:") {
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

            // Trata redirecionamentos HTTP 3xx
            if let Some(code) = status_code {
                if (300..=399).contains(&code) {
                    if let Some(loc) = redirect_location {
                        current_url = if loc.starts_with("http://") || loc.starts_with("https://") {
                            loc
                        } else if loc.starts_with('/') {
                            let scheme = if is_tls { "https" } else { "http" };
                            format!("{}://{}:{}{}", scheme, host, port, loc)
                        } else {
                            loc
                        };
                        continue;
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

            return Ok(IcyResult {
                title: None,
                bitrate,
            });
        }

        anyhow::bail!("Muitos redirecionamentos HTTP (limite de 5 excedido)")
    }
}

fn parse_stream_url(url: &str) -> Result<(bool, String, u16, String)> {
    let trimmed = url.trim();
    let (is_https, raw) = if let Some(stripped) = trimmed.strip_prefix("https://") {
        (true, stripped)
    } else if let Some(stripped) = trimmed.strip_prefix("http://") {
        (false, stripped)
    } else {
        (false, trimmed)
    };

    let parts: Vec<&str> = raw.splitn(2, '/').collect();
    let host_port = parts[0];
    let path = if parts.len() > 1 {
        format!("/{}", parts[1])
    } else {
        "/".to_string()
    };

    let hp: Vec<&str> = host_port.split(':').collect();
    let host = hp[0].to_string();
    let default_port = if is_https { 443 } else { 80 };
    let port = if hp.len() > 1 {
        hp[1].parse::<u16>().unwrap_or(default_port)
    } else {
        default_port
    };

    Ok((is_https, host, port, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_http_url() {
        let (is_tls, host, port, path) =
            parse_stream_url("http://sokakarecos.ddns.net:9005/light128").unwrap();
        assert!(!is_tls);
        assert_eq!(host, "sokakarecos.ddns.net");
        assert_eq!(port, 9005);
        assert_eq!(path, "/light128");
    }

    #[test]
    fn test_parse_https_url() {
        let (is_tls, host, port, path) =
            parse_stream_url("https://stream.zeno.fm/edh4zta40f9uv").unwrap();
        assert!(is_tls);
        assert_eq!(host, "stream.zeno.fm");
        assert_eq!(port, 443);
        assert_eq!(path, "/edh4zta40f9uv");
    }
}
