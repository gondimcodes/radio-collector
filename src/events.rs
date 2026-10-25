#[derive(Debug, Clone)]
pub enum CollectorEvent {
    StreamStatus {
        connected: bool,
        message: String,
        bitrate: Option<String>,
        last_check: String,
    },
    TrackDetected {
        title: String,
    },
    TrackPending {
        title: String,
    },
    DownloadStarted {
        title: String,
    },
    DownloadFinished {
        title: String,
        success: bool,
        message: String,
    },
    TrackCached {
        title: String,
    },
    TrackIgnored {
        title: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct TrackHistoryItem {
    pub time: String,
    pub status: HistoryStatus,
    pub artist: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryStatus {
    Pending,
    Downloading,
    Success,
    Cached,
    Ignored,
    Failed,
}

impl HistoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            HistoryStatus::Pending => "PENDENTE",
            HistoryStatus::Downloading => "BAIXANDO",
            HistoryStatus::Success => "BAIXADO",
            HistoryStatus::Cached => "CACHE",
            HistoryStatus::Ignored => "IGNORADO",
            HistoryStatus::Failed => "FALHA",
        }
    }
}
