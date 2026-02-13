// Axum middleware do walidacji Origin header
//
// Chroni serwer MCP przed nieautoryzowanymi requestami cross-origin.
// Akceptuje tylko requesty z localhost/127.0.0.1 oraz requesty bez Origin (CLI clients).

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware walidujący Origin header
///
/// Akceptuje:
/// - Requesty bez nagłówka Origin (CLI clients, non-browser tools)
/// - Requesty z Origin zawierającym '127.0.0.1' lub 'localhost'
///
/// Odrzuca:
/// - Wszystkie inne Origin values z kodem 403 Forbidden
///
/// # Security
///
/// Zabezpiecza endpoint MCP przed cross-origin attacks z nieautoryzowanych źródeł.
/// Tylko lokalne połączenia są dozwolone.
pub async fn validate_origin(req: Request, next: Next) -> Response {
    let origin = req.headers().get("origin");

    match origin {
        // Brak Origin — dozwolone (CLI clients, curl, itp.)
        None => next.run(req).await,

        // Origin obecny — walidacja
        Some(value) => {
            if let Ok(origin_str) = value.to_str() {
                if is_localhost_origin(origin_str) {
                    next.run(req).await
                } else {
                    forbidden_response(origin_str)
                }
            } else {
                // Origin header nieprawidłowy (non-UTF8)
                forbidden_response("<invalid-origin>")
            }
        }
    }
}

/// Sprawdza czy Origin pochodzi z localhost
///
/// Parsuje Origin jako URL i waliduje czy hostname to:
/// - `127.0.0.1` (IPv4 loopback)
/// - `localhost` (case-insensitive)
/// - `::1` (IPv6 loopback)
///
/// Odrzuca złośliwe domeny typu `evil-127.0.0.1.com` lub `localhost.attacker.com`.
///
/// # Examples
///
/// ```
/// assert!(is_localhost_origin("http://127.0.0.1:3000"));
/// assert!(is_localhost_origin("http://localhost:8080"));
/// assert!(is_localhost_origin("http://LOCALHOST")); // case-insensitive
/// assert!(is_localhost_origin("http://[::1]:3000")); // IPv6
/// assert!(!is_localhost_origin("http://localhost.evil.com")); // subdomain
/// assert!(!is_localhost_origin("http://example.com"));
/// ```
fn is_localhost_origin(origin: &str) -> bool {
    // Parsuj Origin jako URL
    let Ok(url) = origin.parse::<url::Url>() else {
        return false;
    };

    // Origin musi być http lub https (bezpieczeństwo HTTP)
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }

    // Wyciągnij hostname (może być None dla względnych URL)
    let Some(host_str) = url.host_str() else {
        return false;
    };

    // Case-insensitive porównanie dla localhost
    if host_str.eq_ignore_ascii_case("localhost") {
        return true;
    }

    // IPv4 loopback
    if host_str == "127.0.0.1" {
        return true;
    }

    // IPv6 loopback (::1)
    if host_str == "::1" || host_str == "[::1]" {
        return true;
    }

    false
}

/// Zwraca 403 Forbidden response z opisem błędu
fn forbidden_response(origin: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        format!("Origin not allowed: {origin}"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use tower::ServiceExt;

    /// Dummy handler dla testów middleware
    async fn test_handler() -> impl IntoResponse {
        "OK"
    }

    /// Tworzy testowy router z middleware
    fn test_router() -> Router {
        Router::new()
            .route("/test", get(test_handler))
            .layer(middleware::from_fn(validate_origin))
    }

    // ── is_localhost_origin tests ─────────────────────────────────

    #[test]
    fn test_is_localhost_origin_accepts_127_0_0_1() {
        assert!(is_localhost_origin("http://127.0.0.1"));
        assert!(is_localhost_origin("http://127.0.0.1:3000"));
        assert!(is_localhost_origin("https://127.0.0.1:8080"));
    }

    #[test]
    fn test_is_localhost_origin_accepts_localhost() {
        assert!(is_localhost_origin("http://localhost"));
        assert!(is_localhost_origin("http://localhost:3000"));
        assert!(is_localhost_origin("https://localhost:8080"));
    }

    #[test]
    fn test_is_localhost_origin_rejects_remote_hosts() {
        assert!(!is_localhost_origin("http://example.com"));
        assert!(!is_localhost_origin("https://evil.com"));
        assert!(!is_localhost_origin("http://192.168.1.100"));
        assert!(!is_localhost_origin("http://10.0.0.1"));
    }

    #[test]
    fn test_is_localhost_origin_rejects_localhost_subdomains() {
        // Odrzucamy subdomeny i supdomeny zawierające "localhost"
        assert!(!is_localhost_origin("http://not-localhost.com"));
        assert!(!is_localhost_origin("http://localhost.evil.com"));
        assert!(!is_localhost_origin("http://evil.localhost.com"));
        assert!(!is_localhost_origin("http://127.0.0.1.evil.com"));
    }

    #[test]
    fn test_is_localhost_origin_accepts_ipv6_loopback() {
        assert!(is_localhost_origin("http://[::1]"));
        assert!(is_localhost_origin("http://[::1]:8080"));
        assert!(is_localhost_origin("https://[::1]:3000"));
    }

    #[test]
    fn test_is_localhost_origin_case_insensitive_localhost() {
        // "localhost" powinien być case-insensitive
        assert!(is_localhost_origin("http://localhost"));
        assert!(is_localhost_origin("http://LOCALHOST"));
        assert!(is_localhost_origin("http://LocalHost:3000"));
        assert!(is_localhost_origin("http://LoCaLhOsT:8080"));
    }

    #[test]
    fn test_is_localhost_origin_rejects_invalid_urls() {
        assert!(!is_localhost_origin("not a url"));
        assert!(!is_localhost_origin("localhost"));
        assert!(!is_localhost_origin("127.0.0.1"));
        assert!(!is_localhost_origin("ftp://localhost")); // valid URL, ale nie http
    }

    #[test]
    fn test_is_localhost_origin_rejects_spoofed_ips() {
        // Próby spoofowania przez subdomenę
        assert!(!is_localhost_origin("http://evil-127.0.0.1.com"));
        assert!(!is_localhost_origin("http://127.0.0.1.attacker.net"));
        assert!(!is_localhost_origin("http://subdomain.127.0.0.1.evil.org"));
    }

    // ── validate_origin middleware tests ──────────────────────────

    #[tokio::test]
    async fn test_validate_origin_accepts_no_origin_header() {
        let router = test_router();

        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_validate_origin_accepts_localhost() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://localhost:3000")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_validate_origin_accepts_127_0_0_1() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://127.0.0.1:8080")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_validate_origin_rejects_remote_origin() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://example.com")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Origin not allowed"));
        assert!(text.contains("example.com"));
    }

    #[tokio::test]
    async fn test_validate_origin_rejects_evil_origin() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "https://evil.attacker.net")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_validate_origin_rejects_private_network() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://192.168.1.100")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_validate_origin_handles_https_localhost() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "https://localhost")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_validate_origin_case_insensitive_localhost() {
        let router = test_router();

        // "LOCALHOST" kapitalizowane - powinna być akceptowane (case-insensitive)
        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://LOCALHOST:3000")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_validate_origin_rejects_spoofed_localhost() {
        let router = test_router();

        // Subdomena localhost - niebezpieczne, powinno być odrzucone
        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://localhost.evil.com")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_validate_origin_accepts_ipv6_loopback() {
        let router = test_router();

        let req = Request::builder()
            .uri("/test")
            .header("origin", "http://[::1]:3000")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
