// Axum router i handlery HTTP dla MCP Streamable HTTP protocol
//
// Trzy endpointy:
// - POST /mcp — główny JSON-RPC handler (initialize, tools/list, tools/call, notifications)
// - GET /mcp  — 405 Method Not Allowed (SSE nie jest wspierane)
// - DELETE /mcp — terminacja sesji

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::ask_user;
use super::middleware::validate_origin;
use super::protocol::{self, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND};
use super::state::AppState;
use super::tools;

/// Nagłówek HTTP z ID sesji MCP (Streamable HTTP spec)
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Buduje axum Router z endpointami MCP
///
/// Router zawiera middleware walidacji Origin — akceptuje tylko
/// requesty z localhost/127.0.0.1 oraz bez Origin header (CLI clients).
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/mcp", post(handle_post).delete(handle_delete))
        .route("/mcp", get(handle_get))
        .with_state(state)
        .layer(middleware::from_fn(validate_origin))
}

// ── POST /mcp ────────────────────────────────────────────────────

/// Główny handler JSON-RPC dla MCP Streamable HTTP
///
/// Tworzy sesję przy `initialize`, waliduje Mcp-Session-Id dla
/// pozostałych metod. Dispatchuje do odpowiedniego handlera
/// na podstawie metody JSON-RPC.
async fn handle_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    match req.method.as_str() {
        // Initialize — tworzy nową sesję, zwraca Mcp-Session-Id w headerze
        "initialize" => handle_initialize(&state, &req),

        // Notifications — nie wymagają odpowiedzi (fire-and-forget)
        "notifications/initialized" => handle_notifications_initialized(&state, &headers),

        // Inne notyfikacje — ignoruj
        method if method.starts_with("notifications/") => StatusCode::ACCEPTED.into_response(),

        // tools/list i tools/call — wymagają sesji
        "tools/list" | "tools/call" => handle_tool_request(state.clone(), &headers, &req).await,

        // Nieznana metoda
        _ => {
            let resp = JsonRpcResponse::error(
                req.id.clone(),
                METHOD_NOT_FOUND,
                format!("Unknown method: {}", req.method),
            );
            Json(resp).into_response()
        }
    }
}

/// Obsługuje `initialize` — tworzy sesję i zwraca capabilities
fn handle_initialize(state: &AppState, req: &JsonRpcRequest) -> Response {
    // Przekaż params do walidacji wersji protokołu
    let resp = protocol::handle_initialize(req.id.clone(), req.params.clone());

    // Jeśli walidacja się nie powiodła (błąd protokołu), zwróć błąd bez tworzenia sesji
    if resp.error.is_some() {
        return (StatusCode::OK, Json(resp)).into_response();
    }

    // Tworzenie sesji z domyślną ścieżką tasks_path z konfiguracji serwera
    // Sesja interaktywna (nie read-only) - normalny użytkownik CLI
    let session_id = state
        .session_registry
        .create_session(state.default_tasks_path.clone(), false);

    // Zwróć response z nagłówkiem Mcp-Session-Id
    (
        StatusCode::OK,
        [(MCP_SESSION_ID_HEADER, session_id)],
        Json(resp),
    )
        .into_response()
}

/// Obsługuje `notifications/initialized` — oznacza sesję jako zainicjalizowaną
///
/// Wymaga valid session ID — zgodnie z MCP spec, notyfikacja initialized
/// jest wysyłana PO initialize, więc sesja musi istnieć.
fn handle_notifications_initialized(state: &AppState, headers: &HeaderMap) -> Response {
    let session_id = match extract_session_id(headers) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Missing {MCP_SESSION_ID_HEADER} header") })),
            )
                .into_response();
        }
    };

    // Waliduj że sesja istnieje
    if state
        .session_registry
        .validate_session(&session_id)
        .is_none()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Session not found: {session_id}") })),
        )
            .into_response();
    }

    // Oznacz sesję jako zainicjalizowaną
    if let Err(e) = state.session_registry.mark_initialized(&session_id) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to mark session initialized: {e}") })),
        )
            .into_response();
    }

    // Notyfikacja nie wymaga odpowiedzi — 202 Accepted
    StatusCode::ACCEPTED.into_response()
}

