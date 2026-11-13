//! tflip-studio: a web-based admin tool for tflip.

#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

mod connection;
mod error;
mod guard;
mod repository;
mod socket;
mod vault;

use connection::Connections;
use error::{ApiError, Failure};
use guard::Token;
use repository::{Repository, RepositoryEntry, Stored};
use vault::Vault;

/// Loopback only: the server opens any table its user can, so nothing else on
/// the network may reach it. The port is the one after tflipd's 6262, so both
/// can run on one machine.
const ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6263);

/// An object rather than a bare string, so fields can be added without
/// breaking the frontend.
#[derive(Serialize, TS)]
#[ts(export)]
struct Version {
    version: &'static str,
}

async fn version() -> Json<Version> {
    Json(Version {
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Deserialize, TS)]
#[ts(export)]
struct ConnectRequest {
    path: String,
    user: Option<String>,
    password: Option<String>,
    /// Keeps the password in the repository, sealed under the master
    /// password.
    #[serde(default)]
    save_password: bool,
}

#[derive(Serialize, TS)]
#[ts(export)]
struct Connected {
    id: u32,
}

/// Not the engine's `ConnectionInfo`: its `ServerType` has no TypeScript
/// binding, and is always local for now.
#[derive(Serialize, TS)]
#[ts(export)]
struct ConnectionView {
    path: String,
    dictionary: bool,
    user: Option<String>,
}

#[derive(Deserialize, TS)]
#[ts(export)]
struct SqlRequest {
    sql: String,
}

#[derive(Serialize, TS)]
#[ts(export)]
struct VaultState {
    /// Whether a master password was set: the frontend asks to choose one
    /// if not, and to enter it if so.
    exists: bool,
    unlocked: bool,
}

#[derive(Deserialize, TS)]
#[ts(export)]
struct UnlockRequest {
    password: String,
}

#[derive(Clone)]
struct Studio {
    connections: Arc<Connections>,
    repository: Arc<Repository>,
    vault: Arc<Vault>,
    /// Only the first Connect page after a start reopens: later ones are
    /// where a Disconnect leads, and must stay there.
    reopened: Arc<AtomicBool>,
}

impl FromRef<Studio> for Arc<Connections> {
    fn from_ref(studio: &Studio) -> Self {
        studio.connections.clone()
    }
}

impl FromRef<Studio> for Arc<Repository> {
    fn from_ref(studio: &Studio) -> Self {
        studio.repository.clone()
    }
}

/// Every Repository call reads or writes its table, and Argon2 takes a
/// while on purpose.
async fn blocking<R: Send + 'static, E: Send + 'static>(
    f: impl FnOnce() -> Result<R, E> + Send + 'static,
) -> Result<R, E> {
    tokio::task::spawn_blocking(f)
        .await
        .expect("a repository call panicked")
}

/// What a stored password is sealed to: the database and user it opens, so
/// it cannot be copied to another entry.
fn binding(path: &str, user: Option<&str>) -> Vec<u8> {
    format!("{path}\0{}", user.unwrap_or_default()).into_bytes()
}

/// `save` seals the password before connecting, so a locked vault stops
/// the connect rather than leaving it open with the password unsaved.
async fn open_and_remember(
    studio: &Studio,
    path: String,
    user: Option<String>,
    password: Option<String>,
    save: bool,
) -> Result<Json<Connected>, Failure> {
    let sealed = match (&password, save) {
        (Some(password), true) => Some(
            studio
                .vault
                .seal(password, &binding(&path, user.as_deref()))?,
        ),
        _ => None,
    };
    let id = studio
        .connections
        .open_local(path.clone(), user.clone(), password)
        .await?;
    let repository = studio.repository.clone();
    let remembered =
        blocking(move || repository.remember(&path, user.as_deref(), sealed.as_deref())).await;
    // The connection is open: a repository that cannot be written must not
    // turn that into a failure.
    if let Err(error) = remembered {
        eprintln!(
            "cannot remember the connection: {}",
            ApiError::from(error).message
        );
    }
    Ok(Json(Connected { id }))
}

async fn connect(
    State(studio): State<Studio>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<Connected>, Failure> {
    let ConnectRequest {
        path,
        user,
        password,
        save_password,
    } = request;
    open_and_remember(&studio, path, user, password, save_password).await
}

async fn entries(
    State(repository): State<Arc<Repository>>,
) -> Result<Json<Vec<RepositoryEntry>>, ApiError> {
    let stored = blocking(move || repository.list()).await?;
    Ok(Json(stored.into_iter().map(|s| s.entry).collect()))
}

/// The password `stored` keeps, opened; `None` if it keeps none.
fn stored_password(studio: &Studio, stored: &Stored) -> Result<Option<String>, Failure> {
    let Some(sealed) = &stored.sealed else {
        return Ok(None);
    };
    let entry = &stored.entry;
    let binding = binding(&entry.path, entry.user.as_deref());
    Ok(Some(studio.vault.open(sealed, &binding)?))
}

async fn vault_state(State(studio): State<Studio>) -> Result<Json<VaultState>, ApiError> {
    let repository = studio.repository.clone();
    let master = blocking(move || repository.master()).await?;
    Ok(Json(VaultState {
        exists: master.is_some(),
        unlocked: studio.vault.is_unlocked(),
    }))
}

/// Sets the master password if there is none yet, and unlocks with it.
async fn unlock(
    State(studio): State<Studio>,
    Json(request): Json<UnlockRequest>,
) -> Result<StatusCode, Failure> {
    blocking(move || {
        match studio.repository.master()? {
            Some(master) => studio.vault.unlock(&request.password, &master)?,
            None => {
                let master = studio.vault.create(&request.password);
                studio.repository.set_master(&master)?;
            }
        }
        Ok::<_, Failure>(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget(
    State(repository): State<Arc<Repository>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    blocking(move || repository.forget(&id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// With its stored password, if it keeps one; an entry with a user and
/// none needs the Connect form.
async fn connect_entry(
    State(studio): State<Studio>,
    Path(id): Path<String>,
) -> Result<Json<Connected>, Failure> {
    let repository = studio.repository.clone();
    let stored = blocking(move || repository.get(&id)).await?;
    let password = stored_password(&studio, &stored)?;
    let entry = stored.entry;
    open_and_remember(&studio, entry.path, entry.user, password, false).await
}

/// 204 when there is nothing to reopen: not the first call since the start,
/// an empty repository, or a last entry with a user and no stored password.
/// 423 still counts as the one reopen: the frontend connects the entry
/// itself once the vault is unlocked.
async fn reopen(State(studio): State<Studio>) -> Result<Response, Failure> {
    if studio.reopened.swap(true, Ordering::Relaxed) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let repository = studio.repository.clone();
    let stored = blocking(move || repository.list()).await?;
    let Some(last) = stored.into_iter().next() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    if last.entry.user.is_some() && last.sealed.is_none() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let password = stored_password(&studio, &last)?;
    let entry = last.entry;
    Ok(
        open_and_remember(&studio, entry.path, entry.user, password, false)
            .await?
            .into_response(),
    )
}

async fn connection(
    State(connections): State<Arc<Connections>>,
    Path(id): Path<u32>,
) -> Result<Json<ConnectionView>, ApiError> {
    let info = connections
        .get(id)?
        .call(|api, handle| api.connection_info(handle))
        .await?;
    Ok(Json(ConnectionView {
        path: info.path,
        dictionary: info.dictionary,
        user: info.user,
    }))
}

async fn disconnect(
    State(connections): State<Arc<Connections>>,
    Path(id): Path<u32>,
) -> Result<StatusCode, ApiError> {
    connections.close(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify(
    State(connections): State<Arc<Connections>>,
    Path(id): Path<u32>,
    Json(request): Json<SqlRequest>,
) -> Result<StatusCode, ApiError> {
    connections
        .get(id)?
        .call(move |api, handle| {
            let statement = api.create_statement(handle)?;
            let verified = api.verify(statement, request.sql);
            let closed = api.close_statement(statement);
            // The SQL's error matters more than a failed close.
            verified.and(closed)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tflip-studio: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Apart from `run`, so tests call the very routes the server serves, without
/// a socket; takes the token so they can pass one they know.
fn app(token: Token, repository: Repository) -> Router {
    let studio = Studio {
        connections: Arc::new(Connections::default()),
        repository: Arc::new(repository),
        vault: Arc::default(),
        reopened: Arc::default(),
    };
    let api = Router::new()
        .route("/api/version", get(version))
        .route("/api/connections", post(connect))
        .route("/api/connections/{id}", get(connection).delete(disconnect))
        .route("/api/connections/{id}/verify", post(verify))
        .route("/api/repository", get(entries))
        .route("/api/repository/reopen", post(reopen))
        .route("/api/repository/{id}", delete(forget))
        .route("/api/repository/{id}/connect", post(connect_entry))
        .route("/api/vault", get(vault_state))
        .route("/api/vault/unlock", post(unlock))
        .with_state(studio.clone())
        .route_layer(middleware::from_fn_with_state(
            token.clone(),
            guard::check_token,
        ));
    let socket = Router::new()
        .route("/api/connections/{id}/socket", get(socket::open))
        .with_state(studio)
        .route_layer(middleware::from_fn_with_state(token, guard::check_socket));
    // Added last, so it is outermost and also covers the static files and 404s.
    Router::new()
        .merge(api)
        .merge(socket)
        .layer(middleware::from_fn(guard::check_host))
}

async fn run() -> anyhow::Result<()> {
    let token = Token::random()?;
    let listener = tokio::net::TcpListener::bind(ADDRESS)
        .await
        .with_context(|| format!("cannot listen on {ADDRESS}"))?;
    println!("tflip-studio on http://{ADDRESS}/#token={}", token.0);
    // Vite's default port: `cargo run` builds in debug, next to `npm run dev`.
    if cfg!(debug_assertions) {
        println!("dev frontend: http://localhost:5173/#token={}", token.0);
    }
    // Roaming on Windows: ADS data usually sits on mapped drives and UNC
    // paths, which are the same on every machine of the domain.
    let folder = dirs::data_dir()
        .context("no folder for application data")?
        .join("tflip-studio");
    let repository = Repository::open(&folder)?;
    axum::serve(listener, app(token, repository))
        .await
        .context("serving")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use axum::response::Response;
    use serde_json::{Value, json};
    use tflip_engine::ace_error::codes::AE_INVALID_HANDLE;
    use tower::ServiceExt;

    use super::*;

    const TOKEN: &str = "secret";

    /// Each with a repository of its own, as tests run at once.
    fn studio() -> Router {
        app(Token(TOKEN.into()), Repository::open(&scratch()).unwrap())
    }

    fn scratch() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-studio-{}-app-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn fixture() -> String {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/all_types");
        // As the connection reports it, which on Windows is the full path.
        tflip_storage::universal_path(path.to_str().unwrap())
    }

    async fn send(host: &str, token: Option<&str>) -> Response {
        let mut builder = Request::get("/api/version").header(header::HOST, host);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = builder.body(Body::empty()).unwrap();
        studio().oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn serves_the_version() {
        let response = send(&ADDRESS.to_string(), Some(TOKEN)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let expected = format!(r#"{{"version":"{}"}}"#, env!("CARGO_PKG_VERSION"));
        assert_eq!(&body[..], expected.as_bytes());
    }

    #[tokio::test]
    async fn rejects_a_missing_or_wrong_token() {
        for token in [None, Some("wrong")] {
            let response = send(&ADDRESS.to_string(), token).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{token:?}");
        }
    }

    /// With the right token, so only the Host check can have rejected it.
    #[tokio::test]
    async fn rejects_a_foreign_host_even_with_the_token() {
        let response = send("evil.example:6263", Some(TOKEN)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// Through one `app`, so the connections outlive each request; with the
    /// studio's own Host and the token, as the frontend sends them.
    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, ADDRESS.to_string())
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
        let request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        };
        let response = app.clone().oneshot(request.unwrap()).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    #[tokio::test]
    async fn connects_shows_and_disconnects() {
        let app = studio();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/all_types");
        // As the connection reports it, which on Windows is the full path.
        let path = tflip_storage::universal_path(path.to_str().unwrap());

        let body = json!({ "path": path });
        let (status, body) = call(&app, Method::POST, "/api/connections", Some(body)).await;
        assert_eq!(status, StatusCode::OK);
        let uri = format!("/api/connections/{}", body["id"]);

        let (status, body) = call(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK);
        let expected = json!({ "path": path, "dictionary": false, "user": null });
        assert_eq!(body, expected);

        let (status, _) = call(&app, Method::DELETE, &uri, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], AE_INVALID_HANDLE);
    }

    /// The engine's error comes back as an `ApiError`, not a status alone.
    #[tokio::test]
    async fn reports_a_failed_connect() {
        let app = studio();
        let body = json!({ "path": "/nowhere/missing.add" });
        let (status, body) = call(&app, Method::POST, "/api/connections", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["code"].as_u64().is_some_and(|code| code != 0));
    }

    #[tokio::test]
    async fn verifies_sql() {
        let app = studio();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/all_types");
        let body = json!({ "path": tflip_storage::universal_path(path.to_str().unwrap()) });
        let (_, body) = call(&app, Method::POST, "/api/connections", Some(body)).await;
        let uri = format!("/api/connections/{}/verify", body["id"]);

        let good = json!({ "sql": "SELECT 1 FROM system.iota" });
        let (status, _) = call(&app, Method::POST, &uri, Some(good)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let bad = json!({ "sql": "SELEKT" });
        let (status, body) = call(&app, Method::POST, &uri, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["code"].as_u64().is_some_and(|code| code != 0));
    }

    /// Both answers come from the guard, before any upgrade, so no WebSocket
    /// client is needed.
    #[tokio::test]
    async fn guards_the_socket_with_origin_and_query_token() {
        let socket = |origin: Option<&str>, token: &str| {
            let mut builder = Request::get(format!("/api/connections/0/socket?token={token}"))
                .header(header::HOST, ADDRESS.to_string());
            if let Some(origin) = origin {
                builder = builder.header(header::ORIGIN, origin);
            }
            studio().oneshot(builder.body(Body::empty()).unwrap())
        };
        let own = format!("http://{ADDRESS}");

        let response = socket(None, TOKEN).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = socket(Some("http://evil.example:6263"), TOKEN)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = socket(Some(&own), "wrong").await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remembers_connections_and_reopens_the_last_once() {
        let folder = scratch();
        let app = app(Token(TOKEN.into()), Repository::open(&folder).unwrap());
        let body = json!({ "path": fixture() });
        call(&app, Method::POST, "/api/connections", Some(body)).await;
        let (status, body) = call(&app, Method::GET, "/api/repository", None).await;
        assert_eq!(status, StatusCode::OK);
        let entry = &body[0];
        assert_eq!(
            (&entry["name"], &entry["path"], &entry["user"]),
            (&json!("all_types"), &json!(fixture()), &Value::Null)
        );
        let entry = format!("/api/repository/{}", entry["id"].as_str().unwrap());

        // A restart: a new app on the same repository.
        let app = super::app(Token(TOKEN.into()), Repository::open(&folder).unwrap());
        let (status, body) = call(&app, Method::POST, "/api/repository/reopen", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            Method::GET,
            &format!("/api/connections/{}", body["id"]),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(&app, Method::POST, "/api/repository/reopen", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(&app, Method::POST, &format!("{entry}/connect"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["id"].is_number());

        let (status, _) = call(&app, Method::DELETE, &entry, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = call(&app, Method::GET, "/api/repository", None).await;
        assert_eq!(body, json!([]));
    }

    /// Through a restart, as the frontend meets it: a saved password needs
    /// the master password before it connects, and a wrong one is refused.
    #[tokio::test]
    async fn saves_passwords_under_the_master_password() {
        let folder = scratch();
        let app = app(Token(TOKEN.into()), Repository::open(&folder).unwrap());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/adt/dictionary/dict.add");
        let path = tflip_storage::universal_path(path.to_str().unwrap());
        let login = json!({
            "path": path, "user": "ADSSYS", "password": "adssyspw", "save_password": true
        });

        let (status, _) = call(&app, Method::POST, "/api/connections", Some(login.clone())).await;
        assert_eq!(status, StatusCode::LOCKED);
        let (_, body) = call(&app, Method::GET, "/api/vault", None).await;
        assert_eq!(body, json!({ "exists": false, "unlocked": false }));
        let master = json!({ "password": "master" });
        let (status, _) = call(
            &app,
            Method::POST,
            "/api/vault/unlock",
            Some(master.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = call(&app, Method::POST, "/api/connections", Some(login)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = call(&app, Method::GET, "/api/repository", None).await;
        assert_eq!(body[0]["saved"], true);

        let app = super::app(Token(TOKEN.into()), Repository::open(&folder).unwrap());
        let (status, _) = call(&app, Method::POST, "/api/repository/reopen", None).await;
        assert_eq!(status, StatusCode::LOCKED);
        let (_, body) = call(&app, Method::GET, "/api/vault", None).await;
        assert_eq!(body, json!({ "exists": true, "unlocked": false }));
        let wrong = json!({ "password": "wrong" });
        let (status, body) = call(&app, Method::POST, "/api/vault/unlock", Some(wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["code"].as_u64().is_some_and(|code| code != 0));
        let (status, _) = call(&app, Method::POST, "/api/vault/unlock", Some(master)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = call(&app, Method::GET, "/api/repository", None).await;
        let uri = format!(
            "/api/repository/{}/connect",
            body[0]["id"].as_str().unwrap()
        );
        let (status, body) = call(&app, Method::POST, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}
