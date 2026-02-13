use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::shared::error::{RalphError, Result};

/// Stan pojedynczej sesji MCP
///
/// Każda sesja przechowuje ścieżkę do pliku zadań oraz flagę inicjalizacji.
/// Wspiera architekturę wielosesyjną dla orkiestratora workers.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Ścieżka do pliku tasks.yml dla tej sesji
    pub tasks_path: PathBuf,
    /// Czy sesja została zainicjalizowana (otrzymano `notifications/initialized`)
    pub initialized: bool,
    /// Czy sesja jest tylko do odczytu (blokuje modyfikacje zadań)
    pub read_only: bool,
}

/// Rejestr sesji MCP - mapuje UUID sesji na stan
///
/// Thread-safe za pomocą Arc<Mutex<>> - wspiera równoległy dostęp
/// z wielu workerów orkiestratora.
#[derive(Debug, Clone)]
pub struct SessionRegistry {
    /// Mapa: session_id (UUID string) -> SessionState
    sessions: Arc<Mutex<HashMap<String, SessionState>>>,
}

impl SessionRegistry {
    /// Tworzy nowy pusty rejestr sesji
    ///
    /// # Example
    ///
    /// ```no_run
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// ```
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Tworzy nową sesję i zwraca jej UUID
    ///
    /// Generuje unikalny identyfikator UUID v4, tworzy nowy SessionState
    /// i dodaje go do rejestru. Sesja jest początkowo nie zainicjalizowana.
    ///
    /// # Arguments
    ///
    /// * `tasks_path` - Ścieżka do pliku tasks.yml dla tej sesji
    /// * `read_only` - Czy sesja jest tylko do odczytu (true dla workerów orkiestratora)
    ///
    /// # Returns
    ///
    /// UUID sesji jako String (format: "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    /// println!("Created session: {}", session_id);
    /// ```
    pub fn create_session(&self, tasks_path: PathBuf, read_only: bool) -> String {
        let session_id = Uuid::new_v4().to_string();
        let state = SessionState {
            tasks_path,
            initialized: false,
            read_only,
        };

        let mut sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.insert(session_id.clone(), state);

        session_id
    }

    /// Waliduje sesję i zwraca jej stan
    ///
    /// # Arguments
    ///
    /// * `id` - UUID sesji jako &str
    ///
    /// # Returns
    ///
    /// `Some(SessionState)` jeśli sesja istnieje, `None` w przeciwnym razie
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    ///
    /// if let Some(state) = registry.validate_session(&session_id) {
    ///     println!("Session is valid: {:?}", state.tasks_path);
    /// }
    /// ```
    pub fn validate_session(&self, id: &str) -> Option<SessionState> {
        let sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.get(id).cloned()
    }

    /// Oznacza sesję jako zainicjalizowaną
    ///
    /// Wywołane po otrzymaniu notyfikacji `notifications/initialized` od klienta.
    ///
    /// # Arguments
    ///
    /// * `id` - UUID sesji jako &str
    ///
    /// # Returns
    ///
    /// `Ok(())` jeśli sesja istnieje, `Err(RalphError)` jeśli nie znaleziono
    ///
    /// # Errors
    ///
    /// Zwraca `RalphError::Mcp` jeśli sesja o podanym ID nie istnieje
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    ///
    /// registry.mark_initialized(&session_id).expect("Session should exist");
    /// ```
    pub fn mark_initialized(&self, id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().expect("Mutex poisoned");
        let state = sessions
            .get_mut(id)
            .ok_or_else(|| RalphError::Mcp(format!("Session not found: {id}")))?;

        state.initialized = true;
        Ok(())
    }

    /// Pobiera ścieżkę do pliku zadań dla sesji
    ///
    /// # Arguments
    ///
    /// * `id` - UUID sesji jako &str
    ///
    /// # Returns
    ///
    /// `Some(PathBuf)` jeśli sesja istnieje, `None` w przeciwnym razie
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    ///
    /// if let Some(path) = registry.get_tasks_path(&session_id) {
    ///     println!("Tasks file: {:?}", path);
    /// }
    /// ```
    #[allow(dead_code)] // Public API: getter for session tasks_path, used in orchestrator diagnostics
    pub fn get_tasks_path(&self, id: &str) -> Option<PathBuf> {
        let sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.get(id).map(|state| state.tasks_path.clone())
    }

    /// Usuwa sesję z rejestru
    ///
    /// Przydatne przy czyszczeniu po zakończeniu pracy workera.
    ///
    /// # Arguments
    ///
    /// * `id` - UUID sesji jako &str
    ///
    /// # Returns
    ///
    /// `Some(SessionState)` jeśli sesja została usunięta, `None` jeśli nie istniała
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    ///
    /// if let Some(state) = registry.remove_session(&session_id) {
    ///     println!("Removed session: {:?}", state.tasks_path);
    /// }
    /// ```
    pub fn remove_session(&self, id: &str) -> Option<SessionState> {
        let mut sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.remove(id)
    }

