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
    pub details: Option<String>,
}

impl TrackHistoryItem {
    pub fn clean_full_title(&self) -> String {
        if self.artist == "---" || self.artist.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }

    pub fn display_track_and_details(&self) -> String {
        match &self.details {
            Some(d) if self.status == HistoryStatus::Failed => format!("{} ({})", self.title, d),
            Some(d) if self.status == HistoryStatus::Ignored => format!("{} [{}]", self.title, d),
            Some(d) => format!("{} - {}", self.title, d),
            None => self.title.clone(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_full_title_with_artist() {
        let item = TrackHistoryItem {
            time: "18:00:00".to_string(),
            status: HistoryStatus::Failed,
            artist: "David Castle".to_string(),
            title: "You're Too Far Away".to_string(),
            details: Some("yt-dlp erro: Sign in to confirm your age".to_string()),
        };
        assert_eq!(
            item.clean_full_title(),
            "David Castle - You're Too Far Away"
        );
        assert_eq!(
            item.display_track_and_details(),
            "You're Too Far Away (yt-dlp erro: Sign in to confirm your age)"
        );
    }

    #[test]
    fn test_clean_full_title_without_artist() {
        let item = TrackHistoryItem {
            time: "18:00:00".to_string(),
            status: HistoryStatus::Success,
            artist: "---".to_string(),
            title: "Stand By Me".to_string(),
            details: None,
        };
        assert_eq!(item.clean_full_title(), "Stand By Me");
        assert_eq!(item.display_track_and_details(), "Stand By Me");
    }
}
