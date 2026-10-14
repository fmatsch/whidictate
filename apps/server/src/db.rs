use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

pub struct Db(pub Mutex<Connection>);

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let db = Self(Mutex::new(conn));
        db.migrate()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self(Mutex::new(conn));
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE COLLATE NOCASE,
                display_name TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL CHECK (role IN ('author','secretary','admin')),
                active INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                token_hash TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS dictations (
                id TEXT PRIMARY KEY,
                client_ref TEXT NOT NULL,
                author_id TEXT NOT NULL REFERENCES users(id),
                title TEXT NOT NULL,
                matter TEXT NOT NULL,
                instruction TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                language TEXT NOT NULL,
                duration_s REAL NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'new'
                    CHECK (status IN ('new','in_progress','done','returned')),
                assignee_id TEXT REFERENCES users(id),
                raw_text TEXT NOT NULL,
                segments_json TEXT NOT NULL,
                text TEXT NOT NULL,
                rev INTEGER NOT NULL DEFAULT 1,
                audio_file TEXT NOT NULL,
                audio_mime TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE (author_id, client_ref)
             );
             CREATE INDEX IF NOT EXISTS idx_dict_status ON dictations(status, updated_at);
             CREATE TABLE IF NOT EXISTS revisions (
                dictation_id TEXT NOT NULL REFERENCES dictations(id) ON DELETE CASCADE,
                rev INTEGER NOT NULL,
                user_id TEXT NOT NULL REFERENCES users(id),
                at INTEGER NOT NULL,
                text TEXT NOT NULL,
                PRIMARY KEY (dictation_id, rev)
             );
             CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                dictation_id TEXT NOT NULL REFERENCES dictations(id) ON DELETE CASCADE,
                user_id TEXT NOT NULL REFERENCES users(id),
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                detail TEXT NOT NULL
             );",
        )?;
        Ok(())
    }

    pub fn event(&self, dictation: &str, user: &str, kind: &str, detail: &str) {
        let _ = self.0.lock().unwrap().execute(
            "INSERT INTO events (dictation_id, user_id, at, kind, detail) VALUES (?1,?2,?3,?4,?5)",
            params![dictation, user, now(), kind, detail],
        );
    }

    pub fn user_count(&self) -> i64 {
        self.0.lock().unwrap().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap_or(0)
    }

    pub fn find_user_by_name(&self, name: &str) -> Option<(String, String, String, String, String, bool)> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, username, display_name, role, password_hash, active FROM users WHERE username = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)? != 0)),
            )
            .optional()
            .ok()
            .flatten()
    }
}
