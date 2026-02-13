// Streamable HTTP MCP server — binds to localhost, supports graceful shutdown

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::session::SessionRegistry;
use super::state::{AppState, QuestionEnvelope};
use crate::shared::error::{RalphError, Result};

/// Maksymalna liczba prób bindowania portu
const MAX_BIND_RETRIES: u32 = 3;

/// Serwer MCP oparty na Streamable HTTP (axum)
///
/// Binduje się na `127.0.0.1:0` (losowy port przydzielony przez OS),
/// zarządza cyklem życia AppState i wspiera graceful shutdown
/// za pomocą CancellationToken.
pub struct McpServer {
    /// Ścieżka do pliku tasks.yml
    tasks_path: PathBuf,
    /// Kanał do wysyłania pytań do TUI (ask_user)
    question_tx: mpsc::Sender<QuestionEnvelope>,
    /// Token anulowania dla graceful shutdown
    cancellation_token: CancellationToken,
    /// Współdzielony rejestr sesji — dostępny z zewnątrz (orkiestrator)
    session_registry: Arc<SessionRegistry>,
}

impl McpServer {
    /// Tworzy nowy McpServer
    ///
    /// # Arguments
    ///
    /// * `tasks_path` - Ścieżka do pliku tasks.yml
    /// * `question_tx` - Kanał mpsc do komunikacji z TUI
    /// * `cancellation_token` - Token do graceful shutdown
    #[must_use]
    pub fn new(
        tasks_path: PathBuf,
        question_tx: mpsc::Sender<QuestionEnvelope>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            tasks_path,
            question_tx,
            cancellation_token,
            session_registry: Arc::new(SessionRegistry::new()),
        }
    }

    /// Zwraca Arc do rejestru sesji
    ///
    /// Orkiestrator używa go do rejestrowania sesji per-worker,
    /// podczas gdy serwer HTTP współdzieli ten sam rejestr w AppState.
    pub fn session_registry(&self) -> Arc<SessionRegistry> {
        Arc::clone(&self.session_registry)
    }

    /// Rejestruje nową sesję dla zewnętrznego workera
    ///
    /// Wygodne API dla orkiestratora - zamiast wymagać dostępu do rejestru,
    /// serwer bezpośrednio deleguje rejestrację sesji.
    ///
    /// # Arguments
    ///
    /// * `tasks_path` - Ścieżka do pliku tasks.yml dla tej sesji (worktree workera)
    /// * `read_only` - Czy sesja ma być w trybie read-only (true dla workerów orkiestratora)
    ///
    /// # Returns
    ///
    /// UUID sesji jako String - worker przekazuje to ID do Claude przez MCP config
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use tokio::sync::mpsc;
    /// use tokio_util::sync::CancellationToken;
    /// use ralph_wiggum::commands::mcp::server::McpServer;
    ///
    /// # async fn example() {
    /// let (tx, _rx) = mpsc::channel(32);
    /// let token = CancellationToken::new();
    /// let server = McpServer::new(
    ///     PathBuf::from(".ralph/tasks.yml"),
    ///     tx,
    ///     token,
    /// );
    ///
    /// let session_id = server.register_session(PathBuf::from("/tmp/worker1/tasks.yml"), true);
    /// println!("Registered worker session: {}", session_id);
    /// # }
    /// ```
    #[allow(dead_code)] // Public API: used for manual session registration in orchestrator setup
    pub fn register_session(&self, tasks_path: PathBuf, read_only: bool) -> String {
        self.session_registry.create_session(tasks_path, read_only)
    }

    /// Uruchamia serwer HTTP MCP i zwraca port oraz JoinHandle
    ///
    /// Binduje TcpListener na `127.0.0.1:0` z retry (max 3 próby).
    /// Tworzy axum Router z AppState, spawnuje task serwera
    /// z graceful shutdown na CancellationToken.
    ///
    /// # Returns
    ///
    /// `(port, join_handle)` — port na którym nasłuchuje serwer
    /// oraz handle do tasku tokio
    ///
    /// # Errors
    ///
    /// Zwraca `RalphError::Mcp` jeśli bind nie powiedzie się po 3 próbach
    pub async fn start(&self) -> Result<(u16, JoinHandle<()>)> {
        let listener = self.bind_with_retry().await?;
        let port = listener
            .local_addr()
            .map_err(|e| RalphError::Mcp(format!("Failed to get local address: {e}")))?
            .port();

        let state = self.build_app_state();
        let app = Self::build_router(state);

        let cancel = self.cancellation_token.clone();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
                .ok(); // Błąd serwera po shutdown jest oczekiwany
        });

        Ok((port, handle))
    }

    /// Binduje TcpListener z retry logic
    ///
    /// Próbuje zbindować `127.0.0.1:0` do `MAX_BIND_RETRIES` razy.
    /// Port 0 oznacza, że OS przydzieli losowy wolny port.
    async fn bind_with_retry(&self) -> Result<TcpListener> {
        let mut last_error = None;

        for attempt in 1..=MAX_BIND_RETRIES {
            match TcpListener::bind("127.0.0.1:0").await {
                Ok(listener) => return Ok(listener),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_BIND_RETRIES {
                        // Krótka pauza przed ponowną próbą
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }

        Err(RalphError::Mcp(format!(
            "Failed to bind TCP listener after {MAX_BIND_RETRIES} attempts: {}",
            last_error.expect("at least one attempt was made")
        )))
    }

    /// Buduje AppState z komponentów serwera
    fn build_app_state(&self) -> AppState {
        AppState::new(
            Arc::clone(&self.session_registry),
            self.tasks_path.clone(),
            self.question_tx.clone(),
            self.cancellation_token.clone(),
        )
    }

    /// Tworzy axum Router z routingiem MCP (POST/GET/DELETE /mcp)
    fn build_router(state: AppState) -> Router {
        super::router::build_router(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_server() -> McpServer {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token)
    }

    #[test]
    fn test_new_creates_server() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let path = PathBuf::from("/tmp/test_tasks.yml");

        let server = McpServer::new(path.clone(), tx, token);

        assert_eq!(server.tasks_path, path);
    }

    #[test]
    fn test_build_app_state() {
        let server = make_test_server();
        let state = server.build_app_state();

        assert!(!state.cancellation_token.is_cancelled());
        assert_eq!(state.session_registry.session_count(), 0);
    }

    #[test]
    fn test_build_router_creates_router() {
        let server = make_test_server();
        let state = server.build_app_state();
        // Weryfikacja, że router się tworzy bez paniki
        let _router = McpServer::build_router(state);
    }

    #[tokio::test]
    async fn test_start_returns_port_and_handle() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Port powinien być > 0 (losowy port przydzielony przez OS)
        assert!(port > 0);

        // Shutdown serwera
        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_start_binds_to_localhost() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Powinniśmy móc połączyć się z serwerem na localhost
        let addr = format!("127.0.0.1:{port}");
        let result = tokio::net::TcpStream::connect(&addr).await;
        assert!(result.is_ok(), "Should connect to server at {addr}");

        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (_port, handle) = server.start().await.unwrap();

        // Anulowanie powinno zakończyć serwer
        token.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;

        assert!(result.is_ok(), "Server should shut down within 5s");
    }

    #[tokio::test]
    async fn test_multiple_servers_different_ports() {
        let (tx1, _rx1) = mpsc::channel(32);
        let (tx2, _rx2) = mpsc::channel(32);
        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();

        let server1 = McpServer::new(PathBuf::from("/tmp/tasks1.yml"), tx1, token1.clone());
        let server2 = McpServer::new(PathBuf::from("/tmp/tasks2.yml"), tx2, token2.clone());

        let (port1, handle1) = server1.start().await.unwrap();
        let (port2, handle2) = server2.start().await.unwrap();

        // Dwa serwery powinny mieć różne porty
        assert_ne!(port1, port2);

        token1.cancel();
        token2.cancel();
        handle1.await.unwrap();
        handle2.await.unwrap();
    }

    #[tokio::test]
    async fn test_bind_with_retry_succeeds() {
        let server = make_test_server();
        let listener = server.bind_with_retry().await;
        assert!(listener.is_ok());
    }

    #[tokio::test]
    async fn test_cancellation_token_shared_with_state() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let state = server.build_app_state();

        // Token w state powinien być powiązany z oryginalnym
        assert!(!state.cancellation_token.is_cancelled());
        token.cancel();
        assert!(state.cancellation_token.is_cancelled());
    }

    #[test]
    fn test_session_registry_returns_shared_arc() {
        let server = make_test_server();
        let reg1 = server.session_registry();
        let reg2 = server.session_registry();

        // Obie referencje powinny wskazywać na ten sam Arc
        assert!(Arc::ptr_eq(&reg1, &reg2));
        assert_eq!(reg1.session_count(), 0);
    }

    #[test]
    fn test_session_registry_shared_with_app_state() {
        let server = make_test_server();
        let registry = server.session_registry();
        let state = server.build_app_state();

        // Rejestr z session_registry() i z AppState powinien być tym samym obiektem
        assert!(Arc::ptr_eq(&registry, &state.session_registry));

        // Sesja dodana przez zewnętrzny rejestr powinna być widoczna w AppState
        let session_id = registry.create_session(PathBuf::from("/tmp/worker1/tasks.yml"), false);
        assert_eq!(state.session_registry.session_count(), 1);
        assert!(
            state
                .session_registry
                .validate_session(&session_id)
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_session_registry_persists_across_server_start() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        // Rejestracja sesji przed startem serwera
        let registry = server.session_registry();
        let sid = registry.create_session(PathBuf::from("/tmp/w1/tasks.yml"), false);

        let (port, handle) = server.start().await.unwrap();
        assert!(port > 0);

        // Sesja zarejestrowana przed startem powinna nadal istnieć
        assert_eq!(registry.session_count(), 1);
        assert!(registry.validate_session(&sid).is_some());

        token.cancel();
        handle.await.unwrap();
    }

    #[test]
    fn test_register_session_creates_new_session() {
        let server = make_test_server();
        let tasks_path = PathBuf::from("/tmp/worker1/tasks.yml");

        let session_id = server.register_session(tasks_path.clone(), false);

        // UUID v4 format: 8-4-4-4-12 characters
        assert_eq!(session_id.len(), 36);

        // Sesja powinna być widoczna przez session_registry()
        let registry = server.session_registry();
        assert_eq!(registry.session_count(), 1);

        let state = registry.validate_session(&session_id).unwrap();
        assert_eq!(state.tasks_path, tasks_path);
        assert!(!state.initialized);
    }

    #[test]
    fn test_register_session_multiple_workers() {
        let server = make_test_server();

        let id1 = server.register_session(PathBuf::from("/tmp/worker1/tasks.yml"), false);
        let id2 = server.register_session(PathBuf::from("/tmp/worker2/tasks.yml"), false);
        let id3 = server.register_session(PathBuf::from("/tmp/worker3/tasks.yml"), false);

        // Każdy worker powinien mieć unikalny ID
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);

        // Wszystkie sesje powinny być w rejestrze
        let registry = server.session_registry();
        assert_eq!(registry.session_count(), 3);
        assert!(registry.validate_session(&id1).is_some());
        assert!(registry.validate_session(&id2).is_some());
        assert!(registry.validate_session(&id3).is_some());
    }

    #[test]
    fn test_register_session_delegates_to_registry() {
        let server = make_test_server();
        let tasks_path = PathBuf::from("/tmp/worker/tasks.yml");

        // Rejestracja przez register_session
        let session_id = server.register_session(tasks_path.clone(), false);

        // Powinno być równoważne bezpośredniemu wywołaniu registry.create_session
        let registry = server.session_registry();
        let state = registry.validate_session(&session_id).unwrap();

        assert_eq!(state.tasks_path, tasks_path);
        assert!(!state.initialized);
    }

    #[tokio::test]
    async fn test_register_session_before_server_start() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        // Rejestracja sesji przed startem serwera
        let session_id = server.register_session(PathBuf::from("/tmp/worker/tasks.yml"), false);

        let (port, handle) = server.start().await.unwrap();
        assert!(port > 0);

        // Sesja zarejestrowana przed startem powinna nadal istnieć
        let registry = server.session_registry();
        assert_eq!(registry.session_count(), 1);
        assert!(registry.validate_session(&session_id).is_some());

        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_register_session_after_server_start() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();
        assert!(port > 0);

        // Rejestracja sesji po starcie serwera
        let session_id = server.register_session(PathBuf::from("/tmp/worker/tasks.yml"), false);

        let registry = server.session_registry();
        assert_eq!(registry.session_count(), 1);
        assert!(registry.validate_session(&session_id).is_some());

        token.cancel();
        handle.await.unwrap();
    }

    // ── Origin validation tests (integracja HTTP) ────────────────
    // Te testy weryfikują że Origin validation middleware jest prawidłowo
    // zintegrowana z routerem i chroni rzeczywiste endpointy HTTP.
    // Testy walidacji logiki middleware znajdują się w middleware.rs.

    #[tokio::test]
    async fn test_origin_validation_accepts_localhost() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Wyślij JSON-RPC initialize z Origin: http://localhost:PORT
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let response = client
            .post(&url)
            .header("origin", "http://localhost:3000")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }))
            .send()
            .await;

        assert!(response.is_ok(), "Request should be sent successfully");

        let status = response.unwrap().status();
        // Middleware powinien zaakceptować localhost origin
        // Handler zwraca 200 OK dla initialize
        assert_eq!(
            status, 200,
            "Request with localhost origin should reach handler and return 200 OK"
        );

        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_origin_validation_accepts_127_0_0_1() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Wyślij JSON-RPC initialize z Origin: http://127.0.0.1:PORT
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let response = client
            .post(&url)
            .header("origin", "http://127.0.0.1:8080")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }))
            .send()
            .await;

        assert!(response.is_ok(), "Request should be sent successfully");

        let status = response.unwrap().status();
        // Middleware powinien zaakceptować 127.0.0.1 origin
        assert_eq!(
            status, 200,
            "Request with 127.0.0.1 origin should reach handler and return 200 OK"
        );

        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_origin_validation_accepts_missing_origin_header() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Wyślij JSON-RPC initialize BEZ Origin header (CLI clients)
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }))
            .send()
            .await;

        assert!(
            response.is_ok(),
            "Request without origin header should be sent successfully"
        );

        let status = response.unwrap().status();
        // Middleware powinien zaakceptować request bez Origin (CLI clients)
        assert_eq!(
            status, 200,
            "Request without origin header should reach handler and return 200 OK"
        );

        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_origin_validation_rejects_external_origin() {
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let server = McpServer::new(PathBuf::from("/tmp/tasks.yml"), tx, token.clone());

        let (port, handle) = server.start().await.unwrap();

        // Wyślij JSON-RPC initialize z Origin: http://evil.com
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let response = client
            .post(&url)
            .header("origin", "http://evil.com")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }))
            .send()
            .await;

        assert!(response.is_ok(), "Request should be sent");

        let status = response.unwrap().status();
        // Middleware powinien odrzucić external origin z 403 Forbidden
        // Request NIE powinien trafić do handlera
        assert_eq!(
            status, 403,
            "Request with external origin should be blocked by middleware with 403 Forbidden"
        );

        token.cancel();
        handle.await.unwrap();
    }
}
