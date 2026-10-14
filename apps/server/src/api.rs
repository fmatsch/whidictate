use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use whidictate_core::protocol::{DictationSummary, LoginRequest, LoginResponse, Segment, UploadMeta, UserInfo};

use crate::auth::{self, AuthUser, COOKIE, DEVICE_SECS, SESSION_SECS};
use crate::db::now;
use crate::AppState;

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        Self(code, msg.into())
    }
    fn bad(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "keine Berechtigung")
    }
    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "nicht gefunden")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        tracing::error!("Datenbankfehler: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Datenbankfehler")
    }
}

type Res<T> = Result<T, ApiError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/me/password", post(change_password))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/{id}", axum::routing::patch(update_user))
        .route("/api/dictations", get(list_dictations).post(create_dictation))
        .route("/api/dictations/{id}", get(get_dictation).delete(delete_dictation))
        .route("/api/dictations/{id}/audio", get(get_audio))
        .route("/api/dictations/{id}/text", axum::routing::put(put_text))
        .route("/api/dictations/{id}/status", post(set_status))
        .route("/api/dictations/{id}/assign", post(assign))
        .route("/api/dictations/{id}/history", get(history))
        .route("/api/dictations/{id}/meta", axum::routing::put(put_meta))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}

// ---------- Anmeldung ----------

async fn login(State(st): State<AppState>, Json(req): Json<LoginRequest>) -> Res<Response> {
    let key = req.username.to_lowercase();
    {
        let f = st.failures.lock().unwrap();
        if let Some((n, until)) = f.get(&key) {
            if *n >= 5 && *until > now() {
                return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Zu viele Fehlversuche, bitte kurz warten"));
            }
        }
    }
    let user = st.db.find_user_by_name(&req.username);
    let ok = match &user {
        Some((_, _, _, _, hash, active)) => auth::verify_password(&req.password, hash) && *active,
        None => {
            // Zeitverhalten angleichen, damit Benutzernamen nicht erraten werden können.
            let _ = auth::verify_password(&req.password, &st.dummy_hash);
            false
        }
    };
    if !ok {
        let mut f = st.failures.lock().unwrap();
        let e = f.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 = now() + 60;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Benutzername oder Passwort falsch"));
    }
    st.failures.lock().unwrap().remove(&key);
    let (id, username, display_name, role, _, _) = user.unwrap();
    let token = auth::new_token();
    let lifetime = if req.device { DEVICE_SECS } else { SESSION_SECS };
    st.db.0.lock().unwrap().execute(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1,?2,?3,?4)",
        params![auth::token_hash(&token), id, now(), now() + lifetime],
    )?;
    let body = LoginResponse { token: token.clone(), user: UserInfo { id, username, display_name, role } };
    let mut resp = Json(body).into_response();
    let secure = if st.secure_cookies { "; Secure" } else { "" };
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={lifetime}{secure}"))
            .unwrap(),
    );
    Ok(resp)
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Res<Response> {
    if let Some(t) = auth::extract_token(&headers) {
        st.db.0.lock().unwrap().execute("DELETE FROM sessions WHERE token_hash = ?1", [auth::token_hash(&t)])?;
    }
    let mut resp = Json(json!({ "ok": true })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")).unwrap(),
    );
    Ok(resp)
}

async fn me(user: AuthUser) -> Json<UserInfo> {
    Json(user.0)
}

#[derive(Deserialize)]
struct PwChange {
    old: String,
    new: String,
}

async fn change_password(State(st): State<AppState>, user: AuthUser, Json(b): Json<PwChange>) -> Res<Json<serde_json::Value>> {
    if b.new.chars().count() < 8 {
        return Err(ApiError::bad("Das neue Passwort muss mindestens 8 Zeichen haben"));
    }
    let (_, _, _, _, hash, _) = st.db.find_user_by_name(&user.0.username).ok_or_else(ApiError::not_found)?;
    if !auth::verify_password(&b.old, &hash) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Altes Passwort falsch"));
    }
    let new_hash = auth::hash_password(&b.new).map_err(|e| ApiError::bad(e.to_string()))?;
    st.db.0.lock().unwrap().execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![new_hash, user.0.id])?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- Benutzer ----------

