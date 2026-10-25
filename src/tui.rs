use crate::config::DownloadConfig;
use crate::db::Database;
use crate::downloader::Downloader;
use crate::events::{CollectorEvent, HistoryStatus, TrackHistoryItem};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub struct TuiState {
    pub radio_url: String,
    pub output_dir: String,
    pub stream_connected: bool,
    pub stream_status_text: String,
    pub stream_bitrate: String,
    pub last_check_time: String,
    pub current_track: String,
    pub active_download: Option<String>,
    pub history: Vec<TrackHistoryItem>,
    pub table_state: TableState,
    pub pending_delete: Option<(usize, String)>,
    pub pending_action: Option<(usize, String)>,
    pub total_success: usize,
    pub total_cached: usize,
    pub total_ignored: usize,
    pub total_failed: usize,
}

impl TuiState {
    pub fn new(radio_url: &str, output_dir: &str) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        Self {
            radio_url: radio_url.to_string(),
            output_dir: output_dir.to_string(),
            stream_connected: false,
            stream_status_text: "Conectando...".to_string(),
            stream_bitrate: "Detectando...".to_string(),
            last_check_time: "--:--:--".to_string(),
            current_track: "Aguardando primeira leitura...".to_string(),
            active_download: None,
            pending_delete: None,
            pending_action: None,
            history: Vec::new(),
            table_state,
            total_success: 0,
            total_cached: 0,
            total_ignored: 0,
            total_failed: 0,
        }
    }

    pub fn next_row(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.history.len().saturating_sub(1) {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn previous_row(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.history.len().saturating_sub(1)
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn request_action_or_delete(&mut self) {
        if let Some(i) = self.table_state.selected() {
            if i < self.history.len() {
                let item = &self.history[i];
                if item.status == HistoryStatus::Pending {
                    self.pending_action =
                        Some((i, format!("{} - {}", item.artist, item.title)));
                } else if item.status == HistoryStatus::Success {
                    self.pending_delete =
                        Some((i, format!("{} - {}", item.artist, item.title)));
                }
            }
        }
    }

    pub fn confirm_delete(&mut self, db: &Database) {
        if let Some((i, _)) = self.pending_delete.take() {
            if i < self.history.len() {
                let item = self.history.remove(i);
                let full_title = format!("{} - {}", item.artist, item.title);
                let _ = db.delete_track(&full_title, &self.output_dir);
                if item.status == HistoryStatus::Success && self.total_success > 0 {
                    self.total_success -= 1;
                }
                if self.history.is_empty() {
                    self.table_state.select(None);
                } else if i >= self.history.len() {
                    self.table_state.select(Some(self.history.len() - 1));
                }
            }
        }
    }

    pub fn cancel_action_or_delete(&mut self) {
        self.pending_delete = None;
        self.pending_action = None;
    }

    pub fn handle_event(&mut self, event: CollectorEvent) {
        let now = chrono::Local::now().format("%H:%M:%S").to_string();

        match event {
            CollectorEvent::StreamStatus {
                connected,
                message,
                bitrate,
                last_check,
            } => {
                self.stream_connected = connected;
                self.stream_status_text = message;
                self.last_check_time = last_check;
                if let Some(br) = bitrate {
                    self.stream_bitrate = br;
                }
            }
            CollectorEvent::TrackDetected { title } => {
                self.current_track = title;
            }
            CollectorEvent::TrackPending { title } => {
                let (artist, track) = split_artist_track(&title);
                self.history.retain(|item| {
                    !(item.artist == artist && item.title.starts_with(&track))
                });
                self.push_history(TrackHistoryItem {
                    time: now,
                    status: HistoryStatus::Pending,
                    artist,
                    title: track,
                });
            }
            CollectorEvent::DownloadStarted { title } => {
                self.active_download = Some(title.clone());

                let (artist, track) = split_artist_track(&title);
                // Insere ou atualiza o item no topo do histórico com status BAIXANDO
                self.history.retain(|item| {
                    !(item.artist == artist && item.title.starts_with(&track))
                });
                self.push_history(TrackHistoryItem {
                    time: now,
                    status: HistoryStatus::Downloading,
                    artist,
                    title: track,
                });
            }
            CollectorEvent::DownloadFinished { title, success, message } => {
                self.active_download = None;
                let (artist, track) = split_artist_track(&title);
                if success {
                    self.total_success += 1;
                    let had_failure = self.history.iter().any(|item| {
                        item.status == HistoryStatus::Failed
                            && item.artist == artist
                            && item.title.starts_with(&track)
                    });
                    if had_failure && self.total_failed > 0 {
                        self.total_failed -= 1;
                    }
                    // Remove linha de BAIXANDO ou FALHA anterior
                    self.history.retain(|item| {
                        !(item.artist == artist && item.title.starts_with(&track))
                    });

                    self.push_history(TrackHistoryItem {
                        time: now,
                        status: HistoryStatus::Success,
                        artist,
                        title: track,
                    });
                } else {
                    self.total_failed += 1;
                    self.history.retain(|item| {
                        !(item.artist == artist && item.title.starts_with(&track))
                    });

                    self.push_history(TrackHistoryItem {
                        time: now,
                        status: HistoryStatus::Failed,
                        artist,
                        title: format!("{} ({})", track, message),
                    });
                }
            }
            CollectorEvent::TrackCached { title } => {
                let (artist, track) = split_artist_track(&title);
                self.total_cached += 1;
                // Evita duplicar linha se a faixa no topo do histórico já for exatamente essa música
                if let Some(first) = self.history.first() {
                    if first.artist == artist && first.title == track {
                        return;
                    }
                }
                self.push_history(TrackHistoryItem {
                    time: now,
                    status: HistoryStatus::Cached,
                    artist,
                    title: track,
                });
            }
            CollectorEvent::TrackIgnored { title, reason } => {
                self.total_ignored += 1;
                let (artist, track) = split_artist_track(&title);
                self.push_history(TrackHistoryItem {
                    time: now,
                    status: HistoryStatus::Ignored,
                    artist,
                    title: format!("{} [{}]", track, reason),
                });
            }
        }
    }

    fn push_history(&mut self, item: TrackHistoryItem) {
        self.history.insert(0, item);
        if self.history.len() > 100 {
            self.history.pop();
        }
    }
}

pub async fn run_tui(
    mut state: TuiState,
    mut event_rx: Receiver<CollectorEvent>,
    db: Arc<Database>,
    download_cfg: DownloadConfig,
) -> Result<()> {
    let downloader = Arc::new(Downloader::new(&download_cfg, &state.output_dir));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        // 1. Processa eventos pendentes do canal
        while let Ok(event) = event_rx.try_recv() {
            state.handle_event(event);
        }

        // 2. Renderiza frame com estado da tabela mutável
        terminal.draw(|f| render_ui(f, &mut state))?;

        // 3. Captura teclado de forma não-bloqueante
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if state.pending_action.is_some() {
                    match key.code {
                        KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Enter => {
                            // Baixar faixa pendente
                            if let Some((idx, full_title)) = state.pending_action.take() {
                                if idx < state.history.len() {
                                    state.history[idx].status = HistoryStatus::Downloading;
                                    state.history[idx].time = chrono::Local::now().format("%H:%M:%S").to_string();
                                    let db_clone = Arc::clone(&db);
                                    let dl_clone = Arc::clone(&downloader);
                                    let title_clone = full_title.clone();
                                    tokio::spawn(async move {
                                        let _ = db_clone.record_event("DOWNLOAD_START", &title_clone);
                                        match dl_clone.download(&title_clone).await {
                                            Ok(()) => {
                                                let _ = db_clone.record(&title_clone, "SUCESSO");
                                                let _ = db_clone.record_event("DOWNLOAD_FINISH", &format!("OK|{}", title_clone));
                                            }
                                            Err(e) => {
                                                let _ = db_clone.record(&title_clone, "FALHA");
                                                let _ = db_clone.record_event("DOWNLOAD_FINISH", &format!("ERR|{}|{}", e, title_clone));
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Delete => {
                            // Recusar faixa pendente (apenas remove da lista e do SQLite sem apagar arquivo)
                            if let Some((idx, full_title)) = state.pending_action.take() {
                                if idx < state.history.len() {
                                    state.history.remove(idx);
                                    let _ = db.delete_track_record_only(&full_title);
                                    if state.history.is_empty() {
                                        state.table_state.select(None);
                                    } else if idx >= state.history.len() {
                                        state.table_state.select(Some(state.history.len() - 1));
                                    }
                                }
                            }
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            state.cancel_action_or_delete();
                        }
                        _ => {}
                    }
                } else if state.pending_delete.is_some() {
                    match key.code {
                        KeyCode::Char('s')
                        | KeyCode::Char('S')
                        | KeyCode::Char('y')
                        | KeyCode::Char('Y')
                        | KeyCode::Enter => {
                            state.confirm_delete(&db);
                        }
                        KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Esc => {
                            state.cancel_action_or_delete();
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Up | KeyCode::Char('k') => state.previous_row(),
                        KeyCode::Down | KeyCode::Char('j') => state.next_row(),
                        KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Enter => {
                            if let Some(i) = state.table_state.selected() {
                                if i < state.history.len() && state.history[i].status == HistoryStatus::Pending {
                                    state.request_action_or_delete();
                                }
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            if let Some(i) = state.table_state.selected() {
                                if i < state.history.len() && state.history[i].status == HistoryStatus::Pending {
                                    state.request_action_or_delete();
                                }
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Delete => state.request_action_or_delete(),
                        _ => {}
                    }
                }
            }
        }
    }

    // Restaura terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn render_ui(f: &mut Frame, state: &mut TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Header / Status da Conexão / Now Playing
            Constraint::Min(8),     // Histórico Tabela
            Constraint::Length(3),  // Footer / Status
        ])
        .split(f.area());

    // 1. Header com Status de Conexão e Sinal
    let (bullet_color, status_badge) = if state.stream_connected {
        (
            Color::Green,
            Span::styled("● CONECTADO", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        )
    } else {
        (
            Color::Red,
            Span::styled("● SEM SINAL", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        )
    };

    let header_lines = vec![
        Line::from(vec![
            Span::raw(" Rádio: "),
            Span::styled(&state.radio_url, Style::default().fg(Color::Yellow)),
            Span::raw(" | Destino: "),
            Span::styled(&state.output_dir, Style::default().fg(Color::LightBlue)),
        ]),
        Line::from(vec![
            Span::raw(" Sinal: ["),
            status_badge,
            Span::raw("] "),
            Span::styled(&state.stream_status_text, Style::default().fg(Color::Gray)),
            Span::raw(" | Bitrate: "),
            Span::styled(&state.stream_bitrate, Style::default().fg(Color::Magenta)),
            Span::raw(" | Checado: "),
            Span::styled(&state.last_check_time, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw(" Tocando Agora: "),
            Span::styled(
                &state.current_track,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let header_title = format!(" Radio Collector v{} [Rust + Ratatui] ", env!("CARGO_PKG_VERSION"));
    let header = Paragraph::new(header_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(header_title)
            .style(Style::default().fg(bullet_color)),
    );
    f.render_widget(header, chunks[0]);

    // 2. Tabela de Histórico Interativa
    let rows: Vec<Row> = state
        .history
        .iter()
        .map(|item| {
            let status_style = match item.status {
                HistoryStatus::Pending => {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                }
                HistoryStatus::Downloading => {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                }
                HistoryStatus::Success => {
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                }
                HistoryStatus::Cached => Style::default().fg(Color::Blue),
                HistoryStatus::Ignored => Style::default().fg(Color::DarkGray),
                HistoryStatus::Failed => {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                }
            };

            Row::new(vec![
                Cell::from(item.time.clone()),
                Cell::from(item.status.as_str()).style(status_style),
                Cell::from(item.artist.clone()),
                Cell::from(item.title.clone()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(10), // Hora
            Constraint::Length(12), // Status
            Constraint::Length(25), // Artista
            Constraint::Min(20),    // Título / Detalhes
        ],
    )
    .header(
        Row::new(vec!["HORA", "STATUS", "ARTISTA", "FAIXA / DETALHES"]).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Histórico de Faixas (Selecione com ↑/↓ e tecle 'd' ou 'Delete' para excluir) "),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(35, 50, 80))
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    f.render_stateful_widget(table, chunks[1], &mut state.table_state);

    // 3. Footer com Atalhos Interativos e Contagem Físico-Atômica do Acervo
    let total_disco = count_mp3_files(&state.output_dir);

    let footer_text = format!(
        " [↑/↓] Navegar | [Enter/b] Baixar Pendente | [r/d] Recusar/Excluir | [q] Sair | Total no Acervo (MP3): {} | Cache: {} | Ignoradas: {} | Falhas: {}",
        total_disco, state.total_cached, state.total_ignored, state.total_failed
    );
    let footer = Paragraph::new(footer_text).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White)),
    );
    f.render_widget(footer, chunks[2]);

    // 4. Modal Flutuante para Faixas PENDENTES (Baixar ou Recusar)
    if let Some((_, ref title)) = state.pending_action {
        let area = centered_rect(65, 25, f.area());
        f.render_widget(Clear, area);

        let dialog_lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw(" Ação para a faixa "),
                Span::styled(
                    "PENDENTE",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw(":"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  Música: "),
                Span::styled(
                    format!("\"{}\"", title),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "   [B / Enter] Baixar Música   ",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled(
                    "   [R / Del] Recusar (Descartar)   ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled(
                    "   [Esc] Voltar   ",
                    Style::default().fg(Color::Gray),
                ),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" FAIXA PENDENTE - ESCOLHA A AÇÃO ")
            .style(Style::default().fg(Color::Cyan));

        let dialog = Paragraph::new(dialog_lines).block(block);
        f.render_widget(dialog, area);
    }
    // 5. Modal Flutuante de Confirmação de Exclusão (se ativo)
    else if let Some((_, ref title)) = state.pending_delete {
        let area = centered_rect(65, 25, f.area());
        f.render_widget(Clear, area);

        let dialog_lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw(" Deseja realmente excluir esta faixa da "),
                Span::styled(
                    "Base de Dados",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" e do "),
                Span::styled(
                    "Disco",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw("?"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  Música: "),
                Span::styled(
                    format!("\"{}\"", title),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "     [S] Sim (Confirmar Exclusão)     ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw("     "),
                Span::styled(
                    "     [N/Esc] Cancelar     ",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" CONFIRMAR EXCLUSÃO ")
            .style(Style::default().fg(Color::Red));

        let dialog = Paragraph::new(dialog_lines).block(block);
        f.render_widget(dialog, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn split_artist_track(raw: &str) -> (String, String) {
    let parts: Vec<&str> = raw.splitn(2, '-').collect();
    if parts.len() == 2 {
        (parts[0].trim().to_string(), parts[1].trim().to_string())
    } else {
        ("---".to_string(), raw.trim().to_string())
    }
}

pub fn count_mp3_files(dir: &str) -> usize {
    if let Ok(entries) = std::fs::read_dir(dir) {
        entries
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .map_or(false, |ext| ext.eq_ignore_ascii_case("mp3"))
            })
            .count()
    } else {
        0
    }
}
