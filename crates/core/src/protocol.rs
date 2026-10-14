//! Typen, die Client und Server gemeinsam verwenden.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    /// Sekunden ab Aufnahmebeginn.
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// Metadaten beim Hochladen eines Diktats (Teil "meta" des Multipart-Requests).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMeta {
    /// Vom Client erzeugte ID; macht wiederholte Uploads (Offline-Queue) idempotent.
    pub client_ref: String,
    #[serde(default)]
    pub title: String,
    /// Aktenzeichen / Mandat.
    #[serde(default)]
    pub matter: String,
    /// Anweisung an das Sekretariat ("bitte als Schriftsatz, eilig").
    #[serde(default)]
    pub instruction: String,
    /// 0 = normal, 1 = eilig.
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub duration_s: f32,
    /// Transkript nach Anwendung der Diktierbefehle.
    pub text: String,
    /// Rohtranskript von Whisper.
    #[serde(default)]
    pub raw_text: String,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// true = langlebiges Geräte-Token (Desktop-Client), sonst kurze Browser-Sitzung.
    #[serde(default)]
    pub device: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub display_name: String,
    /// "author", "secretary" oder "admin".
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationSummary {
    pub id: String,
    pub client_ref: String,
    pub title: String,
    pub matter: String,
    pub priority: i64,
    /// "new", "in_progress", "done", "returned".
    pub status: String,
    pub author: String,
    pub assignee: Option<String>,
    pub duration_s: f32,
    pub created_at: i64,
    pub updated_at: i64,
}