async fn list_users(State(st): State<AppState>, user: AuthUser) -> Res<Json<Vec<serde_json::Value>>> {
    if !user.is_staff() {
        return Err(ApiError::forbidden());
    }
    let conn = st.db.0.lock().unwrap();
    let mut stmt = conn.prepare("SELECT id, username, display_name, role, active FROM users ORDER BY display_name")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({ "id": r.get::<_, String>(0)?, "username": r.get::<_, String>(1)?,
                       "display_name": r.get::<_, String>(2)?, "role": r.get::<_, String>(3)?,
                       "active": r.get::<_, i64>(4)? != 0 }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct NewUser {
    pub username: String,
    pub display_name: String,
    pub password: String,
    pub role: String,
}

pub fn insert_user(st: &AppState, u: &NewUser) -> Result<String, String> {
    if !["author", "secretary", "admin"].contains(&u.role.as_str()) {
        return Err("Rolle muss author, secretary oder admin sein".into());
    }
    if u.username.trim().is_empty() || u.password.chars().count() < 8 {
        return Err("Benutzername darf nicht leer sein, das Passwort braucht mindestens 8 Zeichen".into());
    }
    let hash = auth::hash_password(&u.password).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    st.db
        .0
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, u.username.trim(), u.display_name.trim(), hash, u.role, now()],
        )
        .map_err(|e| if e.to_string().contains("UNIQUE") { "Benutzername existiert bereits".to_string() } else { e.to_string() })?;
    Ok(id)
}

async fn create_user(State(st): State<AppState>, user: AuthUser, Json(u): Json<NewUser>) -> Res<(StatusCode, Json<serde_json::Value>)> {
    if !user.is_admin() {
        return Err(ApiError::forbidden());
    }
    let id = insert_user(&st, &u).map_err(ApiError::bad)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Deserialize)]
struct UserPatch {
    password: Option<String>,
    active: Option<bool>,
    role: Option<String>,
    display_name: Option<String>,
}

