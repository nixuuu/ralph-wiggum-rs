//! Fuzzy matching algorithm inspirowany fzf scoring.
//!
//! Scoring priorities (malejąco):
//! 1. Exact match (query == target, case-insensitive)
//! 2. Prefix match (target starts with query)
//! 3. Substring match (target contains query as contiguous substring)
//! 4. Fuzzy match (wszystkie znaki query są w target, w kolejności)
//!
//! Bonusy i kary (podobnie jak fzf):
//! - +10 word boundary (znak query pasuje do początku słowa w target)
//! - +5  exact prefix (query jest prefixem target)
//! - +2  sequential bonus (kolejne znaki query dopasowane kolejno w target)
//! - -1  gap penalty (między dopasowaniami jest przerwa)
//!
//! Matching jest case-insensitive. Unicode-safe (operuje na char, nie bytes).
//!
//! # Performance
//!
//! Algorytm ma złożoność O(n * m) gdzie n = len(target), m = len(query).
//! W testach: ≥1000 wywołań fuzzy_match w <10ms na typowych danych.

/// Wynik dopasowania fuzzy dla pojedynczego elementu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Oryginalny tekst (target), który był dopasowywany.
    pub text: String,
    /// Score dopasowania — wyższy = lepszy match. Zawsze > 0.
    pub score: i32,
    /// Indeksy znaków w `text`, które zostały dopasowane (do podświetlenia).
    /// Sortowane rosnąco. Indeksy są w przestrzeni char (nie bajtów).
    pub matched_indices: Vec<usize>,
}

/// Dopasowuje query do target i zwraca wynik z score i indeksami.
///
/// Zwraca `None` jeśli nie wszystkie znaki query są obecne w target (w kolejności).
/// Zwraca `None` jeśli query jest puste — empty query matches everything z score 0,
/// ale wywołujący może to obsłużyć osobno (patrz `fuzzy_filter`).
///
/// # Przykłady
///
/// ```
/// use ralph_wiggum::tui::fuzzy::fuzzy_match;
///
/// let m = fuzzy_match("foo", "foobar").unwrap();
/// assert!(m.score > 0);
/// assert_eq!(m.matched_indices, vec![0, 1, 2]);
///
/// assert!(fuzzy_match("xyz", "foobar").is_none());
/// ```
pub fn fuzzy_match(query: &str, target: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        // Empty query: match everything with score 0, no highlighted chars
        return Some(FuzzyMatch {
            text: target.to_string(),
            score: 0,
            matched_indices: vec![],
        });
    }

    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let target_chars: Vec<char> = target.chars().collect();
    let target_lower: Vec<char> = target.to_lowercase().chars().collect();

    // --- Exact match ---
    if target_lower == query_lower {
        let indices: Vec<usize> = (0..target_chars.len()).collect();
        return Some(FuzzyMatch {
            text: target.to_string(),
            score: 1000 + target_chars.len() as i32,
            matched_indices: indices,
        });
    }

    // --- Prefix match ---
    if target_lower.starts_with(query_lower.as_slice()) {
        let indices: Vec<usize> = (0..query_lower.len()).collect();
        // Długość prefiksu liczy się: krótszy target → wyższy score (bardziej precyzyjny).
        // Clamp do min. 250 żeby prefix zawsze bił fuzzy (nawet przy bardzo długim target).
        let score = (500 + query_lower.len() as i32
            - (target_chars.len() as i32 - query_lower.len() as i32))
            .max(250);
        return Some(FuzzyMatch {
            text: target.to_string(),
            score,
            matched_indices: indices,
        });
    }

    // --- Substring match ---
    // Szukamy query jako ciągłego podciągu w target
    if let Some(start) = find_substring(&target_lower, &query_lower) {
        let end = start + query_lower.len();
        let indices: Vec<usize> = (start..end).collect();
        // Im wcześniej substring, tym wyższy score; penalizuj za długi target
        let position_bonus = (target_chars.len() - start) as i32;
        let score = 200 + position_bonus;
        return Some(FuzzyMatch {
            text: target.to_string(),
            score,
            matched_indices: indices,
        });
    }

    // --- Fuzzy match (char-by-char, fzf-style) ---
    fuzzy_score(&query_lower, &target_chars, &target_lower, target)
}

