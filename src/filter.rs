use crate::config::FilterConfig;
use regex::Regex;

pub struct TrackFilter {
    min_length: usize,
    require_hyphen: bool,
    compiled_patterns: Vec<Regex>,
}

impl TrackFilter {
    pub fn new(cfg: &FilterConfig) -> Self {
        let compiled_patterns = cfg
            .ignore_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        Self {
            min_length: cfg.min_title_length,
            require_hyphen: cfg.require_hyphen,
            compiled_patterns,
        }
    }

    pub fn check(&self, title: &str) -> Option<&'static str> {
        let trimmed = title.trim();

        if trimmed.len() < self.min_length {
            return Some("Título muito curto");
        }

        if self.require_hyphen && !trimmed.contains('-') {
            return Some("Sem separador de artista/música ('-')");
        }

        for regex in &self.compiled_patterns {
            if regex.is_match(trimmed) {
                return Some("Padrão de vinheta/slogan detectado");
            }
        }

        None
    }
}