async fn update_user(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, Json(p): Json<UserPatch>) -> Res<Json<serde_json::Value>> {
    if !user.is_admin() {
        return Err(ApiError::forbidden());
    }
    if id == user.0.id && (p.active == Some(false) || p.role.as_deref().is_some_and(|r| r != "admin")) {
        return Err(ApiError::bad("Du kannst dich nicht selbst sperren oder herabstufen"));
    }
    let conn = st.db.0.lock().unwrap();
    if let Some(pw) = &p.password {
        if pw.chars().count() < 8 {
            return Err(ApiError::bad("Passwort braucht mindestens 8 Zeichen"));
        }
        let h = auth::hash_password(pw).map_err(|e| ApiError::bad(e.to_string()))?;
        conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![h, id])?;
        conn.execute("DELETE FROM sessions WHERE user_id = ?1", [&id])?;
    }
    if let Some(a) = p.active {
        conn.execute("UPDATE users SET active = ?1 WHERE id = ?2", params![a as i64, id])?;
        if !a {
            conn.execute("DELETE FROM sessions WHERE user_id = ?1", [&id])?;
        }
    }
    if let Some(r) = p.role {
        if !["author", "secretary", "admin"].contains(&r.as_str()) {
            return Err(ApiError::bad("ungültige Rolle"));
        }
        conn.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![r, id])?;
    }
    if let Some(n) = p.display_name {
        conn.execute("UPDATE users SET display_name = ?1 WHERE id = ?2", params![n, id])?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------- Diktate ----------

const SUMMARY_SELECT: &str = "SELECT d.id, d.client_ref, d.title, d.matter, d.priority, d.status, a.display_name,
        s.display_name, d.duration_s, d.created_at, d.updated_at
    FROM dictations d JOIN users a ON a.id = d.author_id LEFT JOIN users s ON s.id = d.assignee_id";

fn row_summary(r: &rusqlite::Row) -> rusqlite::Result<DictationSummary> {
    Ok(DictationSummary {
        id: r.get(0)?,
        client_ref: r.get(1)?,
        title: r.get(2)?,
        matter: r.get(3)?,
        priority: r.get(4)?,
        status: r.get(5)?,
        author: r.get(6)?,
        assignee: r.get(7)?,
        duration_s: r.get::<_, f64>(8)? as f32,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

fn audio_ext(mime: &str) -> &'static str {
    match mime {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/aac" => "m4a",
        "audio/webm" => "webm",
        _ => "bin",
    }
}

async fn create_dictation(State(st): State<AppState>, user: AuthUser, mut mp: Multipart) -> Res<(StatusCode, Json<DictationSummary>)> {
    let mut meta: Option<UploadMeta> = None;
    let mut audio: Option<(Vec<u8>, String)> = None;
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad(e.to_string()))? {
        match field.name().unwrap_or("") {
            "meta" => {
                let t = field.text().await.map_err(|e| ApiError::bad(e.to_string()))?;
                meta = Some(serde_json::from_str(&t).map_err(|e| ApiError::bad(format!("meta ungültig: {e}")))?);
            }
            "audio" => {
                let mime = field.content_type().unwrap_or("audio/wav").to_string();
                let bytes = field.bytes().await.map_err(|e| ApiError::bad(e.to_string()))?;
                audio = Some((bytes.to_vec(), mime));
            }
            _ => {}
        }
    }
    let meta = meta.ok_or_else(|| ApiError::bad("Teil 'meta' fehlt"))?;
    let (bytes, mime) = audio.ok_or_else(|| ApiError::bad("Teil 'audio' fehlt"))?;
    if meta.client_ref.trim().is_empty() {
        return Err(ApiError::bad("client_ref fehlt"));
    }

    // Idempotenz: dieselbe client_ref desselben Autors ergibt dasselbe Diktat.
    {
        let conn = st.db.0.lock().unwrap();
        let existing = conn
            .query_row(&format!("{SUMMARY_SELECT} WHERE d.author_id = ?1 AND d.client_ref = ?2"), params![user.0.id, meta.client_ref], row_summary)
            .optional()?;
        if let Some(s) = existing {
            return Ok((StatusCode::OK, Json(s)));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let file = format!("{id}.{}", audio_ext(&mime));
    tokio::fs::write(st.data_dir.join("audio").join(&file), &bytes)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("Audio konnte nicht gespeichert werden: {e}")))?;

    let t = now();
    let title = if meta.title.trim().is_empty() { meta.text.chars().take(60).collect::<String>() } else { meta.title.clone() };
    let segments = serde_json::to_string(&meta.segments).unwrap_or_else(|_| "[]".into());
    let summary = {
        let conn = st.db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO dictations (id, client_ref, author_id, title, matter, instruction, priority, language, duration_s,
                raw_text, segments_json, text, audio_file, audio_mime, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15)",
            params![id, meta.client_ref, user.0.id, title, meta.matter, meta.instruction, meta.priority.clamp(0, 1),
                    meta.language, meta.duration_s as f64, meta.raw_text, segments, meta.text, file, mime, t],
        )?;
        conn.execute("INSERT INTO revisions (dictation_id, rev, user_id, at, text) VALUES (?1,1,?2,?3,?4)", params![id, user.0.id, t, meta.text])?;
        conn.query_row(&format!("{SUMMARY_SELECT} WHERE d.id = ?1"), [&id], row_summary)?
    };
    st.db.event(&id, &user.0.id, "created", "Diktat eingegangen");
    Ok((StatusCode::CREATED, Json(summary)))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    mine: Option<String>,
    q: Option<String>,
}

async fn list_dictations(State(st): State<AppState>, user: AuthUser, Query(q): Query<ListQuery>) -> Res<Json<Vec<DictationSummary>>> {
    let only_mine = q.mine.as_deref().is_some_and(|m| m == "1" || m == "true") || !user.is_staff();
    let statuses: Vec<String> = q
        .status
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let mut sql = format!("{SUMMARY_SELECT} WHERE 1=1");
    let mut args: Vec<String> = Vec::new();
    if only_mine {
        args.push(user.0.id.clone());
        sql += &format!(" AND d.author_id = ?{}", args.len());
    }
    if !statuses.is_empty() {
        let mut ph = Vec::new();
        for s in statuses {
            args.push(s);
            ph.push(format!("?{}", args.len()));
        }
        sql += &format!(" AND d.status IN ({})", ph.join(","));
    }
    if let Some(text) = q.q.filter(|t| !t.trim().is_empty()) {
        args.push(format!("%{}%", text.trim()));
        let n = args.len();
        sql += &format!(" AND (d.title LIKE ?{n} OR d.matter LIKE ?{n} OR d.text LIKE ?{n} OR a.display_name LIKE ?{n})");
    }
    sql += " ORDER BY d.priority DESC, d.created_at ASC LIMIT 500";
    let conn = st.db.0.lock().unwrap();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_summary)?.collect::<Result<Vec<_>, _>>()?;
    Ok(Json(rows))
}

struct Access {
    author_id: String,
}

