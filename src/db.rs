use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Falha ao criar diretório para banco SQLite: {:?}", parent)
            })?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Falha ao abrir banco SQLite em: {}", path))?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS historico (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                titulo TEXT UNIQUE,
                baixado_em TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                status TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_historico_titulo ON historico(titulo);
            CREATE INDEX IF NOT EXISTS idx_historico_status ON historico(status);

            CREATE TABLE IF NOT EXISTS eventos_monitor (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tipo TEXT,
                detalhe TEXT,
                criado_em TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn get_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Falha ao adquirir lock do SQLite: {}", e))
    }

    /// Reconcilia o banco SQLite com o sistema de arquivos:
    /// - Garante que todo arquivo MP3 válido presente no disco tenha registro SUCESSO no SQLite.
    /// - Corrige registros que estavam marcados como FALHA caso o arquivo físico exista no disco.
    /// - Limpa registros duplicados de FALHA para faixas que já existem com SUCESSO no disco.
    pub fn reconcile_with_disk(&self, dest_dir: &str) -> Result<usize> {
        let dest_path = Path::new(dest_dir);
        if !dest_path.exists() {
            return Ok(0);
        }

        // 1. Coleta todos os arquivos .mp3 válidos no disco (>= 500 KB)
        let mut disk_stems = Vec::new();
        if let Ok(entries) = fs::read_dir(dest_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
                {
                    if let Ok(meta) = path.metadata() {
                        if meta.len() >= 500 * 1024 {
                            if let Some(stem) = path.file_stem() {
                                disk_stems.push(stem.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }

        if disk_stems.is_empty() {
            return Ok(0);
        }

        let mut updated = 0;
        let conn = self.get_conn()?;

        let mut stmt = conn.prepare("SELECT id, titulo, status FROM historico")?;
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .flatten()
            .collect();

        let mut matched_db_ids = std::collections::HashSet::new();

        // 2. Reconciliação Disco -> Banco com casamento fonográfico estrito
        for file_stem in &disk_stems {
            let mut matched = false;

            // Casamento exato, normalizado ou sem sufixo de mídia
            for (id, titulo, status) in &rows {
                if Self::is_matching_audio_file(file_stem, titulo) {
                    matched = true;
                    matched_db_ids.insert(*id);
                    if status != "SUCESSO" {
                        let _ = conn.execute(
                            "UPDATE historico SET status = 'SUCESSO' WHERE id = ?1",
                            params![id],
                        );
                        updated += 1;
                    }
                    break;
                }
            }

            // Se o arquivo físico no disco não existe no banco, insere como SUCESSO
            if !matched {
                let _ = conn.execute(
                    "INSERT INTO historico (titulo, status, baixado_em)
                     VALUES (?1, 'SUCESSO', CURRENT_TIMESTAMP)
                     ON CONFLICT(titulo) DO UPDATE SET status = 'SUCESSO'",
                    params![file_stem],
                );
                updated += 1;
            }
        }

        // 3. Reconciliação Banco -> Disco: marca como AUSENTE registros SUCESSO cujos arquivos inexistem no disco
        for (id, _titulo, status) in &rows {
            if status == "SUCESSO" && !matched_db_ids.contains(id) {
                let _ = conn.execute(
                    "UPDATE historico SET status = 'AUSENTE' WHERE id = ?1",
                    params![id],
                );
                updated += 1;
            }
        }

        // 4. Limpa registros duplicados de FALHA para faixas que já existem com SUCESSO no disco
        let _ = conn.execute(
            "DELETE FROM historico WHERE status IN ('FALHA', 'TIMEOUT') AND LOWER(titulo) IN (SELECT LOWER(titulo) FROM historico WHERE status = 'SUCESSO')",
            [],
        );

        Ok(updated)
    }

    pub fn is_processed(&self, title: &str, dest_dir: &str) -> Result<bool> {
        let norm_title = Self::normalize(title);

        let parts: Vec<&str> = title.splitn(2, '-').collect();
        let (artist_part, track_part) = if parts.len() == 2 {
            (Self::normalize(parts[0]), Self::normalize(parts[1]))
        } else {
            (String::new(), Self::normalize(title))
        };

        // 1. Checagem no banco de dados SQLite (comparações exatas e normalizadas para SUCESSO ou PENDENTE)
        {
            let conn = self.get_conn()?;
            let mut stmt = conn
                .prepare("SELECT titulo FROM historico WHERE status IN ('SUCESSO', 'PENDENTE')")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for r in rows.flatten() {
                let db_norm = Self::normalize(&r);
                if db_norm == norm_title {
                    return Ok(true);
                }
                if !artist_part.is_empty() && !track_part.is_empty() {
                    let db_parts: Vec<&str> = r.splitn(2, '-').collect();
                    if db_parts.len() == 2 {
                        let db_artist = Self::normalize(db_parts[0]);
                        let db_track = Self::normalize(db_parts[1]);
                        if db_artist == artist_part && db_track == track_part {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        // 2. Checagem no sistema de arquivos com casamento estrito
        let dest_path = Path::new(dest_dir);
        if dest_path.exists() {
            if let Ok(entries) = fs::read_dir(dest_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_mp3 = path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));
                    if !is_mp3 {
                        continue;
                    }

                    let file_stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if Self::is_matching_audio_file(&file_stem, title) {
                        let _ = self.record(title, "SUCESSO");
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    pub fn record_event(&self, tipo: &str, detalhe: &str) -> Result<()> {
        let conn = self.get_conn()?;
        conn.execute(
            "INSERT INTO eventos_monitor (tipo, detalhe) VALUES (?1, ?2)",
            params![tipo, detalhe],
        )?;
        Ok(())
    }

    pub fn get_latest_event_id(&self) -> Result<i64> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare("SELECT COALESCE(MAX(id), 0) FROM eventos_monitor")?;
        let max_id: i64 = stmt.query_row([], |row| row.get(0))?;
        Ok(max_id)
    }

    pub fn get_new_events(&self, after_id: i64) -> Result<Vec<(i64, String, String)>> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, tipo, detalhe FROM eventos_monitor WHERE id > ?1 ORDER BY id ASC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![after_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut list = Vec::new();
        for r in rows.flatten() {
            list.push(r);
        }
        Ok(list)
    }

    pub fn record(&self, title: &str, status: &str) -> Result<()> {
        let conn = self.get_conn()?;

        // Busca se já existe registro com casamento insensível a maiúsculas/minúsculas
        let existing_id: Option<i64> = conn
            .prepare("SELECT id FROM historico WHERE LOWER(titulo) = LOWER(?1) LIMIT 1")?
            .query_row(params![title], |row| row.get(0))
            .ok();

        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE historico SET titulo = ?1, status = ?2, baixado_em = CURRENT_TIMESTAMP WHERE id = ?3",
                params![title, status, id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO historico (titulo, status, baixado_em) VALUES (?1, ?2, CURRENT_TIMESTAMP)
                 ON CONFLICT(titulo) DO UPDATE SET status = excluded.status, baixado_em = CURRENT_TIMESTAMP",
                params![title, status],
            )?;
        }

        // Se a faixa alcançou SUCESSO, purga qualquer registro duplicado prévio que tenha ficado como FALHA
        if status == "SUCESSO" {
            let _ = conn.execute(
                "DELETE FROM historico WHERE status IN ('FALHA', 'TIMEOUT') AND LOWER(titulo) = LOWER(?1)",
                params![title],
            );
        }

        Ok(())
    }

    pub fn delete_track_record_only(&self, title: &str) -> Result<()> {
        let conn = self.get_conn()?;
        let norm = Self::normalize(title);
        let affected = conn.execute(
            "DELETE FROM historico WHERE LOWER(titulo) = LOWER(?1) OR titulo = ?1",
            params![title],
        )?;
        if affected == 0 {
            let mut stmt = conn.prepare("SELECT id, titulo FROM historico")?;
            let rows: Vec<(i64, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();
            for (id, db_title) in rows {
                if Self::normalize(&db_title) == norm {
                    let _ = conn.execute("DELETE FROM historico WHERE id = ?1", params![id]);
                }
            }
        }
        Ok(())
    }

    pub fn delete_track(&self, title: &str, dest_dir: &str) -> Result<()> {
        self.delete_track_record_only(title)?;

        let dest_path = Path::new(dest_dir);
        if dest_path.exists() {
            if let Ok(entries) = fs::read_dir(dest_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_mp3 = path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));
                    if !is_mp3 {
                        continue;
                    }

                    let file_stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();

                    // Deleta apenas se coincidir estritamente com a faixa auditada
                    if Self::is_matching_audio_file(&file_stem, title) {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get_stats(&self) -> Result<(usize, usize, usize, usize)> {
        let mut total_success = 0;
        let mut total_failed = 0;
        let mut total_cached = 0;
        let mut total_missing = 0;

        let conn = self.get_conn()?;
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM historico GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((status, count as usize))
        })?;

        for r in rows.flatten() {
            match r.0.as_str() {
                "SUCESSO" => total_success = r.1,
                "CACHE" => total_cached += r.1,
                "AUSENTE" => total_missing += r.1,
                "FALHA" | "TIMEOUT" => total_failed += r.1,
                _ => {}
            }
        }

        Ok((total_success, total_failed, total_cached, total_missing))
    }

    pub fn get_all_history(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT titulo, status, strftime('%d/%m/%Y %H:%M:%S', baixado_em, 'localtime')
             FROM historico
             ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let titulo: String = row.get(0)?;
            let status: String = row.get(1)?;
            let hora: String = row
                .get(2)
                .unwrap_or_else(|_| "--/--/---- --:--:--".to_string());
            Ok((titulo, status, hora))
        })?;

        let mut list = Vec::new();
        for r in rows.flatten() {
            list.push(r);
        }
        Ok(list)
    }

    pub fn normalize(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'á' | 'à' | 'ã' | 'â' | 'ä' | 'Á' | 'À' | 'Ã' | 'Â' | 'Ä' => {
                    result.push('a')
                }
                'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => result.push('e'),
                'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => result.push('i'),
                'ó' | 'ò' | 'õ' | 'ô' | 'ö' | 'Ó' | 'Ò' | 'Õ' | 'Ô' | 'Ö' => {
                    result.push('o')
                }
                'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => result.push('u'),
                'ç' | 'Ç' => result.push('c'),
                '&' => result.push_str(" e "),
                c if c.is_alphanumeric() || c == ' ' => {
                    for lc in c.to_lowercase() {
                        result.push(lc);
                    }
                }
                _ => result.push(' '),
            }
        }
        result.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Compara se o nome de um arquivo físico no disco (sem extensão) corresponde
    /// estritamente ao título da faixa, eliminando colisões falsas por substrings.
    pub fn is_matching_audio_file(file_stem: &str, title: &str) -> bool {
        // 1. Igualdade exata insensível a maiúsculas/minúsculas
        if file_stem.eq_ignore_ascii_case(title) {
            return true;
        }

        let norm_stem = Self::normalize(file_stem);
        let norm_title = Self::normalize(title);

        // 2. Igualdade exata das strings normalizadas (sem acentos, pontuação, múltiplos espaços)
        if norm_stem == norm_title {
            return true;
        }

        // 3. Remoção de sufixos de mídia/versão em parênteses ou colchetes no nome do arquivo físico
        // Ex: "Djavan - Se (Áudio Oficial)" -> "Djavan - Se"
        let clean_stem = if let Some(idx) = file_stem.find(['(', '[']) {
            file_stem[..idx].trim()
        } else {
            file_stem.trim()
        };
        let norm_clean_stem = Self::normalize(clean_stem);
        if norm_clean_stem == norm_title {
            return true;
        }

        // 4. Remoção de sufixos de mídia/versão no título pesquisado
        // Ex: se o título for "Paul McCartney - Let 'Em In (with Wings)" e o arquivo for "Paul McCartney - Let 'Em In.mp3"
        let clean_title = if let Some(idx) = title.find(['(', '[']) {
            title[..idx].trim()
        } else {
            title.trim()
        };
        let norm_clean_title = Self::normalize(clean_title);
        if norm_stem == norm_clean_title || norm_clean_stem == norm_clean_title {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delete_track_and_physical_file() {
        let unique = format!(
            "test_db_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp_dir = std::env::temp_dir().join(&unique);
        fs::create_dir_all(&temp_dir).unwrap();

        let db_path = temp_dir.join("test.db");
        let db = Database::open(db_path.to_str().unwrap()).unwrap();

        let track_title = "Nikka Costa - (Out Here) On My Own";
        db.record(track_title, "SUCESSO").unwrap();

        // Cria o arquivo físico simulado .mp3 no diretório
        let mp3_path = temp_dir.join(format!("{}.mp3", track_title));
        fs::write(&mp3_path, b"dummy audio content").unwrap();
        assert!(mp3_path.exists());

        // Valida que delete_track remove do banco e do disco
        db.delete_track(track_title, temp_dir.to_str().unwrap())
            .unwrap();

        assert!(
            !mp3_path.exists(),
            "O arquivo mp3 deve ser deletado do disco"
        );

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.0, 0, "Registro deve ter sido removido do SQLite");

        // Limpeza
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_processed_word_boundary_anti_substring_collision() {
        let unique = format!(
            "test_db_boundary_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp_dir = std::env::temp_dir().join(&unique);
        fs::create_dir_all(&temp_dir).unwrap();

        let db_path = temp_dir.join("test.db");
        let db = Database::open(db_path.to_str().unwrap()).unwrap();

        // Cria o arquivo físico "Ivan Lins - Somos todos iguais nesta noite.mp3"
        let existing_file = "Ivan Lins - Somos todos iguais nesta noite";
        let mp3_path = temp_dir.join(format!("{}.mp3", existing_file));
        fs::write(&mp3_path, vec![0u8; 600 * 1024]).unwrap();

        // 1. "A Noite" NÃO deve casar como substring dentro de "nesta noite"
        let short_title = "Ivan Lins - A Noite";
        let processed = db
            .is_processed(short_title, temp_dir.to_str().unwrap())
            .unwrap();
        assert!(
            !processed,
            "'A Noite' não deve colidir como substring de 'nesta noite'"
        );

        // 2. A faixa real existente DEVE ser reconhecida
        let real_match = db
            .is_processed(existing_file, temp_dir.to_str().unwrap())
            .unwrap();
        assert!(
            real_match,
            "A faixa real idêntica deve ser reconhecida no disco"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reconcile_with_disk_restores_missing_files_and_preserves_distinct_tracks() {
        let unique = format!(
            "test_db_reconcile_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp_dir = std::env::temp_dir().join(&unique);
        fs::create_dir_all(&temp_dir).unwrap();

        let db_path = temp_dir.join("test.db");
        let db = Database::open(db_path.to_str().unwrap()).unwrap();

        // 1. Faixa com nome compartilhado menor (ex: Gilberto Gil - Estrela)
        let track1 = "Gilberto Gil - Estrela";
        db.record(track1, "SUCESSO").unwrap();
        let mp3_1 = temp_dir.join(format!("{}.mp3", track1));
        fs::write(&mp3_1, vec![0u8; 600 * 1024]).unwrap();

        // 2. Faixa mais longa com palavras em comum no disco (ex: Gilberto Gil - Amarra o teu arado a uma estrela)
        let track2 = "Gilberto Gil - Amarra o teu arado a uma estrela";
        let mp3_2 = temp_dir.join(format!("{}.mp3", track2));
        fs::write(&mp3_2, vec![0u8; 600 * 1024]).unwrap();

        // Executa reconciliação
        db.reconcile_with_disk(temp_dir.to_str().unwrap()).unwrap();

        // Ambas as faixas DEVEM existir no banco como SUCESSO
        let (suc_after, _, _, _) = db.get_stats().unwrap();
        assert_eq!(suc_after, 2, "Ambas as faixas devem existir no banco");

        let all_hist = db.get_all_history().unwrap();
        let titles: Vec<String> = all_hist.into_iter().map(|h| h.0).collect();
        assert!(titles.contains(&track1.to_string()));
        assert!(titles.contains(&track2.to_string()));

        // 3. Faixa no banco cujo arquivo físico foi removido deve transicionar para AUSENTE
        let ghost = "Missing Artist - Missing Track";
        db.record(ghost, "SUCESSO").unwrap();
        let (suc_ghost, _, _, mis_before) = db.get_stats().unwrap();
        assert_eq!(suc_ghost, 3);
        assert_eq!(mis_before, 0);

        db.reconcile_with_disk(temp_dir.to_str().unwrap()).unwrap();

        let (suc_final, _, _, mis_after) = db.get_stats().unwrap();
        assert_eq!(
            suc_final, 2,
            "Apenas as 2 faixas com arquivo físico devem ser SUCESSO"
        );
        assert_eq!(
            mis_after, 1,
            "A faixa sem arquivo físico deve ser marcada como AUSENTE"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_matching_audio_file_strict_anti_substring() {
        // Casos que DEVEM casar:
        assert!(Database::is_matching_audio_file(
            "Djavan - Se",
            "Djavan - Se"
        ));
        assert!(Database::is_matching_audio_file(
            "djavan - se",
            "Djavan - Se"
        ));
        assert!(Database::is_matching_audio_file(
            "Djavan - Se (Áudio Oficial)",
            "Djavan - Se"
        ));
        assert!(Database::is_matching_audio_file(
            "Djavan - Se [Official Audio]",
            "Djavan - Se"
        ));
        assert!(Database::is_matching_audio_file(
            "Paul McCartney - Let 'Em In",
            "Paul McCartney - Let 'Em In (with Wings)"
        ));
        assert!(Database::is_matching_audio_file(
            "Paul McCartney - Let 'Em In (with Wings)",
            "Paul McCartney - Let 'Em In"
        ));
        assert!(Database::is_matching_audio_file(
            "Nikka Costa - (Out Here) On My Own",
            "Nikka Costa - (Out Here) On My Own"
        ));

        // Casos de regressão que NUNCA DEVEM casar (bug crítico do Djavan - Se):
        assert!(!Database::is_matching_audio_file(
            "Djavan - Seduzir",
            "Djavan - Se"
        ));
        assert!(!Database::is_matching_audio_file(
            "Djavan - Segredo",
            "Djavan - Se"
        ));
        assert!(!Database::is_matching_audio_file(
            "Djavan - Se Eu Não Te Amasse Tanto Assim",
            "Djavan - Se"
        ));
        assert!(!Database::is_matching_audio_file(
            "Djavan - Esse Brilho Em Teu Olhar",
            "Djavan - Se"
        ));
        assert!(!Database::is_matching_audio_file(
            "Queen - Love of My Life",
            "Queen - Love"
        ));
        assert!(!Database::is_matching_audio_file(
            "Gilberto Gil - Amarra o teu arado a uma estrela",
            "Gilberto Gil - Estrela"
        ));
        assert!(!Database::is_matching_audio_file(
            "Ivan Lins - Somos todos iguais nesta noite",
            "Ivan Lins - A Noite"
        ));
    }
}
