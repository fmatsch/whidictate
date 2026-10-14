//! HTTP-Zugriff auf den whidictate-Server.

use std::time::Duration;

use anyhow::{anyhow, Result};
use whidictate_core::protocol::{DictationSummary, LoginRequest, LoginResponse, UploadMeta};

/// Wie ein fehlgeschlagener Upload behandelt wird.
#[derive(Debug)]
pub enum UploadError {
    /// Netz/Server vorübergehend nicht erreichbar: später erneut versuchen.
    Transient(String),
    /// Token abgelaufen oder ungültig: neu anmelden.
    Unauthorized,
    /// Server lehnt das Diktat dauerhaft ab (z. B. 400).
    Rejected(String),
}

fn agent(secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(secs)))
        .build()
        .into()
}

fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

pub fn health(url: &str) -> Result<String> {
    let mut r = agent(8).get(&format!("{}/api/health", base(url))).call().map_err(|e| anyhow!("Server nicht erreichbar: {e}"))?;
    if !r.status().is_success() {
        return Err(anyhow!("Server antwortet mit Status {}", r.status()));
    }
    let v: serde_json::Value = r.body_mut().read_json().map_err(|e| anyhow!("{e}"))?;
    Ok(v["version"].as_str().unwrap_or("?").to_string())
}

pub fn login(url: &str, username: &str, password: &str) -> Result<LoginResponse> {
    let req = LoginRequest { username: username.into(), password: password.into(), device: true };
    let mut r = agent(15)
        .post(&format!("{}/api/login", base(url)))
        .send_json(&req)
        .map_err(|e| anyhow!("Server nicht erreichbar: {e}"))?;
    if r.status().as_u16() == 401 {
        return Err(anyhow!("Benutzername oder Passwort falsch"));
    }
    if !r.status().is_success() {
        let msg = r.body_mut().read_json::<serde_json::Value>().ok().and_then(|v| v["error"].as_str().map(String::from));
        return Err(anyhow!(msg.unwrap_or_else(|| format!("Status {}", r.status()))));
    }
    r.body_mut().read_json().map_err(|e| anyhow!("{e}"))
}

pub fn upload(url: &str, token: &str, meta: &UploadMeta, wav: &[u8]) -> Result<(), UploadError> {
    let boundary = format!("whidictate{}", uuid::Uuid::new_v4().simple());
    let meta_json = serde_json::to_string(meta).map_err(|e| UploadError::Rejected(e.to_string()))?;
    let mut body = Vec::with_capacity(wav.len() + meta_json.len() + 400);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"meta\"\r\nContent-Type: application/json\r\n\r\n{meta_json}\r\n").as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let mut r = agent(300)
        .post(&format!("{}/api/dictations", base(url)))
        .header("Authorization", &format!("Bearer {token}"))
        .content_type(&format!("multipart/form-data; boundary={boundary}"))
        .send(&body)
        .map_err(|e| UploadError::Transient(e.to_string()))?;
    let code = r.status().as_u16();
    match code {
        200 | 201 => Ok(()),
        401 => Err(UploadError::Unauthorized),
        408 | 429 | 500..=599 => Err(UploadError::Transient(format!("Serverstatus {code}"))),
        _ => {
            let msg = r.body_mut().read_json::<serde_json::Value>().ok().and_then(|v| v["error"].as_str().map(String::from));
            Err(UploadError::Rejected(msg.unwrap_or_else(|| format!("Serverstatus {code}"))))
        }
    }
}

pub fn my_dictations(url: &str, token: &str) -> Result<Vec<DictationSummary>> {
    let mut r = agent(15)
        .get(&format!("{}/api/dictations?mine=1", base(url)))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| anyhow!("Server nicht erreichbar: {e}"))?;
    if r.status().as_u16() == 401 {
        return Err(anyhow!("Sitzung abgelaufen – bitte in den Einstellungen neu anmelden"));
    }
    r.body_mut().read_json().map_err(|e| anyhow!("{e}"))
}

/// Fertiger Text eines Diktats (für "Text kopieren").
pub fn dictation_text(url: &str, token: &str, id: &str) -> Result<String> {
    let mut r = agent(15)
        .get(&format!("{}/api/dictations/{id}", base(url)))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| anyhow!("Server nicht erreichbar: {e}"))?;
    let v: serde_json::Value = r.body_mut().read_json().map_err(|e| anyhow!("{e}"))?;
    Ok(v["text"].as_str().unwrap_or("").to_string())
}

/// Live-Test gegen einen laufenden Server:
/// `WD_TEST_SERVER=http://127.0.0.1:8484 WD_TEST_USER=anna WD_TEST_PASS=... cargo test -p whidictate live -- --ignored`
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn upload_roundtrip() {
        let (Ok(url), Ok(user), Ok(pass)) = (std::env::var("WD_TEST_SERVER"), std::env::var("WD_TEST_USER"), std::env::var("WD_TEST_PASS")) else {
            panic!("WD_TEST_SERVER, WD_TEST_USER, WD_TEST_PASS setzen");
        };
        assert!(health(&url).is_ok());
        assert!(login(&url, &user, "falsch-falsch").is_err());
        let resp = login(&url, &user, &pass).expect("Anmeldung");
        let wav = crate::wav::encode_wav16(&vec![0.1; 16_000]);
        let meta = UploadMeta {
            client_ref: uuid::Uuid::new_v4().to_string(),
            title: "Live-Test".into(),
            matter: "1 Test 2/26".into(),
            instruction: "nur Test".into(),
            priority: 0,
            language: "de".into(),
            duration_s: 1.0,
            text: "Das ist ein Test.".into(),
            raw_text: "Das ist ein Test".into(),
            segments: vec![whidictate_core::protocol::Segment { start: 0.0, end: 1.0, text: "Das ist ein Test".into() }],
        };
        upload(&url, &resp.token, &meta, &wav).expect("Upload");
        upload(&url, &resp.token, &meta, &wav).expect("zweiter Upload (idempotent)");
        let mine = my_dictations(&url, &resp.token).expect("Liste");
        assert_eq!(mine.iter().filter(|d| d.client_ref == meta.client_ref).count(), 1);
        assert!(matches!(upload(&url, "ungueltig", &meta, &wav), Err(UploadError::Unauthorized)));
        // Server nicht erreichbar -> vorübergehender Fehler (bleibt in der Warteschlange)
        assert!(matches!(upload("http://127.0.0.1:9", &resp.token, &meta, &wav), Err(UploadError::Transient(_))));
    }
}