fn check_access(st: &AppState, user: &AuthUser, id: &str) -> Res<Access> {
    let author_id: String = st
        .db
        .0
        .lock()
        .unwrap()
        .query_row("SELECT author_id FROM dictations WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(ApiError::not_found)?;
    if user.is_staff() || author_id == user.0.id {
        Ok(Access { author_id })
    } else {
        Err(ApiError::forbidden())
    }
}

#[derive(Serialize)]
struct DictationDetail {
    #[serde(flatten)]
    summary: DictationSummary,
    instruction: String,
    language: String,
    raw_text: String,
    segments: Vec<Segment>,
    text: String,
    rev: i64,
    audio_mime: String,
    author_id: String,
}

async fn get_dictation(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> Res<Json<DictationDetail>> {
    let acc = check_access(&st, &user, &id)?;
    let conn = st.db.0.lock().unwrap();
    let summary = conn.query_row(&format!("{SUMMARY_SELECT} WHERE d.id = ?1"), [&id], row_summary)?;
    let (instruction, language, raw_text, seg_json, text, rev, audio_mime) = conn.query_row(
        "SELECT instruction, language, raw_text, segments_json, text, rev, audio_mime FROM dictations WHERE id = ?1",
        [&id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?, r.get::<_, String>(6)?)),
    )?;
    Ok(Json(DictationDetail {
        summary,
        instruction,
        language,
        raw_text,
        segments: serde_json::from_str(&seg_json).unwrap_or_default(),
        text,
        rev,
        audio_mime,
        author_id: acc.author_id,
    }))
}

async fn get_audio(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, headers: HeaderMap) -> Res<Response> {
    check_access(&st, &user, &id)?;
    let (file, mime): (String, String) = st
        .db
        .0
        .lock()
        .unwrap()
        .query_row("SELECT audio_file, audio_mime FROM dictations WHERE id = ?1", [&id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let data = tokio::fs::read(st.data_dir.join("audio").join(&file))
        .await
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "Audiodatei fehlt"))?;
    let total = data.len();
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    resp_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    resp_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));

    if let Some((start, end)) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| parse_range(v, total)) {
        resp_headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap());
        return Ok((StatusCode::PARTIAL_CONTENT, resp_headers, data[start..=end].to_vec()).into_response());
    }
    Ok((StatusCode::OK, resp_headers, data).into_response())
}

fn parse_range(v: &str, total: usize) -> Option<(usize, usize)> {
    let spec = v.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    if total == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: usize = b.parse().ok()?;
        (total.saturating_sub(n), total - 1)
    } else {
        let s: usize = a.parse().ok()?;
        let e = if b.is_empty() { total - 1 } else { b.parse::<usize>().ok()?.min(total - 1) };
        (s, e)
    };
    (start <= end && start < total).then_some((start, end))
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
    base_rev: i64,
}

async fn put_text(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, Json(b): Json<TextBody>) -> Res<Json<serde_json::Value>> {
    if !user.is_staff() {
        return Err(ApiError::forbidden());
    }
    check_access(&st, &user, &id)?;
    let new_rev = {
        let conn = st.db.0.lock().unwrap();
        let rev: i64 = conn.query_row("SELECT rev FROM dictations WHERE id = ?1", [&id], |r| r.get(0))?;
        if rev != b.base_rev {
            return Err(ApiError::new(StatusCode::CONFLICT, "Das Diktat wurde zwischenzeitlich von jemand anderem geändert. Bitte neu laden."));
        }
        let new_rev = rev + 1;
        let t = now();
        conn.execute(
            "UPDATE dictations SET text = ?1, rev = ?2, updated_at = ?3,
                status = CASE WHEN status = 'new' THEN 'in_progress' ELSE status END,
                assignee_id = COALESCE(assignee_id, ?4) WHERE id = ?5",
            params![b.text, new_rev, t, user.0.id, id],
        )?;
        conn.execute("INSERT INTO revisions (dictation_id, rev, user_id, at, text) VALUES (?1,?2,?3,?4,?5)", params![id, new_rev, user.0.id, t, b.text])?;
        new_rev
    };
    st.db.event(&id, &user.0.id, "edit", &format!("Text bearbeitet (Version {new_rev})"));
    Ok(Json(json!({ "rev": new_rev })))
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
    #[serde(default)]
    note: String,
}