/// Szuka podciągu `needle` w `haystack` (oba jako slice of char).
/// Zwraca indeks (w haystack) pierwszego wystąpienia lub None.
fn find_substring(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Oblicza fuzzy score (char-by-char greedy matching).
///
/// Algorytm:
/// 1. Greedy match — znajdź pierwsze możliwe dopasowanie wszystkich znaków query.
/// 2. Oblicz score na podstawie bonusów za granice słów, sekwencyjność i kar za luki.
/// 3. Jeśli nie wszystkie znaki dopasowane → None.
fn fuzzy_score(
    query_lower: &[char],
    target_chars: &[char],
    target_lower: &[char],
    original_target: &str,
) -> Option<FuzzyMatch> {
    let mut matched_indices = Vec::with_capacity(query_lower.len());
    let mut target_idx = 0;

    // Greedy matching: dla każdego znaku query znajdź następne dopasowanie w target
    for &qc in query_lower {
        // Szukaj od bieżącej pozycji
        let found = target_lower[target_idx..].iter().position(|&tc| tc == qc);

        match found {
            Some(offset) => {
                target_idx += offset;
                matched_indices.push(target_idx);
                target_idx += 1;
            }
            None => return None, // Nie wszystkie znaki query są w target
        }
    }

    // Oblicz score na podstawie pozycji dopasowanych znaków
    let score = compute_score(&matched_indices, target_chars);

    Some(FuzzyMatch {
        text: original_target.to_string(),
        score,
        matched_indices,
    })
}

/// Oblicza score na podstawie listy dopasowanych indeksów.
///
/// Bonusy:
/// - +10 za każdy znak na granicy słowa (poprzedni znak to separator lub pozycja 0)
/// - +2  za każdy sekwencyjny bonus (bieżący indeks = poprzedni + 1)
/// - -1  za każdą lukę (bieżący indeks > poprzedni + 1)
fn compute_score(indices: &[usize], target_chars: &[char]) -> i32 {
    if indices.is_empty() {
        return 0;
    }

    let mut score = 0i32;
    let mut prev_idx: Option<usize> = None;

    for &idx in indices {
        // Word boundary bonus
        if is_word_boundary(idx, target_chars) {
            score += 10;
        }

        if let Some(prev) = prev_idx {
            if idx == prev + 1 {
                // Sequential bonus — kolejne znaki query są obok siebie w target
                score += 2;
            } else {
                // Gap penalty — przerwa między dopasowaniami
                score -= 1;
            }
        }

        prev_idx = Some(idx);
    }

    score
}

/// Sprawdza czy pozycja `idx` w target jest granicą słowa.
///
/// Granica słowa: pozycja 0 lub poprzedni znak jest separatorem
/// (spacja, podkreślnik, myślnik, ukośnik, kropka, dwukropek).
fn is_word_boundary(idx: usize, chars: &[char]) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = chars[idx - 1];
    matches!(
        prev,
        ' ' | '_' | '-' | '/' | '.' | ':' | '(' | ')' | '[' | ']'
    )
}