/// Obsługuje tools/list i tools/call — wymagają aktywnej sesji
async fn handle_tool_request(
    state: AppState,
    headers: &HeaderMap,
    req: &JsonRpcRequest,
) -> Response {
    // Walidacja sesji
    let session_id = match extract_session_id(headers) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(JsonRpcResponse::error(
                    req.id.clone(),
                    INVALID_PARAMS,
                    format!("Missing {MCP_SESSION_ID_HEADER} header"),
                )),
            )
                .into_response();
        }
    };

    let session = match state.session_registry.validate_session(&session_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(JsonRpcResponse::error(
                    req.id.clone(),
                    INVALID_PARAMS,
                    format!("Session not found: {session_id}"),
                )),
            )
                .into_response();
        }
    };

    // Zgodnie z MCP spec: tools/* wymagają zainicjalizowanej sesji
    // (klient musi najpierw wysłać notifications/initialized)
    if !session.initialized {
        return (
            StatusCode::FORBIDDEN,
            Json(JsonRpcResponse::error(
                req.id.clone(),
                INVALID_PARAMS,
                format!(
                    "Session not initialized: {session_id}. Send notifications/initialized first."
                ),
            )),
        )
            .into_response();
    }

    // Dispatch na podstawie metody
    match req.method.as_str() {
        "tools/list" => {
            // Dla sesji read-only zwróć tylko query tools + ask_user
            let tools_list = if session.read_only {
                tools::list_tools_readonly()
            } else {
                tools::list_tools()
            };
            let resp = JsonRpcResponse::success(req.id.clone(), tools_list);
            Json(resp).into_response()
        }
        "tools/call" => {
            handle_tools_call(req, &session.tasks_path, state.clone(), session.read_only).await
        }
        _ => unreachable!("Filtrowane wcześniej w handle_post"),
    }
}

/// Obsługuje tools/call — deleguje do tools::handle_tool_call() lub ask_user SSE
async fn handle_tools_call(
    req: &JsonRpcRequest,
    tasks_path: &std::path::Path,
    state: AppState,
    read_only: bool,
) -> Response {
    // Sprawdź czy to ask_user — wymaga SSE response zamiast JSON
    let tool_name = req
        .params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str());

    if tool_name == Some("ask_user") {
        let tool_args = req
            .params
            .as_ref()
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let rpc_id = req.id.clone().unwrap_or(serde_json::Value::Null);
        return ask_user::handle_ask_user(tool_args, state, rpc_id)
            .await
            .into_response();
    }

    // Blokuj mutation tools w trybie read-only
    if read_only && tool_name.is_some_and(tools::is_mutation_tool) {
        return (
            StatusCode::FORBIDDEN,
            Json(JsonRpcResponse::error(
                req.id.clone(),
                INVALID_PARAMS,
                "Session is read-only: mutation tools are disabled".to_string(),
            )),
        )
            .into_response();
    }

    // Inne toole — synchroniczny JSON response
    let resp = tools::handle_tool_call(req.id.clone(), req.params.as_ref(), tasks_path);
    Json(resp).into_response()
}

// ── GET /mcp ─────────────────────────────────────────────────────

/// GET /mcp — 405 Method Not Allowed
///
/// MCP Streamable HTTP nie wymaga GET (SSE). Zwracamy 405 z Allow header.
async fn handle_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "POST, DELETE")],
        "Method Not Allowed",
    )
        .into_response()
}

// ── DELETE /mcp ──────────────────────────────────────────────────

/// DELETE /mcp — terminacja sesji
///
/// Usuwa sesję z rejestru na podstawie nagłówka Mcp-Session-Id.
/// Zwraca 200 OK jeśli sesja istniała, 404 jeśli nie znaleziono.
async fn handle_delete(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let session_id = match extract_session_id(&headers) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Missing {MCP_SESSION_ID_HEADER} header") })),
            )
                .into_response();
        }
    };

    match state.session_registry.remove_session(&session_id) {
        Some(_) => StatusCode::OK.into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Session not found: {session_id}") })),
        )
            .into_response(),
    }
}

