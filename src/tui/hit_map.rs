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

    /// Sprawdza czy punkt (x, y) znajduje się w tym regionie.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.rect.contains(Position { x, y })
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
/// hit_map.add_region(header_rect, HitId::Sidebar);
/// hit_map.add_region(output_rect, HitId::OutputView);
///
/// // Kliknięcie na (10, 5)
/// if let Some(id) = hit_map.hit_at(10, 5) {
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

    /// Dodaje nowy region do mapy hitów.
    pub fn add_region(&mut self, rect: Rect, id: HitId) {
        self.regions.push(HitRegion::new(rect, id));
    }

    /// Zwraca HitId dla punktu (x, y), jeśli punkt jest w którymś z regionów.
    ///
    /// Przeszukuje regiony w odwrotnej kolejności (ostatnie dodane pierwszym)
    /// aby obsługiwać layering (górne regiony mają priorytet).
    pub fn hit_at(&self, x: u16, y: u16) -> Option<HitId> {
        self.regions
            .iter()
            .rev()
            .find(|region| region.contains(x, y))
            .map(|region| region.id)
    }

    /// Zwraca referencję do wszystkich regionów w mapie.
    pub fn regions(&self) -> &[HitRegion] {
        &self.regions
    }

    /// Czyści mapę hitów (usuwa wszystkie regiony).
    pub fn clear(&mut self) {
        self.regions.clear();
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

        hit_map.add_region(rect1, HitId::OutputView);
        hit_map.add_region(rect2, HitId::Sidebar);

        assert_eq!(hit_map.hit_at(5, 5), Some(HitId::OutputView));
        assert_eq!(hit_map.hit_at(25, 5), Some(HitId::Sidebar));
        assert_eq!(hit_map.hit_at(15, 5), None);
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

        hit_map.add_region(rect, HitId::OutputView);
        hit_map.add_region(rect, HitId::Sidebar); // Nakłada się na OutputView

        // Punkt w zakresu obu — zwraca ostatnio dodany (Sidebar)
        assert_eq!(hit_map.hit_at(10, 10), Some(HitId::Sidebar));
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
        hit_map.add_region(rect, HitId::OutputView);

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
}