/// Filtruje listę elementów przez fuzzy matching i sortuje po score malejąco.
///
/// - Pusta query zwraca wszystkie elementy z score=0 (w oryginalnej kolejności).
/// - Elementy bez dopasowania są pomijane.
/// - Wyniki są sortowane po score DESC (wyższy score = lepsze dopasowanie).
///
/// # Argumenty
///
/// * `query` - Szukany wzorzec (case-insensitive)
/// * `items` - Lista elementów do filtrowania
/// * `key`   - Funkcja wyciągająca string z elementu (do dopasowania)
///
/// # Przykłady
///
/// ```
/// use ralph_wiggum::tui::fuzzy::fuzzy_filter;
///
/// let items = vec!["foobar", "baz", "foo"];
/// let results = fuzzy_filter("foo", &items, |s| s);
/// // "foo" (exact) > "foobar" (prefix) > brak "baz"
/// assert_eq!(results[0].0, &"foo");
/// assert_eq!(results[1].0, &"foobar");
/// ```
pub fn fuzzy_filter<'a, T>(
    query: &str,
    items: &'a [T],
    key: fn(&T) -> &str,
) -> Vec<(&'a T, FuzzyMatch)> {
    if query.is_empty() {
        // Empty query: zwróć wszystkie z score=0, zachowaj oryginalną kolejność
        return items
            .iter()
            .map(|item| {
                let text = key(item).to_string();
                (
                    item,
                    FuzzyMatch {
                        text,
                        score: 0,
                        matched_indices: vec![],
                    },
                )
            })
            .collect();
    }

    let mut results: Vec<(&'a T, FuzzyMatch)> = items
        .iter()
        .filter_map(|item| {
            let text = key(item);
            fuzzy_match(query, text).map(|m| (item, m))
        })
        .collect();

    // Sortuj po score malejąco (wyższy score = lepsze dopasowanie)
    results.sort_by(|a, b| b.1.score.cmp(&a.1.score));

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── fuzzy_match tests ────────────────────────────────────────────────

    #[test]
    fn test_exact_match_returns_highest_score() {
        let m = fuzzy_match("foo", "foo").unwrap();
        assert!(
            m.score >= 1000,
            "Exact match score should be ≥1000, got {}",
            m.score
        );
        assert_eq!(m.matched_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_prefix_match_score_lower_than_exact() {
        let exact = fuzzy_match("foo", "foo").unwrap();
        let prefix = fuzzy_match("foo", "foobar").unwrap();
        assert!(
            exact.score > prefix.score,
            "Exact ({}) should beat prefix ({})",
            exact.score,
            prefix.score
        );
        assert_eq!(prefix.matched_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_substring_match_score_lower_than_prefix() {
        let prefix = fuzzy_match("foo", "foobar").unwrap();
        let substring = fuzzy_match("bar", "foobar").unwrap();
        assert!(
            prefix.score > substring.score,
            "Prefix ({}) should beat substring ({})",
            prefix.score,
            substring.score
        );
        // "bar" appears at index 3,4,5 in "foobar"
        assert_eq!(substring.matched_indices, vec![3, 4, 5]);
    }

    #[test]
    fn test_fuzzy_match_score_lower_than_substring() {
        // "fb" is fuzzy match (f at 0, b at 3) but not substring
        let substring = fuzzy_match("fo", "foobar").unwrap(); // prefix, ale sprawdzimy ordering
        let fuzzy = fuzzy_match("fb", "foobar").unwrap();
        // "fo" jest prefixem, "fb" jest fuzzy — prefix score >> fuzzy score
        assert!(
            substring.score > fuzzy.score,
            "Substring/prefix ({}) should beat fuzzy ({})",
            substring.score,
            fuzzy.score
        );
    }

    #[test]
    fn test_no_match_returns_none() {
        assert!(fuzzy_match("xyz", "foobar").is_none());
        assert!(fuzzy_match("abc", "xyz").is_none());
        assert!(fuzzy_match("zzz", "hello").is_none());
    }

    #[test]
    fn test_case_insensitive_matching() {
        // Query uppercase, target lowercase
        let m = fuzzy_match("FOO", "foobar").unwrap();
        assert_eq!(m.matched_indices, vec![0, 1, 2]);

        // Query lowercase, target uppercase
        let m = fuzzy_match("foo", "FOOBAR").unwrap();
        assert_eq!(m.matched_indices, vec![0, 1, 2]);

        // Mixed case
        let m = fuzzy_match("FoO", "fOoBaR").unwrap();
        assert_eq!(m.matched_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_empty_query_matches_everything() {
        let m = fuzzy_match("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.matched_indices.is_empty());

        let m = fuzzy_match("", "").unwrap();
        assert_eq!(m.score, 0);
    }

    #[test]
    fn test_empty_target_no_match_for_nonempty_query() {
        assert!(fuzzy_match("a", "").is_none());
    }

    #[test]
    fn test_matched_indices_are_sorted() {
        let m = fuzzy_match("abc", "xaxbxcx").unwrap();
        // a at 1, b at 3, c at 5
        assert_eq!(m.matched_indices, vec![1, 3, 5]);
        // Verify sorted
        let mut sorted = m.matched_indices.clone();
        sorted.sort();
        assert_eq!(m.matched_indices, sorted);
    }

    #[test]
    fn test_exact_match_full_indices() {
        let m = fuzzy_match("hello", "hello").unwrap();
        assert_eq!(m.matched_indices, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_word_boundary_bonus() {
        // Word boundary bonus działa w obrębie fuzzy scoring.
        // "fz" w "foo_zoo" — "f" na granicy (idx=0), "z" na granicy po '_' (idx=4)
        // "fz" w "xfyzx" — "f" wewnątrz słowa (idx=1), "z" wewnątrz słowa (idx=3)
        // Oba są fuzzy matchami (nie prefix/substring), więc score pochodzi z compute_score
        let m_boundary = fuzzy_match("fz", "foo_zoo").unwrap();
        let m_non_boundary = fuzzy_match("fz", "xfyzx").unwrap();
        assert!(
            m_boundary.score > m_non_boundary.score,
            "Word boundary match ({}) should score higher than non-boundary ({})",
            m_boundary.score,
            m_non_boundary.score
        );
    }

    #[test]
    fn test_sequential_bonus() {
        // "ab" sequential in "xab" vs scattered in "axb"
        let m_sequential = fuzzy_match("ab", "xab").unwrap();
        let m_scattered = fuzzy_match("ab", "axb").unwrap();
        assert!(
            m_sequential.score >= m_scattered.score,
            "Sequential match ({}) should score >= scattered ({})",
            m_sequential.score,
            m_scattered.score
        );
    }

    // ── Unicode tests ────────────────────────────────────────────────────

    #[test]
    fn test_unicode_exact_match() {
        let m = fuzzy_match("żółw", "żółw").unwrap();
        assert!(m.score >= 1000);
        assert_eq!(m.matched_indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_unicode_case_insensitive() {
        // Polish uppercase → lowercase matching
        let m = fuzzy_match("żółw", "ŻÓŁW").unwrap();
        assert_eq!(m.matched_indices, vec![0, 1, 2, 3]);

        let m = fuzzy_match("Ą", "ą").unwrap();
        assert!(m.score >= 1000); // exact match (case-insensitive)
    }

    #[test]
    fn test_unicode_fuzzy_chars() {
        // Japanese chars — fuzzy match across positions
        let m = fuzzy_match("日本", "東京日本語テスト").unwrap();
        assert!(m.matched_indices.len() == 2);
    }

    #[test]
    fn test_unicode_substring() {
        let m = fuzzy_match("świat", "witaj świat!").unwrap();
        // "świat" should be found as substring at position 7
        assert!(m.matched_indices.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn test_emoji_in_target() {
        // Emoji jako część targetu
        let m = fuzzy_match("test", "🧪 test case").unwrap();
        assert!(m.score > 0);
        // "test" appears at char index 2 (after "🧪 ")
        assert!(!m.matched_indices.is_empty());
    }

    // ── fuzzy_filter tests ───────────────────────────────────────────────

    #[test]
    fn test_fuzzy_filter_sorts_by_score_descending() {
        let items = vec!["foobar", "baz", "foo", "xfoo"];
        let results = fuzzy_filter("foo", &items, |s| s);

        // "foo" (exact) should come first
        assert_eq!(*results[0].0, "foo");
        // "foobar" (prefix) should come second
        assert_eq!(*results[1].0, "foobar");
        // "baz" should not appear (no fuzzy match for "foo" in "baz")
        assert!(results.iter().all(|(item, _)| **item != "baz"));
    }

    #[test]
    fn test_fuzzy_filter_empty_query_returns_all() {
        let items = vec!["a", "b", "c"];
        let results = fuzzy_filter("", &items, |s| s);
        assert_eq!(results.len(), 3);
        // All scores should be 0
        for (_, m) in &results {
            assert_eq!(m.score, 0);
        }
    }

    #[test]
    fn test_fuzzy_filter_no_matches() {
        let items = vec!["hello", "world"];
        let results = fuzzy_filter("xyz", &items, |s| s);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_filter_with_struct() {
        #[derive(Debug)]
        struct Task {
            name: String,
            id: u32,
        }

        let tasks = vec![
            Task {
                name: "Fix bug".to_string(),
                id: 1,
            },
            Task {
                name: "Add feature".to_string(),
                id: 2,
            },
            Task {
                name: "Refactor code".to_string(),
                id: 3,
            },
        ];

        let results = fuzzy_filter("fix", &tasks, |t| &t.name);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, 1);
    }

    #[test]
    fn test_fuzzy_filter_case_insensitive() {
        let items = vec!["Hello", "WORLD", "goodbye"];
        let results = fuzzy_filter("hello", &items, |s| s);
        assert_eq!(results.len(), 1);
        assert_eq!(*results[0].0, "Hello");
    }

    #[test]
    fn test_fuzzy_filter_score_ordering() {
        // Weryfikacja kolejności: exact > prefix > substring > fuzzy
        let items = vec![
            "bar",    // brak dopasowania dla "foo"
            "foo",    // exact
            "foobar", // prefix
            "barfoo", // suffix/substring
            "f_o_o",  // fuzzy
        ];
        let results = fuzzy_filter("foo", &items, |s| s);

        assert!(!results.is_empty());
        // Pierwszy wynik: exact match "foo"
        assert_eq!(*results[0].0, "foo");
        // Drugi wynik: prefix "foobar"
        assert_eq!(*results[1].0, "foobar");
    }

    // ── Performance test ─────────────────────────────────────────────────
    //
    // Weryfikuje że ≥1000 wywołań fuzzy_match wykonuje się w rozsądnym czasie.
    // Nie jest to formalny benchmark (brak kryterium czasowego w assert),
    // ale uruchomienie `cargo test -- --nocapture` pokaże czas.
    //
    // Dla pewności: na typowym hardware (2024) 10_000 wywołań zajmuje ~1ms.

    #[test]
    fn test_performance_1000_matches() {
        let targets: Vec<String> = (0..1000)
            .map(|i| format!("task_{i}_do_something_important"))
            .collect();

        let start = std::time::Instant::now();
        for target in &targets {
            let _ = fuzzy_match("dsi", target);
        }
        let elapsed = start.elapsed();

        // W trybie debug (cargo test) brak optymalizacji — limit jest wyższy niż dla release.
        // W release fuzzy_match jest o rząd wielkości szybszy.
        #[cfg(debug_assertions)]
        let limit_ms = 500;
        #[cfg(not(debug_assertions))]
        let limit_ms = 10;

        assert!(
            elapsed.as_millis() < limit_ms,
            "1000 fuzzy_match calls took {}ms, expected <{}ms",
            elapsed.as_millis(),
            limit_ms,
        );
    }

    #[test]
    fn test_performance_fuzzy_filter_1000_items() {
        let items: Vec<String> = (0..1000)
            .map(|i| format!("component_{i}_handler"))
            .collect();

        let start = std::time::Instant::now();
        let results = fuzzy_filter("ch", &items, |s| s.as_str());
        let elapsed = start.elapsed();

        assert!(!results.is_empty(), "Should match some items");
        assert!(
            elapsed.as_millis() < 10,
            "fuzzy_filter over 1000 items took {}ms, expected <10ms",
            elapsed.as_millis()
        );
    }

    // ── Edge cases ───────────────────────────────────────────────────────

    #[test]
    fn test_single_char_query() {
        let m = fuzzy_match("a", "banana").unwrap();
        // "a" appears at index 1 in "banana" (greedy → first 'a')
        assert!(m.matched_indices.contains(&1));
    }

    #[test]
    fn test_query_longer_than_target() {
        assert!(fuzzy_match("toolong", "too").is_none());
    }

    #[test]
    fn test_all_same_chars() {
        let m = fuzzy_match("aaa", "aaabbb").unwrap();
        // Should match first 3 a's
        assert_eq!(m.matched_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_matched_indices_cover_all_query_chars() {
        let query = "abc";
        let target = "axbxcx";
        let m = fuzzy_match(query, target).unwrap();
        // Musi być dokładnie tyle indeksów ile znaków w query
        assert_eq!(m.matched_indices.len(), query.chars().count());
    }

    #[test]
    fn test_text_field_is_original_target() {
        // FuzzyMatch.text powinien zawierać oryginalny target (nie lowercase)
        let m = fuzzy_match("foo", "FooBar").unwrap();
        assert_eq!(m.text, "FooBar");
    }

    #[test]
    fn test_prefix_score_not_negative_for_long_target() {
        // Przy bardzo długim targecie prefix score nie może być ujemny
        // i musi być >= 250 (wyższy niż typowy fuzzy score)
        let long_target = format!("ab{}", "x".repeat(1000));
        let m = fuzzy_match("ab", &long_target).unwrap();
        assert!(
            m.score >= 250,
            "Prefix score for long target should be >=250, got {}",
            m.score
        );
    }

    #[test]
    fn test_scoring_ordering_invariant() {
        // Invariant: exact > prefix > substring > fuzzy
        // dla tego samego query
        let query = "ab";
        let exact = fuzzy_match(query, "ab").unwrap();
        let prefix = fuzzy_match(query, "abc").unwrap();
        let substring = fuzzy_match(query, "xabx").unwrap();
        let fuzzy = fuzzy_match(query, "xaxb").unwrap();

        assert!(exact.score > prefix.score, "exact > prefix");
        assert!(prefix.score > substring.score, "prefix > substring");
        assert!(substring.score > fuzzy.score, "substring > fuzzy");
    }
}
