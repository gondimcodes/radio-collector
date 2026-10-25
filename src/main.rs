mod config;
mod db;
mod downloader;
mod events;
mod filter;
mod icy;
mod tui;

use anyhow::Result;
use clap::Parser;
use config::Config;
use db::Database;
use downloader::Downloader;
use events::{CollectorEvent, HistoryStatus, TrackHistoryItem};
use filter::TrackFilter;
use icy::IcyClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tui::{run_tui, split_artist_track, TuiState};

#[derive(Parser, Debug)]
#[command(
    name = "radio-collector",
    author = "gondim <gondim@gmail.com>",
    version = env!("CARGO_PKG_VERSION"),
    about = "Coletor inteligente de rádio online com TUI e download via yt-dlp"
)]
struct Cli {
    /// Caminho customizado para o arquivo de configuração
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Executa em modo daemon de background (sem interface TUI, ideal para systemd)
    #[arg(short, long)]
    daemon: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // 1. Carregar Configuração
    let (cfg, config_path) = Config::load(cli.config.as_deref())?;

    // 2. Inicializar Banco SQLite e Reconciliar com o Disco
    let db = Arc::new(Database::open(&cfg.storage.db_path)?);
    let _ = db.reconcile_with_disk(&cfg.storage.output_dir);

    // Registra heartbeat/evento no banco para sincronização entre processos (daemon -> TUI)
    let _ = db.record_event("STARTUP", "Sistema iniciado");

    // 3. Inicializar Canal de Eventos entre Background e UI
    let (event_tx, event_rx) = mpsc::channel::<CollectorEvent>(100);

    // 4. Decisão de Interface: TUI interativa ou Headless Daemon
    let run_in_daemon_mode = cli.daemon || !cfg.ui.tui;

    if run_in_daemon_mode {
        register_worker("daemon");

        println!(
            "[*] Radio Collector iniciado em modo DAEMON (Config: {:?})",
            config_path
        );
        println!("[*] Monitorando: {}", cfg.stream.url);
        println!("[*] Destino de Áudio: {}", cfg.storage.output_dir);

        let worker_cfg = cfg.clone();
        let worker_db = Arc::clone(&db);
        let worker_tx = event_tx.clone();

        tokio::spawn(async move {
            run_worker(worker_cfg, worker_db, worker_tx, true).await;
        });

        // Processa eventos e emite logs estruturados no stdout/journald
        let mut rx = event_rx;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("[*] Sinal de interrupção recebido. Encerrando de forma segura...");
                unregister_worker();
            }
            _ = async {
                while let Some(event) = rx.recv().await {
                    match event {
                        CollectorEvent::TrackDetected { title } => {
                            println!("[NOW PLAYING] Rádio transmitindo: '{}'", title);
                        }
                        CollectorEvent::DownloadStarted { title } => {
                            println!("[DOWNLOAD] Iniciando busca e download em 320k: '{}'", title);
                        }
                        CollectorEvent::DownloadFinished { title, success, message } => {
                            if success {
                                println!("[SUCESSO] Faixa gravada com capa e ID3: '{}'", title);
                            } else {
                                eprintln!("[FALHA] Erro ao baixar '{}': {}", title, message);
                            }
                        }
                        CollectorEvent::TrackCached { title } => {
                            println!("[CACHE] Faixa já existe no acervo local: '{}'", title);
                        }
                        CollectorEvent::TrackIgnored { title, reason } => {
                            println!("[IGNORADO] Descartado ({}): '{}'", reason, title);
                        }
                        _ => {}
                    }
                }
            } => {}
        }
    } else {
        let active_worker = get_active_worker_info();
        let is_daemon_active = match active_worker {
            Some((pid, ref role)) => pid != std::process::id() as i32 && role == "daemon",
            None => false,
        };

        let worker_cfg = cfg.clone();
        let worker_db = Arc::clone(&db);
        let worker_tx = event_tx.clone();

        // Obtém o último event_id de eventos_monitor para o observer processar apenas eventos futuros
        let initial_event_id = db.get_latest_event_id().unwrap_or(0);

        if is_daemon_active {
            // Modo Observador: O daemon systemd já está ativo baixando.
            tokio::spawn(async move {
                run_observer(worker_cfg, worker_db, worker_tx, initial_event_id).await;
            });
        } else {
            // Modo Coletor Standalone: Não há daemon ativo, a TUI assume a liderança.
            register_worker("tui");
            tokio::spawn(async move {
                run_worker(worker_cfg, worker_db, worker_tx, false).await;
            });
        }

        let mut state = TuiState::new(&cfg.stream.url, &cfg.storage.output_dir);
        let (success, failed, cached) = db.get_stats().unwrap_or((0, 0, 0));
        state.total_success = success;
        state.total_failed = failed;
        state.total_cached = cached;

        // Pré-carrega os últimos downloads do SQLite para a tabela não iniciar vazia
        if let Ok(recent) = db.get_recent(100) {
            for (titulo, status, hora) in recent.into_iter().rev() {
                let (artist, track) = split_artist_track(&titulo);
                let hist_status = match status.as_str() {
                    "SUCESSO" => HistoryStatus::Success,
                    "PENDENTE" => HistoryStatus::Pending,
                    "CACHE" => HistoryStatus::Cached,
                    "IGNORADO" => HistoryStatus::Ignored,
                    _ => HistoryStatus::Failed,
                };
                state.history.insert(
                    0,
                    TrackHistoryItem {
                        time: hora,
                        status: hist_status,
                        artist,
                        title: track,
                    },
                );
            }
        }

        let res = run_tui(state, event_rx, Arc::clone(&db), cfg.download.clone()).await;
        if !is_daemon_active {
            unregister_worker();
        }
        res?;
    }

    Ok(())
}