async fn set_status(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, Json(b): Json<StatusBody>) -> Res<Json<serde_json::Value>> {
    if !user.is_staff() {
        return Err(ApiError::forbidden());
    }
    check_access(&st, &user, &id)?;
    if !["new", "in_progress", "done", "returned"].contains(&b.status.as_str()) {
        return Err(ApiError::bad("ungültiger Status"));
    }
    st.db.0.lock().unwrap().execute(
        "UPDATE dictations SET status = ?1, updated_at = ?2,
            assignee_id = CASE WHEN ?1 = 'in_progress' THEN COALESCE(assignee_id, ?3) ELSE assignee_id END WHERE id = ?4",
        params![b.status, now(), user.0.id, id],
    )?;
    let detail = if b.note.trim().is_empty() { format!("Status: {}", b.status) } else { format!("Status: {} – {}", b.status, b.note.trim()) };
    st.db.event(&id, &user.0.id, "status", &detail);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AssignBody {
    user_id: Option<String>,
}

async fn assign(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, Json(b): Json<AssignBody>) -> Res<Json<serde_json::Value>> {
    if !user.is_staff() {
        return Err(ApiError::forbidden());
    }
    check_access(&st, &user, &id)?;
    st.db.0.lock().unwrap().execute("UPDATE dictations SET assignee_id = ?1, updated_at = ?2 WHERE id = ?3", params![b.user_id, now(), id])
        .map_err(|_| ApiError::bad("Benutzer existiert nicht"))?;
    st.db.event(&id, &user.0.id, "assign", &format!("Zugewiesen an {}", b.user_id.as_deref().unwrap_or("niemanden")));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MetaPatch {
    matter: Option<String>,
    title: Option<String>,
    priority: Option<i64>,
}

async fn put_meta(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>, Json(p): Json<MetaPatch>) -> Res<Json<serde_json::Value>> {
    if !user.is_staff() {
        return Err(ApiError::forbidden());
    }
    check_access(&st, &user, &id)?;
    {
        let conn = st.db.0.lock().unwrap();
        if let Some(m) = p.matter {
            conn.execute("UPDATE dictations SET matter = ?1, updated_at = ?2 WHERE id = ?3", params![m, now(), id])?;
        }
        if let Some(t) = p.title {
            conn.execute("UPDATE dictations SET title = ?1, updated_at = ?2 WHERE id = ?3", params![t, now(), id])?;
        }
        if let Some(pr) = p.priority {
            conn.execute("UPDATE dictations SET priority = ?1, updated_at = ?2 WHERE id = ?3", params![pr.clamp(0, 1), now(), id])?;
        }
    }
    st.db.event(&id, &user.0.id, "meta", "Angaben geändert");
    Ok(Json(json!({ "ok": true })))
}

async fn history(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> Res<Json<serde_json::Value>> {
    check_access(&st, &user, &id)?;
    let conn = st.db.0.lock().unwrap();
    let mut ev = conn.prepare("SELECT e.at, u.display_name, e.kind, e.detail FROM events e JOIN users u ON u.id = e.user_id WHERE e.dictation_id = ?1 ORDER BY e.id")?;
    let events = ev
        .query_map([&id], |r| Ok(json!({ "at": r.get::<_, i64>(0)?, "user": r.get::<_, String>(1)?, "kind": r.get::<_, String>(2)?, "detail": r.get::<_, String>(3)? })))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut rv = conn.prepare("SELECT r.rev, r.at, u.display_name, r.text FROM revisions r JOIN users u ON u.id = r.user_id WHERE r.dictation_id = ?1 ORDER BY r.rev DESC")?;
    let revisions = rv
        .query_map([&id], |r| Ok(json!({ "rev": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "user": r.get::<_, String>(2)?, "text": r.get::<_, String>(3)? })))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({ "events": events, "revisions": revisions })))
}

async fn delete_dictation(State(st): State<AppState>, user: AuthUser, Path(id): Path<String>) -> Res<Json<serde_json::Value>> {
    if !user.is_admin() {
        return Err(ApiError::forbidden());
    }
    check_access(&st, &user, &id)?;
    let file: String = st.db.0.lock().unwrap().query_row("SELECT audio_file FROM dictations WHERE id = ?1", [&id], |r| r.get(0))?;
    st.db.0.lock().unwrap().execute("DELETE FROM dictations WHERE id = ?1", [&id])?;
    let _ = tokio::fs::remove_file(st.data_dir.join("audio").join(file)).await;
    Ok(Json(json!({ "ok": true })))
}
