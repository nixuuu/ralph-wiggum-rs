use ratatui::layout::{Position, Rect};

/// Identyfikator klikalnego UI elementu.
///
/// Każdy wariant reprezentuje inny typ interaktywnego elementu w UI,
/// używany do mapowania pozycji myszy na akcje aplikacji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitId {
    /// Panel pracownika w widoku orchestrate (u32 = worker id)
    WorkerPanel(u32),
    /// Pasek boczny (sidebar) całą powierzchnią
    Sidebar,
    /// Konkretny task w sidebar (index = pozycja w liście)
    SidebarTask { index: usize },
    /// Obszar outputu (OutputView)
    OutputView,
    /// Opcja w pytaniu AskUser (index = numer opcji)
    AskUserOption { index: usize },
    /// Przycisk w potwierdzeniu AskUser (bool = true dla "yes", false dla "no")
    AskUserConfirm(bool),
    /// Pole tekstowe input
    TextInput,
    /// Dolny pasek statusu (StatusBar)
    StatusBar,
}

/// Region klikowalny — prostokąt + identyfikator UI elementu.
///
/// Używany do mapowania pozycji myszy na konkretne akcje w UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    /// Prostokąt na ekranie (pozycja i wymiary)
    pub rect: Rect,
    /// Identyfikator UI elementu tego regionu
    pub id: HitId,
}

impl HitRegion {
    /// Tworzy nowy HitRegion ze wskazanym prostokątem i identyfikatorem.
    pub fn new(rect: Rect, id: HitId) -> Self {
        HitRegion { rect, id }
    }

    /// Sprawdza czy punkt (col, row) znajduje się w tym regionie.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        self.rect.contains(Position { x: col, y: row })
    }
}

/// Mapa hitów — centralne repozytorium klikowalnych regionów UI.
///
/// Przechowuje listę wszystkich aktywnych regionów interaktywnych.
/// Mapa jest czyszczona i przebudowywana od nowa w każdym draw() callzie.
///
/// # Przykład
///
/// ```ignore
/// let mut hit_map = HitMap::new();
/// hit_map.register(header_rect, HitId::Sidebar);
/// hit_map.register(output_rect, HitId::OutputView);
///
/// // Kliknięcie na (10, 5)
/// if let Some(id) = hit_map.hit_test(10, 5) {
///     // obsługa kliknięcia
/// }
/// ```
#[derive(Debug, Clone)]
pub struct HitMap {
    /// Lista wszystkich klikowalnych regionów
    regions: Vec<HitRegion>,
}

impl HitMap {
    /// Tworzy nową, pustą mapę hitów.
    pub fn new() -> Self {
        HitMap {
            regions: Vec::new(),
        }
    }

    /// Czyści mapę hitów (usuwa wszystkie regiony).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Dodaje nowy region do mapy hitów (z-order: ostatni dodany = górną warstwę).
    ///
    /// Region jest identyfikowany przez HitId i zajmuje prostokąt na ekranie.
    /// Nowo dodane regiony mają wyższy priorytet w hit_test.
    pub fn register(&mut self, rect: Rect, id: HitId) {
        self.regions.push(HitRegion::new(rect, id));
    }

    /// Zwraca HitId dla punktu (col, row), jeśli punkt jest w którymś z regionów.
    ///
    /// Przeszukuje regiony w odwrotnej kolejności (ostatnie dodane pierwszym)
    /// aby obsługiwać z-order (górne regiony mają priorytet).
    /// Zwraca None jeśli punkt nie pasuje do żadnego regionu.
    pub fn hit_test(&self, col: u16, row: u16) -> Option<HitId> {
        self.regions
            .iter()
            .rev()
            .find(|region| region.contains(col, row))
            .map(|region| region.id)
    }

    /// Zwraca referencję do wszystkich regionów w mapie.
    pub fn regions(&self) -> &[HitRegion] {
        &self.regions
    }

    /// Zwraca liczbę regionów w mapie.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Sprawdza czy mapa hitów jest pusta.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

impl Default for HitMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hit_map_returns_none() {
        // Pusta mapa hitów nie powinna zwracać żadnego hitu
        let hit_map = HitMap::new();
        assert!(hit_map.is_empty());
        assert_eq!(hit_map.hit_test(0, 0), None);
        assert_eq!(hit_map.hit_test(100, 100), None);
    }

    #[test]
    fn test_hit_region_contains() {
        let rect = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 10,
        };
        let region = HitRegion::new(rect, HitId::OutputView);

        // Punkt wewnątrz regionu
        assert!(region.contains(15, 8));
        assert!(region.contains(10, 5));
        assert!(region.contains(29, 14));

