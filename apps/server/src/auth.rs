use argon2::password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::{header, request::Parts, HeaderMap, StatusCode};
use sha2::{Digest, Sha256};
use whidictate_core::protocol::UserInfo;

use crate::db::now;
use crate::AppState;

pub const SESSION_SECS: i64 = 12 * 3600;
/// Geräte-Token des Desktop-Clients (Anmeldung einmalig, Sperren über Benutzerverwaltung).
pub const DEVICE_SECS: i64 = 90 * 24 * 3600;
pub const COOKIE: &str = "wd_session";

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    Ok(Argon2::default().hash_password(pw.as_bytes()).map_err(|e| anyhow::anyhow!("{e}"))?.to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

pub fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct AuthUser(pub UserInfo);

impl AuthUser {
    pub fn is_staff(&self) -> bool {
        matches!(self.0.role.as_str(), "secretary" | "admin")
    }
    pub fn is_admin(&self) -> bool {
        self.0.role == "admin"
    }
}

pub fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers).ok_or((StatusCode::UNAUTHORIZED, "nicht angemeldet"))?;
        let h = token_hash(&token);
        let conn = state.db.0.lock().unwrap();
        let user = conn
            .query_row(
                "SELECT u.id, u.username, u.display_name, u.role
                 FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.active = 1",
                rusqlite::params![h, now()],
                |r| {
                    Ok(UserInfo { id: r.get(0)?, username: r.get(1)?, display_name: r.get(2)?, role: r.get(3)? })
                },
            )
            .map_err(|_| (StatusCode::UNAUTHORIZED, "Sitzung ungültig oder abgelaufen"))?;
        Ok(AuthUser(user))
    }
}