fn get_worker_pid_path() -> PathBuf {
    let var_lib = std::path::Path::new("/var/lib/radio-collector");
    if var_lib.is_dir() {
        var_lib.join("worker.pid")
    } else {
        PathBuf::from("/tmp/radio-collector-worker.pid")
    }
}

fn get_active_worker_info() -> Option<(i32, String)> {
    let pid_file = get_worker_pid_path();
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        let parts: Vec<&str> = content.trim().split(':').collect();
        if let Ok(pid) = parts[0].parse::<i32>() {
            let role = if parts.len() > 1 { parts[1] } else { "daemon" };
            let proc_path = format!("/proc/{}", pid);
            if std::path::Path::new(&proc_path).exists() {
                let comm_path = format!("/proc/{}/comm", pid);
                if let Ok(comm) = std::fs::read_to_string(comm_path) {
                    if comm.trim().contains("radio-collector") {
                        return Some((pid, role.to_string()));
                    }
                }
            }
        }
    }
    None
}

fn register_worker(role: &str) {
    let pid_file = get_worker_pid_path();
    let my_pid = std::process::id();
    let _ = std::fs::write(&pid_file, format!("{}:{}", my_pid, role));
}

fn unregister_worker() {
    let pid_file = get_worker_pid_path();
    let my_pid = std::process::id();
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if content.starts_with(&my_pid.to_string()) {
            let _ = std::fs::remove_file(&pid_file);
        }
    }
}