// ── Helpers ──────────────────────────────────────────────────────

/// Wyciąga Mcp-Session-Id z nagłówków HTTP
fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::super::session::SessionRegistry;
    use super::super::state::QuestionEnvelope;

    /// Tworzy testowy AppState
    fn test_state() -> AppState {
        let registry = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel::<QuestionEnvelope>(32);
        let token = CancellationToken::new();
        let tasks_path = std::path::PathBuf::from("/tmp/tasks.yml");
        AppState::new(registry, tasks_path, tx, token)
    }

    /// Tworzy testowy router z AppState
    fn test_router() -> (Router, AppState) {
        let state = test_state();
        let router = build_router(state.clone());
        (router, state)
    }

    /// Helper: wysyła JSON-RPC POST do /mcp
    fn post_json_rpc(body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    /// Helper: wysyła JSON-RPC POST z nagłówkiem sesji
    fn post_json_rpc_with_session(body: Value, session_id: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header(MCP_SESSION_ID_HEADER, session_id)
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    /// Helper: parsuje response body jako JSON
    async fn response_json(response: Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    // ── POST /mcp: initialize ────────────────────────────────────

    #[tokio::test]
    async fn test_post_initialize_returns_200() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            }
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Sprawdź nagłówek Mcp-Session-Id
        let session_id = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .expect("Should have session ID header");
        assert!(!session_id.is_empty());
    }

    #[tokio::test]
    async fn test_post_initialize_response_structure() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            }
        }));

        let response = router.oneshot(req).await.unwrap();
        let body = response_json(response).await;

        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"]["protocolVersion"].is_string());
        assert!(body["result"]["capabilities"]["tools"].is_object());
        assert_eq!(body["result"]["serverInfo"]["name"], "ralph-tasks");
    }

    #[tokio::test]
    async fn test_post_initialize_creates_session() {
        let (router, state) = test_router();

        assert_eq!(state.session_registry.session_count(), 0);

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }));

        let response = router.oneshot(req).await.unwrap();
        let session_id = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Sesja powinna być utworzona
        assert_eq!(state.session_registry.session_count(), 1);
        assert!(
            state
                .session_registry
                .validate_session(&session_id)
                .is_some()
        );
    }

    // ── POST /mcp: notifications/initialized ─────────────────────

    #[tokio::test]
    async fn test_post_notifications_initialized_returns_202() {
        let (router, state) = test_router();

        // Najpierw utwórz sesję
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_post_notifications_initialized_marks_session() {
        let (router, state) = test_router();

        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        // Przed — niezainicjalizowana
        let session = state
            .session_registry
            .validate_session(&session_id)
            .unwrap();
        assert!(!session.initialized);

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            &session_id,
        );

        router.oneshot(req).await.unwrap();

        // Po — zainicjalizowana
        let session = state
            .session_registry
            .validate_session(&session_id)
            .unwrap();
        assert!(session.initialized);
    }

    #[tokio::test]
    async fn test_post_notifications_initialized_without_session_returns_400() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = response_json(response).await;
        assert!(body["error"].as_str().unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_post_notifications_initialized_invalid_session_returns_404() {
        let (router, _state) = test_router();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            "non-existent-session-id",
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = response_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("Session not found")
        );
    }

    // ── POST /mcp: tools/list ────────────────────────────────────

    #[tokio::test]
    async fn test_post_tools_list_returns_tools() {
        let (router, state) = test_router();

        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        // Oznacz sesję jako zainicjalizowaną (wymagane dla tools/list)
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 2);

        let tools = body["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 12);
    }

    #[tokio::test]
    async fn test_post_tools_list_without_session_returns_400() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_post_tools_list_invalid_session_returns_404() {
        let (router, _state) = test_router();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            "non-existent-session-id",
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_post_tools_list_uninitialized_session_returns_403() {
        let (router, state) = test_router();

        // Utwórz sesję ale NIE oznacz jej jako zainicjalizowanej
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = response_json(response).await;
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], INVALID_PARAMS);
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not initialized")
        );
    }

    #[tokio::test]
    async fn test_post_tools_list_read_only_session_returns_limited_tools() {
        let (router, state) = test_router();

        // Utwórz sesję read-only
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), true);

        // Oznacz sesję jako zainicjalizowaną
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 2);

        // Sprawdź że zwrócono tylko read-only tools (5 zamiast 12)
        let tools = body["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);

        // Sprawdź że to są właściwe tools
        let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

        assert!(tool_names.contains(&"tasks_list"));
        assert!(tool_names.contains(&"tasks_get"));
        assert!(tool_names.contains(&"tasks_summary"));
        assert!(tool_names.contains(&"tasks_tree"));
        assert!(tool_names.contains(&"ask_user"));

        // Sprawdź że mutation tools NIE są obecne
        assert!(!tool_names.contains(&"tasks_create"));
        assert!(!tool_names.contains(&"tasks_update"));
        assert!(!tool_names.contains(&"tasks_delete"));
    }

    #[tokio::test]
    async fn test_post_tools_list_writable_session_returns_all_tools() {
        let (router, state) = test_router();

        // Utwórz sesję zapisywalną (read_only = false)
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        // Oznacz sesję jako zainicjalizowaną
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;

        // Sprawdź że zwrócono wszystkie 12 tools
        let tools = body["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 12);
    }

    // ── POST /mcp: tools/call ────────────────────────────────────

    #[tokio::test]
    async fn test_post_tools_call_missing_name_returns_error() {
        let (router, state) = test_router();

        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        // Oznacz sesję jako zainicjalizowaną (wymagane dla tools/call)
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "arguments": {} }
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_post_tools_call_uninitialized_session_returns_403() {
        let (router, state) = test_router();

        // Utwórz sesję ale NIE oznacz jej jako zainicjalizowanej
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "tasks_list",
                    "arguments": {}
                }
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = response_json(response).await;
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], INVALID_PARAMS);
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not initialized")
        );
    }

    // ── POST /mcp: read-only session blocking ────────────────────

    #[tokio::test]
    async fn test_post_tools_call_read_only_blocks_mutating_operations() {
        let (router, state) = test_router();

        // Utwórz sesję read-only
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), true);

        // Oznacz sesję jako zainicjalizowaną
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        // Lista operacji modyfikujących, które powinny być zablokowane
        let mutating_tools = vec![
            "tasks_create",
            "tasks_update",
            "tasks_delete",
            "tasks_move",
            "tasks_batch_status",
            "tasks_set_deps",
            "tasks_set_default_model",
        ];

        for tool_name in mutating_tools {
            let req = post_json_rpc_with_session(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": tool_name,
                        "arguments": {}
                    }
                }),
                &session_id,
            );

            let response = router.clone().oneshot(req).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "Tool '{}' should be blocked in read-only mode",
                tool_name
            );

            let body = response_json(response).await;
            assert!(body["error"].is_object());
            assert_eq!(body["error"]["code"], INVALID_PARAMS);
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("read-only"),
                "Error message should mention read-only for tool '{}'",
                tool_name
            );
        }
    }

    #[tokio::test]
    async fn test_post_tools_call_read_only_allows_query_operations() {
        let (router, state) = test_router();

        // Utwórz sesję read-only
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), true);

        // Oznacz sesję jako zainicjalizowaną
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        // Lista operacji tylko do odczytu, które powinny być dozwolone
        let query_tools = vec!["tasks_list", "tasks_get", "tasks_summary", "tasks_tree"];

        for tool_name in query_tools {
            let req = post_json_rpc_with_session(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": tool_name,
                        "arguments": {}
                    }
                }),
                &session_id,
            );

            let response = router.clone().oneshot(req).await.unwrap();
            // Dla testów zakładamy, że plik /tmp/tasks.yml może nie istnieć,
            // więc sprawdzamy, że nie dostajemy FORBIDDEN
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "Tool '{}' should be allowed in read-only mode",
                tool_name
            );
        }
    }

    #[tokio::test]
    async fn test_post_tools_call_writable_session_allows_all_operations() {
        let (router, state) = test_router();

        // Utwórz sesję zapisywalną (read_only = false)
        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);

        // Oznacz sesję jako zainicjalizowaną
        state
            .session_registry
            .mark_initialized(&session_id)
            .unwrap();

        // Test operacji modyfikującej - nie powinna być zablokowana
        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "tasks_update",
                    "arguments": {
                        "id": "1",
                        "status": "done"
                    }
                }
            }),
            &session_id,
        );

        let response = router.oneshot(req).await.unwrap();
        // Nie powinna być FORBIDDEN (może być inny błąd np. file not found)
        assert_ne!(response.status(), StatusCode::FORBIDDEN);
    }

    // ── POST /mcp: unknown method ────────────────────────────────

    #[tokio::test]
    async fn test_post_unknown_method_returns_error() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "unknown/method"
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], METHOD_NOT_FOUND);
    }

    // ── POST /mcp: generic notification ──────────────────────────

    #[tokio::test]
    async fn test_post_generic_notification_returns_202() {
        let (router, _state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled"
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    // ── GET /mcp ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_returns_405() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("GET")
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        // Sprawdź Allow header
        let allow = response.headers().get("allow").unwrap().to_str().unwrap();
        assert!(allow.contains("POST"));
        assert!(allow.contains("DELETE"));
    }

    // ── DELETE /mcp ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_removes_session() {
        let (router, state) = test_router();

        let session_id = state
            .session_registry
            .create_session(std::path::PathBuf::from("/tmp/tasks.yml"), false);
        assert_eq!(state.session_registry.session_count(), 1);

        let req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(MCP_SESSION_ID_HEADER, &session_id)
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.session_registry.session_count(), 0);
    }

    #[tokio::test]
    async fn test_delete_without_session_id_returns_400() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_invalid_session_returns_404() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(MCP_SESSION_ID_HEADER, "non-existent-session")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Integration: pełen lifecycle ─────────────────────────────

    #[tokio::test]
    async fn test_full_session_lifecycle() {
        let state = test_state();

        // 1. Initialize — tworzy sesję
        let router = build_router(state.clone());
        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }));
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let session_id = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(state.session_registry.session_count(), 1);

        // 2. notifications/initialized
        let router = build_router(state.clone());
        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            &session_id,
        );
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let session = state
            .session_registry
            .validate_session(&session_id)
            .unwrap();
        assert!(session.initialized);

        // 3. tools/list
        let router = build_router(state.clone());
        let req = post_json_rpc_with_session(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }),
            &session_id,
        );
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 12);

        // 4. DELETE — terminacja sesji
        let router = build_router(state.clone());
        let req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(MCP_SESSION_ID_HEADER, &session_id)
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.session_registry.session_count(), 0);
    }

    // ── Origin validation middleware tests ──────────────────────

    #[tokio::test]
    async fn test_middleware_accepts_request_without_origin() {
        let (router, state) = test_router();

        let req = post_json_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }));

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.session_registry.session_count(), 1);
    }

    #[tokio::test]
    async fn test_middleware_accepts_localhost_origin() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("origin", "http://localhost:3000")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_accepts_127_0_0_1_origin() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("origin", "http://127.0.0.1:8080")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_rejects_remote_origin() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("origin", "http://evil.attacker.com")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Origin not allowed"));
    }

    #[tokio::test]
    async fn test_middleware_rejects_private_network_origin() {
        let (router, _state) = test_router();

        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("origin", "http://192.168.1.100")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_middleware_blocks_before_handler() {
        let (router, state) = test_router();

        // Requesty z niedozwolonym Origin nie powinny tworzyć sesji
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("origin", "http://malicious.com")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Sesja nie powinna być utworzona
        assert_eq!(state.session_registry.session_count(), 0);
    }
}
