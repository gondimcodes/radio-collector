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
    pub fn reconcile_with_disk(&self, dest_dir: &str) -> Result<usize> {
        let dest_path = Path::new(dest_dir);
        if !dest_path.exists() {
            return Ok(0);
        }

        let mut updated = 0;
        let conn = self.get_conn()?;

        if let Ok(entries) = fs::read_dir(dest_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
                {
                    if let Ok(meta) = path.metadata() {
                        if meta.len() >= 500 * 1024 {
                            let file_stem = path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();

                            let norm_stem = Self::normalize(&file_stem);

                            let mut stmt =
                                conn.prepare("SELECT id, titulo, status FROM historico")?;
                            let rows: Vec<(i64, String, String)> = stmt
                                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                                .flatten()
                                .collect();

                            let mut matched = false;
                            for (id, titulo, status) in rows {
                                let parts: Vec<&str> = titulo.splitn(2, '-').collect();
                                let (artist_p, track_p) = if parts.len() == 2 {
                                    (Self::normalize(parts[0]), Self::normalize(parts[1]))
                                } else {
                                    (String::new(), Self::normalize(&titulo))
                                };
                                let norm_tit = Self::normalize(&titulo);

                                let is_match = (!artist_p.is_empty()
                                    && norm_stem.contains(&artist_p)
                                    && norm_stem.contains(&track_p))
                                    || norm_stem.contains(&norm_tit);

                                if is_match {
                                    matched = true;
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
                    }
                }
            }
        }

        // Limpa registros duplicados de FALHA para faixas que já existem com SUCESSO no disco
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

        // 2. Checagem no sistema de arquivos com casamento estrito de Artista + Música
        let norm_full_title = Self::normalize(title);

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
                    let norm_fname = Self::normalize(&file_stem);
                    let is_match = norm_fname == norm_full_title
                        || (!artist_part.is_empty()
                            && !track_part.is_empty()
                            && norm_fname.contains(&artist_part)
                            && norm_fname.contains(&track_part));
                    if is_match {
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

        let norm_full_title = Self::normalize(title);
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
                    let norm_stem = Self::normalize(&file_stem);

                    // Deleta apenas se coincidir com o título normalizado ou título exato
                    if norm_stem == norm_full_title || file_stem.eq_ignore_ascii_case(title) {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get_stats(&self) -> Result<(usize, usize, usize)> {
        let mut total_success = 0;
        let mut total_failed = 0;
        let mut total_cached = 0;

        let conn = self.get_conn()?;
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM historico GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: usize = row.get(1)?;
            Ok((status, count))
        })?;

        for r in rows.flatten() {
            match r.0.as_str() {
                "SUCESSO" => total_success = r.1,
                "CACHE" => total_cached += r.1,
                "FALHA" | "TIMEOUT" => total_failed += r.1,
                _ => {}
            }
        }

        Ok((total_success, total_failed, total_cached))
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
}