async fn run_observer(
    cfg: Config,
    db: Arc<Database>,
    tx: mpsc::Sender<CollectorEvent>,
    initial_event_id: i64,
) {
    let icy = IcyClient::new(
        &cfg.stream.url,
        &cfg.stream.user_agent,
        cfg.stream.timeout_seconds,
    );

    let mut last_processed_event_id = initial_event_id;
    let mut check_counter: u32 = 0;
    let interval = Duration::from_millis(500);

    loop {
        // A cada ~10 segundos (20 iterações de 500ms), afere a conexão do stream para status do cabeçalho
        if check_counter % 20 == 0 {
            let now = chrono::Local::now().format("%H:%M:%S").to_string();
            match icy.fetch_current_title().await {
                Ok(result) => {
                    let _ = tx
                        .send(CollectorEvent::StreamStatus {
                            connected: true,
                            message: "Sinal recebido (Modo Monitor - Serviço ativo)".to_string(),
                            bitrate: result.bitrate,
                            last_check: now,
                        })
                        .await;

                    if let Some(title) = result.title {
                        let _ = tx
                            .send(CollectorEvent::TrackDetected {
                                title: title.clone(),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(CollectorEvent::StreamStatus {
                            connected: false,
                            message: format!("Erro: {}", e),
                            bitrate: None,
                            last_check: now,
                        })
                        .await;
                }
            }
        }
        check_counter = check_counter.wrapping_add(1);

        // Processa eventos em tempo real gravados pelo daemon em `eventos_monitor`
        if let Ok(new_events) = db.get_new_events(last_processed_event_id) {
            for (ev_id, tipo, detalhe) in new_events {
                last_processed_event_id = ev_id;
                match tipo.as_str() {
                    "CACHE" => {
                        let _ = tx.send(CollectorEvent::TrackCached { title: detalhe }).await;
                    }
                    "PENDENTE" => {
                        let _ = tx.send(CollectorEvent::TrackPending { title: detalhe }).await;
                    }
                    "IGNORADO" => {
                        let parts: Vec<&str> = detalhe.splitn(2, " [").collect();
                        let title = parts[0].to_string();
                        let reason = if parts.len() > 1 {
                            parts[1].trim_end_matches(']').to_string()
                        } else {
                            "Filtro".to_string()
                        };
                        let _ = tx
                            .send(CollectorEvent::TrackIgnored { title, reason })
                            .await;
                    }
                    "DOWNLOAD_START" => {
                        let _ = tx
                            .send(CollectorEvent::DownloadStarted { title: detalhe })
                            .await;
                    }
                    "DOWNLOAD_FINISH" => {
                        let parts: Vec<&str> = detalhe.splitn(3, '|').collect();
                        if !parts.is_empty() {
                            let success = parts[0] == "OK";
                            let (msg, title) = if success && parts.len() >= 2 {
                                ("OK".to_string(), parts[1].to_string())
                            } else if !success && parts.len() >= 3 {
                                (parts[1].to_string(), parts[2].to_string())
                            } else {
                                ("Finalizado".to_string(), detalhe.clone())
                            };

                            let _ = tx
                                .send(CollectorEvent::DownloadFinished {
                                    title,
                                    success,
                                    message: msg,
                                })
                                .await;
                        }
                    }
                    _ => {}
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn run_worker(
    cfg: Config,
    db: Arc<Database>,
    tx: mpsc::Sender<CollectorEvent>,
    is_daemon: bool,
) {
    let icy = IcyClient::new(
        &cfg.stream.url,
        &cfg.stream.user_agent,
        cfg.stream.timeout_seconds,
    );
    let filter = TrackFilter::new(&cfg.filter);
    let downloader = Downloader::new(&cfg.download, &cfg.storage.output_dir);

    let mut last_title = String::new();
    let interval = Duration::from_secs(cfg.stream.check_interval_seconds);

    loop {
        // Se a TUI estava coletando e o serviço systemd iniciou em background,
        // a TUI cede o controle imediatamente e transiciona para Modo Observador.
        if !is_daemon {
            if let Some((pid, role)) = get_active_worker_info() {
                if pid != std::process::id() as i32 && role == "daemon" {
                    let _ = tx
                        .send(CollectorEvent::StreamStatus {
                            connected: true,
                            message: "Serviço systemd assumiu a coleta (Modo Monitor)".to_string(),
                            bitrate: None,
                            last_check: chrono::Local::now().format("%H:%M:%S").to_string(),
                        })
                        .await;

                    // Transiciona a TUI para modo observador passivo (sem downloads locais)
                    let last_ev_id = db.get_latest_event_id().unwrap_or(0);
                    run_observer(cfg, db, tx, last_ev_id).await;
                    return;
                }
            }
        }

        let now = chrono::Local::now().format("%H:%M:%S").to_string();

        match icy.fetch_current_title().await {
            Ok(result) => {
                let _ = tx
                    .send(CollectorEvent::StreamStatus {
                        connected: true,
                        message: "Sinal recebido com sucesso".to_string(),
                        bitrate: result.bitrate,
                        last_check: now,
                    })
                    .await;

                if let Some(title) = result.title {
                    if title != last_title {
                        last_title = title.clone();
                        let _ = tx
                            .send(CollectorEvent::TrackDetected {
                                title: title.clone(),
                            })
                            .await;

                        // 1. Checa filtros de vinheta
                        if let Some(reason) = filter.check(&title) {
                            let _ = db.record_event("IGNORADO", &format!("{} [{}]", title, reason));
                            let _ = tx
                                .send(CollectorEvent::TrackIgnored {
                                    title: title.clone(),
                                    reason: reason.to_string(),
                                })
                                .await;
                        }
                        // 2. Checa se já foi processada no SQLite ou se já existe no disco
                        else if db.is_processed(&title, &cfg.storage.output_dir).unwrap_or(false) {
                            let _ = db.record_event("CACHE", &title);
                            let _ = tx
                                .send(CollectorEvent::TrackCached {
                                    title: title.clone(),
                                })
                                .await;
                        }
                        // 3. Checa modo de download: Automático ou Pendente
                        else if !cfg.download.auto_download {
                            let _ = db.record(&title, "PENDENTE");
                            let _ = db.record_event("PENDENTE", &title);
                            let _ = tx
                                .send(CollectorEvent::TrackPending {
                                    title: title.clone(),
                                })
                                .await;
                        }
                        // 4. Dispara o download oficial de alta fidelidade automaticamente
                        else {
                            let _ = db.record_event("DOWNLOAD_START", &title);
                            let _ = tx
                                .send(CollectorEvent::DownloadStarted {
                                    title: title.clone(),
                                })
                                .await;

                            match downloader.download(&title).await {
                                Ok(()) => {
                                    let _ = db.record(&title, "SUCESSO");
                                    let _ = db.record_event("DOWNLOAD_FINISH", &format!("OK|{}", title));
                                    let _ = tx
                                        .send(CollectorEvent::DownloadFinished {
                                            title: title.clone(),
                                            success: true,
                                            message: "OK".to_string(),
                                        })
                                        .await;
                                }
                                Err(e) => {
                                    let _ = db.record(&title, "FALHA");
                                    let _ = db.record_event("DOWNLOAD_FINISH", &format!("ERR|{}|{}", e, title));
                                    let _ = tx
                                        .send(CollectorEvent::DownloadFinished {
                                            title: title.clone(),
                                            success: false,
                                            message: e.to_string(),
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx
                    .send(CollectorEvent::StreamStatus {
                        connected: false,
                        message: format!("Erro: {}", e),
                        bitrate: None,
                        last_check: now,
                    })
                    .await;
            }
        }

        tokio::time::sleep(interval).await;
    }
}
