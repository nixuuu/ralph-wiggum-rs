// MCP HTTP Server — moduły protokołu, routingu, handlerów i sesji
pub mod ask_user;
pub mod handlers;
pub mod middleware;
pub mod protocol;
pub mod router;
pub mod server;
pub mod session;
pub mod state;
mod tools;
pub mod tui;

#[cfg(test)]
mod tests {
    use super::session::SessionRegistry;
    use std::path::PathBuf;

    /// Test referencyjny dla modułu session - zapewnia, że moduł jest kompilowany
    #[test]
    fn test_session_module_integration() {
        let registry = SessionRegistry::new();
        let session_id = registry.create_session(PathBuf::from("/tmp/test.yml"), false);
        assert!(!session_id.is_empty());
        assert_eq!(registry.session_count(), 1);
    }
}