    /// Zwraca liczbę aktywnych sesji
    ///
    /// Przydatne do monitorowania i debugowania.
    ///
    /// # Returns
    ///
    /// Liczba sesji w rejestrze
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), false);
    ///
    /// assert_eq!(registry.session_count(), 1);
    /// ```
    #[allow(dead_code)] // Public API: getter for session count, used for monitoring and tests
    pub fn session_count(&self) -> usize {
        let sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.len()
    }

    /// Sprawdza czy sesja jest w trybie tylko do odczytu
    ///
    /// # Arguments
    ///
    /// * `id` - UUID sesji jako &str
    ///
    /// # Returns
    ///
    /// `Some(bool)` jeśli sesja istnieje, `None` w przeciwnym razie
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use ralph_wiggum::commands::mcp::session::SessionRegistry;
    ///
    /// let registry = SessionRegistry::new();
    /// let session_id = registry.create_session(PathBuf::from(".ralph/tasks.yml"), true);
    ///
    /// assert_eq!(registry.is_read_only(&session_id), Some(true));
    /// ```
    #[allow(dead_code)] // Public API: getter for read-only status, used for MCP tool authorization
    pub fn is_read_only(&self, id: &str) -> Option<bool> {
        let sessions = self.sessions.lock().expect("Mutex poisoned");
        sessions.get(id).map(|state| state.read_only)
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_session_registry_new() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.session_count(), 0);
    }

    #[test]
    fn test_create_session() {
        let registry = SessionRegistry::new();
        let tasks_path = PathBuf::from("/tmp/tasks.yml");

        let session_id = registry.create_session(tasks_path.clone(), false);

        // UUID v4 format: 8-4-4-4-12 characters
        assert_eq!(session_id.len(), 36);
        assert_eq!(registry.session_count(), 1);

        // Sprawdź czy sesja jest w rejestrze
        let state = registry.validate_session(&session_id).unwrap();
        assert_eq!(state.tasks_path, tasks_path);
        assert!(!state.initialized);
    }

    #[test]
    fn test_validate_session_exists() {
        let registry = SessionRegistry::new();
        let tasks_path = PathBuf::from("/tmp/tasks.yml");
        let session_id = registry.create_session(tasks_path.clone(), false);

        let state = registry.validate_session(&session_id);
        assert!(state.is_some());

        let state = state.unwrap();
        assert_eq!(state.tasks_path, tasks_path);
        assert!(!state.initialized);
    }

    #[test]
    fn test_validate_session_not_exists() {
        let registry = SessionRegistry::new();
        let state = registry.validate_session("non-existent-id");
        assert!(state.is_none());
    }

    #[test]
    fn test_mark_initialized_success() {
        let registry = SessionRegistry::new();
        let tasks_path = PathBuf::from("/tmp/tasks.yml");
        let session_id = registry.create_session(tasks_path, false);

        // Przed inicjalizacją
        let state = registry.validate_session(&session_id).unwrap();
        assert!(!state.initialized);

        // Inicjalizacja
        let result = registry.mark_initialized(&session_id);
        assert!(result.is_ok());

        // Po inicjalizacji
        let state = registry.validate_session(&session_id).unwrap();
        assert!(state.initialized);
    }

    #[test]
    fn test_mark_initialized_not_found() {
        let registry = SessionRegistry::new();
        let result = registry.mark_initialized("non-existent-id");

        assert!(result.is_err());
        match result {
            Err(RalphError::Mcp(msg)) => assert!(msg.contains("Session not found")),
            _ => panic!("Expected RalphError::Mcp"),
        }
    }

    #[test]
    fn test_get_tasks_path_exists() {
        let registry = SessionRegistry::new();
        let tasks_path = PathBuf::from("/tmp/tasks.yml");
        let session_id = registry.create_session(tasks_path.clone(), false);

        let retrieved_path = registry.get_tasks_path(&session_id);
        assert_eq!(retrieved_path, Some(tasks_path));
    }

    #[test]
    fn test_get_tasks_path_not_exists() {
        let registry = SessionRegistry::new();
        let path = registry.get_tasks_path("non-existent-id");
        assert!(path.is_none());
    }

    #[test]
    fn test_remove_session_exists() {
        let registry = SessionRegistry::new();
        let tasks_path = PathBuf::from("/tmp/tasks.yml");
        let session_id = registry.create_session(tasks_path.clone(), false);

        assert_eq!(registry.session_count(), 1);

        let removed_state = registry.remove_session(&session_id);
        assert!(removed_state.is_some());
        assert_eq!(removed_state.unwrap().tasks_path, tasks_path);
        assert_eq!(registry.session_count(), 0);

        // Sesja powinna być usunięta
        assert!(registry.validate_session(&session_id).is_none());
    }

    #[test]
    fn test_remove_session_not_exists() {
        let registry = SessionRegistry::new();
        let removed_state = registry.remove_session("non-existent-id");
        assert!(removed_state.is_none());
    }

    #[test]
    fn test_session_count() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.session_count(), 0);

        let id1 = registry.create_session(PathBuf::from("/tmp/tasks1.yml"), false);
        assert_eq!(registry.session_count(), 1);

        let id2 = registry.create_session(PathBuf::from("/tmp/tasks2.yml"), false);
        assert_eq!(registry.session_count(), 2);

        registry.remove_session(&id1);
        assert_eq!(registry.session_count(), 1);

        registry.remove_session(&id2);
        assert_eq!(registry.session_count(), 0);
    }

    #[test]
    fn test_multiple_sessions_different_paths() {
        let registry = SessionRegistry::new();

        let path1 = PathBuf::from("/tmp/worker1/tasks.yml");
        let path2 = PathBuf::from("/tmp/worker2/tasks.yml");
        let path3 = PathBuf::from("/tmp/worker3/tasks.yml");

        let id1 = registry.create_session(path1.clone(), false);
        let id2 = registry.create_session(path2.clone(), false);
        let id3 = registry.create_session(path3.clone(), false);

        assert_eq!(registry.session_count(), 3);

        // Sprawdź czy każda sesja ma właściwą ścieżkę
        assert_eq!(registry.get_tasks_path(&id1), Some(path1));
        assert_eq!(registry.get_tasks_path(&id2), Some(path2));
        assert_eq!(registry.get_tasks_path(&id3), Some(path3));
    }

    #[test]
    fn test_session_state_clone() {
        let state = SessionState {
            tasks_path: PathBuf::from("/tmp/tasks.yml"),
            initialized: true,
            read_only: false,
        };

        let cloned = state.clone();
        assert_eq!(cloned.tasks_path, state.tasks_path);
        assert_eq!(cloned.initialized, state.initialized);
        assert_eq!(cloned.read_only, state.read_only);
    }

    #[test]
    fn test_session_registry_clone() {
        let registry1 = SessionRegistry::new();
        let session_id = registry1.create_session(PathBuf::from("/tmp/tasks.yml"), false);

        // Clone powinien wskazywać na ten sam Arc<Mutex<>>
        let registry2 = registry1.clone();
        assert_eq!(registry2.session_count(), 1);

        // Dodanie sesji w registry2 powinno być widoczne w registry1
        registry2.create_session(PathBuf::from("/tmp/tasks2.yml"), false);
        assert_eq!(registry1.session_count(), 2);
        assert_eq!(registry2.session_count(), 2);

        // Oba powinny widzieć tę samą sesję
        assert!(registry1.validate_session(&session_id).is_some());
        assert!(registry2.validate_session(&session_id).is_some());
    }

    #[test]
    fn test_session_registry_default() {
        let registry: SessionRegistry = Default::default();
        assert_eq!(registry.session_count(), 0);
    }

    #[test]
    fn test_unique_session_ids() {
        let registry = SessionRegistry::new();
        let id1 = registry.create_session(PathBuf::from("/tmp/tasks.yml"), false);
        let id2 = registry.create_session(PathBuf::from("/tmp/tasks.yml"), false);
        let id3 = registry.create_session(PathBuf::from("/tmp/tasks.yml"), false);

        // Każda sesja powinna mieć unikalny UUID
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_mark_initialized_idempotent() {
        let registry = SessionRegistry::new();
        let session_id = registry.create_session(PathBuf::from("/tmp/tasks.yml"), false);

        // Pierwsze oznaczenie jako zainicjalizowana
        assert!(registry.mark_initialized(&session_id).is_ok());
        let state = registry.validate_session(&session_id).unwrap();
        assert!(state.initialized);

        // Ponowne oznaczenie jako zainicjalizowana - powinno zadziałać
        assert!(registry.mark_initialized(&session_id).is_ok());
        let state = registry.validate_session(&session_id).unwrap();
        assert!(state.initialized);
    }

    #[test]
    fn test_read_only_flag_false() {
        let registry = SessionRegistry::new();
        let session_id = registry.create_session(PathBuf::from("/tmp/tasks.yml"), false);

        // Sprawdź czy sesja nie jest read-only
        assert_eq!(registry.is_read_only(&session_id), Some(false));

        let state = registry.validate_session(&session_id).unwrap();
        assert!(!state.read_only);
    }

    #[test]
    fn test_read_only_flag_true() {
        let registry = SessionRegistry::new();
        let session_id = registry.create_session(PathBuf::from("/tmp/tasks.yml"), true);

        // Sprawdź czy sesja jest read-only
        assert_eq!(registry.is_read_only(&session_id), Some(true));

        let state = registry.validate_session(&session_id).unwrap();
        assert!(state.read_only);
    }

    #[test]
    fn test_is_read_only_not_exists() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.is_read_only("non-existent-id"), None);
    }

    #[test]
    fn test_multiple_sessions_different_read_only() {
        let registry = SessionRegistry::new();

        let id1 = registry.create_session(PathBuf::from("/tmp/tasks1.yml"), false);
        let id2 = registry.create_session(PathBuf::from("/tmp/tasks2.yml"), true);
        let id3 = registry.create_session(PathBuf::from("/tmp/tasks3.yml"), false);

        assert_eq!(registry.is_read_only(&id1), Some(false));
        assert_eq!(registry.is_read_only(&id2), Some(true));
        assert_eq!(registry.is_read_only(&id3), Some(false));
    }
}