        // Punkt poza regionem
        assert!(!region.contains(9, 5));
        assert!(!region.contains(30, 5));
        assert!(!region.contains(15, 4));
        assert!(!region.contains(15, 15));
    }

    #[test]
    fn test_hit_region_zero_size() {
        // Zero-size rect nigdy nie zawiera żadnego punktu
        let region = HitRegion::new(
            Rect {
                x: 5,
                y: 5,
                width: 0,
                height: 0,
            },
            HitId::TextInput,
        );
        assert!(!region.contains(5, 5));
        assert!(!region.contains(4, 4));
    }

    #[test]
    fn test_hit_map_add_and_query() {
        let mut hit_map = HitMap::new();

        let rect1 = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let rect2 = Rect {
            x: 20,
            y: 0,
            width: 10,
            height: 10,
        };

        hit_map.register(rect1, HitId::OutputView);
        hit_map.register(rect2, HitId::Sidebar);

        assert_eq!(hit_map.hit_test(5, 5), Some(HitId::OutputView));
        assert_eq!(hit_map.hit_test(25, 5), Some(HitId::Sidebar));
        assert_eq!(hit_map.hit_test(15, 5), None);
    }

    #[test]
    fn test_hit_map_layering() {
        // Drugie dodane regiony mają priorytet (są wyżej)
        let mut hit_map = HitMap::new();

        let rect = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        };

        hit_map.register(rect, HitId::OutputView);
        hit_map.register(rect, HitId::Sidebar); // Nakłada się na OutputView

        // Punkt w zakresu obu — zwraca ostatnio zarejestrowany (Sidebar)
        assert_eq!(hit_map.hit_test(10, 10), Some(HitId::Sidebar));
    }

    #[test]
    fn test_hit_map_clear() {
        let mut hit_map = HitMap::new();
        let rect = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        hit_map.register(rect, HitId::OutputView);

        assert!(!hit_map.is_empty());
        assert_eq!(hit_map.len(), 1);

        hit_map.clear();
        assert!(hit_map.is_empty());
        assert_eq!(hit_map.len(), 0);
    }

    #[test]
    fn test_hit_id_variants() {
        // Upewnij się że wszystkie warianty HitId mogą być tworzone
        let _w = HitId::WorkerPanel(1);
        let _s = HitId::Sidebar;
        let _st = HitId::SidebarTask { index: 0 };
        let _o = HitId::OutputView;
        let _ao = HitId::AskUserOption { index: 0 };
        let _ac = HitId::AskUserConfirm(true);
        let _t = HitId::TextInput;
        let _sb = HitId::StatusBar;
    }

    #[test]
    fn test_hit_map_partial_overlap() {
        // Częściowe nakładanie się regionów — weryfikacja z-order w różnych punktach
        let mut hit_map = HitMap::new();

        // rect1: (0,0) → (19,19)
        let rect1 = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        };
        // rect2: (10,10) → (29,29) — częściowo nakłada się z rect1
        let rect2 = Rect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };

        hit_map.register(rect1, HitId::OutputView);
        hit_map.register(rect2, HitId::Sidebar);

        // Tylko rect1 — OutputView
        assert_eq!(hit_map.hit_test(5, 5), Some(HitId::OutputView));
        // Overlap — Sidebar (ostatnio zarejestrowany)
        assert_eq!(hit_map.hit_test(15, 15), Some(HitId::Sidebar));
        // Tylko rect2 — Sidebar
        assert_eq!(hit_map.hit_test(25, 25), Some(HitId::Sidebar));
        // Poza obu regionów
        assert_eq!(hit_map.hit_test(35, 35), None);
    }

    #[test]
    fn test_hit_map_large_u16_coordinates() {
        // Upewnij się że HitMap poprawnie obsługuje duże wartości u16 (bez overflow)
        let mut hit_map = HitMap::new();

        // rect bliski granicy u16: x=65530, width=5 → zakres [65530, 65534]
        let rect = Rect {
            x: 65530,
            y: 100,
            width: 5,
            height: 5,
        };
        hit_map.register(rect, HitId::StatusBar);

        // Punkt na prawej granicy prostokąta — powinien trafić
        assert_eq!(hit_map.hit_test(65534, 104), Some(HitId::StatusBar));
        // Punkt wewnątrz
        assert_eq!(hit_map.hit_test(65532, 102), Some(HitId::StatusBar));
        // Punkt tuż przed lewą granicą — nie trafia
        assert_eq!(hit_map.hit_test(65529, 102), None);
    }

    #[test]
    fn test_hit_map_clear_then_reregister() {
        // Sprawdzenie lifecycle'u mapy: clear() → reregister() → hit_test()
        let mut hit_map = HitMap::new();
        let rect = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        // Dodaj region
        hit_map.register(rect, HitId::Sidebar);
        assert_eq!(hit_map.len(), 1);
        assert_eq!(hit_map.hit_test(5, 5), Some(HitId::Sidebar));

        // Wyczyść mapę
        hit_map.clear();
        assert!(hit_map.is_empty());
        assert_eq!(hit_map.hit_test(5, 5), None);

        // Dodaj nowy region
        hit_map.register(rect, HitId::OutputView);
        assert_eq!(hit_map.len(), 1);
        assert_eq!(hit_map.hit_test(5, 5), Some(HitId::OutputView));
    }
}
