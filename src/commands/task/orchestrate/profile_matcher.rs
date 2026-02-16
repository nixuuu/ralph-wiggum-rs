use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashSet;

use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::VerifyProfile;

/// Skompilowane globy profilu — osobno include i exclude patterns.
/// Plik pasuje jeśli: matches(include) AND NOT matches(exclude).
struct CompiledProfile {
    name: String,
    include: GlobSet,
    exclude: GlobSet,
}

/// ProfileMatcher kompiluje glob patterns z profili weryfikacyjnych
/// i dopasowuje zmienione pliki do odpowiednich profili.
///
/// Obsługuje negację przez prefix `!` — pattern `!src/tests/**` wyklucza
/// pliki z `src/tests/`. Plik pasuje do profilu jeśli pasuje do któregokolwiek
/// include pattern i nie pasuje do żadnego exclude pattern.
pub struct ProfileMatcher {
    profiles: Vec<CompiledProfile>,
}

impl ProfileMatcher {
    /// Tworzy nowy ProfileMatcher z prekompilowanymi glob patterns.
    ///
    /// Patterns z prefixem `!` traktowane są jako wykluczenia (exclude).
    /// Pozostałe jako dopasowania (include).
    ///
    /// # Błędy
    /// Zwraca błąd jeśli któryś z glob patterns jest nieprawidłowy.
    pub fn new(profiles: &[VerifyProfile]) -> Result<Self> {
        let mut compiled_profiles = Vec::new();

        for profile in profiles {
            let mut include_builder = GlobSetBuilder::new();
            let mut exclude_builder = GlobSetBuilder::new();

            for pattern in &profile.paths {
                // Patterns z ! na początku to negacje (exclude)
                let (is_exclude, raw_pattern) = if let Some(stripped) = pattern.strip_prefix('!') {
                    (true, stripped)
                } else {
                    (false, pattern.as_str())
                };

                let glob = Glob::new(raw_pattern).map_err(|e| {
                    RalphError::Config(format!(
                        "Invalid glob pattern '{}' in profile '{}': {}",
                        pattern, profile.name, e
                    ))
                })?;

                if is_exclude {
                    exclude_builder.add(glob);
                } else {
                    include_builder.add(glob);
                }
            }

            let include = include_builder.build().map_err(|e| {
                RalphError::Config(format!(
                    "Failed to build include GlobSet for profile '{}': {}",
                    profile.name, e
                ))
            })?;
            let exclude = exclude_builder.build().map_err(|e| {
                RalphError::Config(format!(
                    "Failed to build exclude GlobSet for profile '{}': {}",
                    profile.name, e
                ))
            })?;

            compiled_profiles.push(CompiledProfile {
                name: profile.name.clone(),
                include,
                exclude,
            });
        }

        Ok(Self {
            profiles: compiled_profiles,
        })
    }

    /// Dopasowuje zmienione pliki do profili i zwraca listę nazw dopasowanych profili.
    ///
    /// Plik pasuje do profilu jeśli: matches(include) AND NOT matches(exclude).
    /// Zwrócona lista zachowuje kolejność profili z konfiguracji, bez duplikatów.
    pub fn match_changed_files(&self, changed_files: &[String]) -> Vec<String> {
        let mut matched = Vec::new();

        for profile in &self.profiles {
            let has_match = changed_files
                .iter()
                .any(|file| profile.include.is_match(file) && !profile.exclude.is_match(file));

            if has_match {
                matched.push(profile.name.clone());
            }
        }

        matched
    }
}

/// Rozwiązuje ostateczną listę profili weryfikacyjnych do uruchomienia.
///
/// Łączy profile z zadania (task_profiles) i profile dopasowane na podstawie
/// zmienionych plików (git_matched), eliminując duplikaty i zachowując kolejność:
/// 1. Najpierw profile z task_profiles (w kolejności z TOML)
/// 2. Następnie profile z git_matched (które jeszcze nie były uwzględnione)
///
/// # Argumenty
/// * `task_profiles` - Lista nazw profili określonych w zadaniu
/// * `git_matched` - Lista nazw profili dopasowanych na podstawie zmienionych plików
/// * `all_profiles` - Wszystkie zdefiniowane profile weryfikacyjne
///
/// # Zwraca
/// Lista referencji do profili weryfikacyjnych, w kolejności wykonania.
/// Profile nieznalezione w `all_profiles` są pomijane.
///
/// # Przykład
/// ```no_run
/// use ralph_wiggum::commands::task::orchestrate::profile_matcher::resolve_verify_profiles;
/// use ralph_wiggum::shared::file_config::VerifyProfile;
///
/// let all_profiles = vec![/* profile */];
/// let task_profiles = vec!["backend".to_string()];
/// let git_matched = vec!["frontend".to_string(), "backend".to_string()];
///
/// let resolved = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
/// // Kolejność: ["backend", "frontend"] — backend z task_profiles idzie pierwszy,
/// // backend z git_matched jest pomijany (duplikat), frontend z git_matched jest dodany
/// ```
pub fn resolve_verify_profiles<'a>(
    task_profiles: &[String],
    git_matched: &[String],
    all_profiles: &'a [VerifyProfile],
) -> Vec<&'a VerifyProfile> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    // 1. Najpierw dodaj profile z task_profiles (w kolejności TOML)
    for name in task_profiles {
        if let Some(profile) = all_profiles.iter().find(|p| p.name == *name)
            && seen.insert(name.clone())
        {
            result.push(profile);
        }
    }

    // 2. Następnie dodaj profile z git_matched (które jeszcze nie były uwzględnione)
    for name in git_matched {
        if let Some(profile) = all_profiles.iter().find(|p| p.name == *name)
            && seen.insert(name.clone())
        {
            result.push(profile);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_profile(name: &str, paths: Vec<&str>) -> VerifyProfile {
        VerifyProfile {
            name: name.to_string(),
            description: None,
            paths: paths.iter().map(|s| s.to_string()).collect(),
            working_dir: None,
            verify_commands: Vec::new(),
            setup_commands: Vec::new(),
        }
    }

    #[test]
    fn test_profile_matcher_empty() {
        let profiles = vec![];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec!["src/main.rs".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert!(matched.is_empty());
    }

    #[test]
    fn test_profile_matcher_single_match() {
        let profiles = vec![create_test_profile("backend", vec!["src/**/*.rs"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec!["src/main.rs".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(matched, vec!["backend"]);
    }

    #[test]
    fn test_profile_matcher_no_match() {
        let profiles = vec![create_test_profile("backend", vec!["src/**/*.rs"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec!["docs/README.md".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert!(matched.is_empty());
    }

    #[test]
    fn test_profile_matcher_multiple_profiles() {
        let profiles = vec![
            create_test_profile("backend", vec!["src/api/**/*.rs"]),
            create_test_profile("frontend", vec!["src/ui/**/*.rs"]),
            create_test_profile("tests", vec!["tests/**/*.rs"]),
        ];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec![
            "src/api/handlers.rs".to_string(),
            "tests/integration.rs".to_string(),
        ];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(matched, vec!["backend", "tests"]);
    }

    #[test]
    fn test_profile_matcher_multiple_paths_in_profile() {
        let profiles = vec![create_test_profile(
            "docs",
            vec!["docs/**/*.md", "README.md"],
        )];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed1 = vec!["docs/guide.md".to_string()];
        let matched1 = matcher.match_changed_files(&changed1);
        assert_eq!(matched1, vec!["docs"]);

        let changed2 = vec!["README.md".to_string()];
        let matched2 = matcher.match_changed_files(&changed2);
        assert_eq!(matched2, vec!["docs"]);
    }

    #[test]
    fn test_profile_matcher_no_duplicates() {
        let profiles = vec![create_test_profile("backend", vec!["src/**/*.rs"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(matched, vec!["backend"]);
    }

    #[test]
    fn test_profile_matcher_invalid_glob() {
        let profiles = vec![create_test_profile("bad", vec!["src/**[invalid"])];
        let result = ProfileMatcher::new(&profiles);
        assert!(result.is_err());
    }

    #[test]
    fn test_profile_matcher_negation() {
        let profiles = vec![create_test_profile(
            "no-tests",
            vec!["src/**/*.rs", "!src/tests/**"],
        )];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        // src/main.rs pasuje do include i nie pasuje do exclude → match
        let changed1 = vec!["src/main.rs".to_string()];
        let matched1 = matcher.match_changed_files(&changed1);
        assert_eq!(matched1, vec!["no-tests"]);

        // src/tests/unit.rs pasuje do include (src/**/*.rs) ALE też do exclude → brak match
        let changed2 = vec!["src/tests/unit.rs".to_string()];
        let matched2 = matcher.match_changed_files(&changed2);
        assert!(
            matched2.is_empty(),
            "plik w src/tests/ powinien być wykluczony"
        );

        // Mix: plik pasujący i wykluczony — profil pasuje (bo main.rs pasuje)
        let changed3 = vec!["src/main.rs".to_string(), "src/tests/unit.rs".to_string()];
        let matched3 = matcher.match_changed_files(&changed3);
        assert_eq!(matched3, vec!["no-tests"]);
    }

    #[test]
    fn test_profile_matcher_only_excludes() {
        // Profil z samymi exclude patterns — nigdy nie dopasuje (brak include)
        let profiles = vec![create_test_profile("bad", vec!["!src/tests/**"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();
        let changed = vec!["src/main.rs".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert!(
            matched.is_empty(),
            "profil bez include patterns nie powinien pasować"
        );
    }

    #[test]
    fn test_profile_matcher_brace_expansion() {
        // globset obsługuje {a,b} syntax
        let profiles = vec![create_test_profile("sources", vec!["src/**/*.{rs,toml}"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed1 = vec!["src/main.rs".to_string()];
        let matched1 = matcher.match_changed_files(&changed1);
        assert_eq!(matched1, vec!["sources"]);

        let changed2 = vec!["Cargo.toml".to_string()];
        let matched2 = matcher.match_changed_files(&changed2);
        assert!(matched2.is_empty()); // nie pasuje, bo pattern to src/**/*.toml

        let changed3 = vec!["src/config.toml".to_string()];
        let matched3 = matcher.match_changed_files(&changed3);
        assert_eq!(matched3, vec!["sources"]);
    }

    #[test]
    fn test_resolve_verify_profiles_empty() {
        let all_profiles = vec![];
        let result = resolve_verify_profiles(&[], &[], &all_profiles);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_verify_profiles_task_only() {
        let all_profiles = vec![
            create_test_profile("backend", vec!["src/**/*.rs"]),
            create_test_profile("frontend", vec!["ui/**/*.ts"]),
        ];
        let task_profiles = vec!["backend".to_string()];
        let git_matched = vec![];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "backend");
    }

    #[test]
    fn test_resolve_verify_profiles_git_only() {
        let all_profiles = vec![
            create_test_profile("backend", vec!["src/**/*.rs"]),
            create_test_profile("frontend", vec!["ui/**/*.ts"]),
        ];
        let task_profiles = vec![];
        let git_matched = vec!["frontend".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "frontend");
    }

    #[test]
    fn test_resolve_verify_profiles_no_duplicates() {
        let all_profiles = vec![
            create_test_profile("backend", vec!["src/**/*.rs"]),
            create_test_profile("frontend", vec!["ui/**/*.ts"]),
        ];
        let task_profiles = vec!["backend".to_string()];
        let git_matched = vec!["backend".to_string(), "frontend".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "backend");
        assert_eq!(result[1].name, "frontend");
    }

    #[test]
    fn test_resolve_verify_profiles_preserves_task_order() {
        let all_profiles = vec![
            create_test_profile("a", vec![]),
            create_test_profile("b", vec![]),
            create_test_profile("c", vec![]),
        ];
        let task_profiles = vec!["c".to_string(), "a".to_string()];
        let git_matched = vec!["b".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "c");
        assert_eq!(result[1].name, "a");
        assert_eq!(result[2].name, "b");
    }

    #[test]
    fn test_resolve_verify_profiles_unknown_profile() {
        let all_profiles = vec![create_test_profile("backend", vec!["src/**/*.rs"])];
        let task_profiles = vec!["backend".to_string(), "unknown".to_string()];
        let git_matched = vec!["unknown".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "backend");
    }

    #[test]
    fn test_resolve_verify_profiles_git_matched_appended() {
        let all_profiles = vec![
            create_test_profile("lint", vec![]),
            create_test_profile("test", vec![]),
            create_test_profile("build", vec![]),
        ];
        let task_profiles = vec!["lint".to_string()];
        let git_matched = vec!["test".to_string(), "build".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "lint");
        assert_eq!(result[1].name, "test");
        assert_eq!(result[2].name, "build");
    }

    // ===== Testy zgodnie z task 41.4 =====

    #[test]
    fn test_simple_glob_apps_frontend() {
        // Test: simple glob Apps/Frontend/*.ts matches Apps/Frontend/index.ts
        // UWAGA: globset domyślnie ma literal_separator=false,
        // więc * matchuje również '/' i pasuje do zagnieżdżonych ścieżek.
        // Użytkownicy powinni używać ** świadomie, lub w przyszłości
        // rozważyć włączenie literal_separator(true) w GlobBuilder.
        let profiles = vec![create_test_profile("frontend", vec!["Apps/Frontend/*.ts"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        // Matchuje pliki bezpośrednio w Apps/Frontend/
        let changed = vec!["Apps/Frontend/index.ts".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(matched, vec!["frontend"]);

        // Nie matchuje zagnieżdżonych plików z innym rozszerzeniem
        let changed_tsx = vec!["Apps/Frontend/index.tsx".to_string()];
        let matched_tsx = matcher.match_changed_files(&changed_tsx);
        assert!(
            matched_tsx.is_empty(),
            "Apps/Frontend/*.ts nie powinien matchować .tsx"
        );

        // Z literal_separator=false (domyślne), * matchuje też zagnieżdżone pliki
        let changed_nested = vec!["Apps/Frontend/components/Button.ts".to_string()];
        let matched_nested = matcher.match_changed_files(&changed_nested);
        assert_eq!(
            matched_nested,
            vec!["frontend"],
            "z domyślnym literal_separator=false, * matchuje przez /"
        );

        // Nie matchuje plików poza Apps/Frontend/
        let changed_outside = vec!["Apps/Backend/index.ts".to_string()];
        let matched_outside = matcher.match_changed_files(&changed_outside);
        assert!(
            matched_outside.is_empty(),
            "Apps/Frontend/*.ts nie powinien matchować plików poza Apps/Frontend/"
        );
    }

    #[test]
    fn test_recursive_glob_matches_nested() {
        // Test: recursive ** matches nested files
        let profiles = vec![create_test_profile("backend", vec!["src/**/*.rs"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        // Głęboko zagnieżdżone pliki
        let changed = vec![
            "src/commands/task/orchestrate/profile_matcher.rs".to_string(),
            "src/lib.rs".to_string(),
            "src/a/b/c/d/e/deep.rs".to_string(),
        ];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(matched, vec!["backend"]);
    }

    #[test]
    fn test_negation_excludes_test_files() {
        // Test: negacja !**/test/** excludes test files
        let profiles = vec![create_test_profile(
            "no-tests",
            vec!["src/**/*.rs", "!**/test/**", "!**/tests/**"],
        )];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        // Pliki produkcyjne pasują
        let changed_prod = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let matched_prod = matcher.match_changed_files(&changed_prod);
        assert_eq!(matched_prod, vec!["no-tests"]);

        // Pliki testowe są wykluczane
        let changed_tests = vec![
            "src/test/unit.rs".to_string(),
            "src/tests/integration.rs".to_string(),
        ];
        let matched_tests = matcher.match_changed_files(&changed_tests);
        assert!(
            matched_tests.is_empty(),
            "pliki w test/ i tests/ powinny być wykluczone"
        );
    }

    #[test]
    fn test_alternatives_ts_tsx() {
        // Test: alternatywy {*.ts,*.tsx} matches both
        let profiles = vec![create_test_profile("typescript", vec!["**/*.{ts,tsx}"])];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed_ts = vec!["src/index.ts".to_string()];
        let matched_ts = matcher.match_changed_files(&changed_ts);
        assert_eq!(matched_ts, vec!["typescript"]);

        let changed_tsx = vec!["src/components/App.tsx".to_string()];
        let matched_tsx = matcher.match_changed_files(&changed_tsx);
        assert_eq!(matched_tsx, vec!["typescript"]);

        let changed_both = vec![
            "src/index.ts".to_string(),
            "src/App.tsx".to_string(),
            "src/utils.ts".to_string(),
        ];
        let matched_both = matcher.match_changed_files(&changed_both);
        assert_eq!(matched_both, vec!["typescript"]);

        // Inne rozszerzenia nie pasują
        let changed_js = vec!["src/index.js".to_string()];
        let matched_js = matcher.match_changed_files(&changed_js);
        assert!(matched_js.is_empty(), "plik .js nie powinien pasować");
    }

    #[test]
    fn test_no_matching_files_empty_result() {
        // Test: brak pasujących plików → empty result
        let profiles = vec![
            create_test_profile("backend", vec!["src/**/*.rs"]),
            create_test_profile("frontend", vec!["ui/**/*.tsx"]),
        ];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed = vec![
            "docs/README.md".to_string(),
            "LICENSE".to_string(),
            "Cargo.toml".to_string(),
        ];
        let matched = matcher.match_changed_files(&changed);
        assert!(matched.is_empty(), "żaden profil nie powinien pasować");
    }

    #[test]
    fn test_multiple_profiles_match_same_file() {
        // Test: wiele profili matchują ten sam plik → oba w wyniku
        let profiles = vec![
            create_test_profile("rust-files", vec!["**/*.rs"]),
            create_test_profile("src-files", vec!["src/**/*"]),
            create_test_profile("api-files", vec!["src/api/**/*.rs"]),
        ];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed = vec!["src/api/handlers.rs".to_string()];
        let matched = matcher.match_changed_files(&changed);
        assert_eq!(
            matched.len(),
            3,
            "wszystkie 3 profile powinny pasować do src/api/handlers.rs"
        );
        assert_eq!(matched[0], "rust-files");
        assert_eq!(matched[1], "src-files");
        assert_eq!(matched[2], "api-files");
    }

    #[test]
    fn test_resolve_verify_profiles_union_no_duplicates() {
        // Test: resolve_verify_profiles — unia task+git bez duplikatów
        let all_profiles = vec![
            create_test_profile("lint", vec!["**/*.rs"]),
            create_test_profile("test", vec!["tests/**/*"]),
            create_test_profile("build", vec!["src/**/*"]),
        ];
        let task_profiles = vec!["lint".to_string(), "build".to_string()];
        let git_matched = vec!["build".to_string(), "test".to_string(), "lint".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);

        // Kolejność: lint (task), build (task), test (git, nowy)
        // lint i build z git są pomijane (duplikaty)
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "lint");
        assert_eq!(result[1].name, "build");
        assert_eq!(result[2].name, "test");
    }

    #[test]
    fn test_resolve_verify_profiles_task_first_order() {
        // Test: resolve_verify_profiles — kolejność task profiles first
        let all_profiles = vec![
            create_test_profile("a", vec![]),
            create_test_profile("b", vec![]),
            create_test_profile("c", vec![]),
            create_test_profile("d", vec![]),
        ];
        let task_profiles = vec!["d".to_string(), "b".to_string()];
        let git_matched = vec!["a".to_string(), "c".to_string(), "b".to_string()];
        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);

        // Kolejność: d (task), b (task), a (git, nowy), c (git, nowy)
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].name, "d");
        assert_eq!(result[1].name, "b");
        assert_eq!(result[2].name, "a");
        assert_eq!(result[3].name, "c");
    }

    #[test]
    fn test_profile_empty_paths_never_matches() {
        // Test: profil z pustymi paths → nigdy nie matchuje po git diff
        let profiles = vec![
            create_test_profile("always-run", vec![]), // pusty paths
            create_test_profile("backend", vec!["src/**/*.rs"]),
        ];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        let changed = vec![
            "src/main.rs".to_string(),
            "docs/README.md".to_string(),
            "anything.txt".to_string(),
        ];
        let matched = matcher.match_changed_files(&changed);

        // Tylko backend pasuje, always-run z pustymi paths nigdy nie matchuje
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0], "backend");
        assert!(
            !matched.contains(&"always-run".to_string()),
            "profil z pustymi paths nie powinien matchować żadnych plików"
        );
    }

    // ── Task 53.4: Test duplikaty nazw profili — matczowanie ──

    #[test]
    fn test_profile_matcher_duplicate_names() {
        // Dwa profile o tej samej nazwie "frontend" z różnymi path patterns
        let profiles = vec![
            create_test_profile("frontend", vec!["src/ui/**/*.ts"]),
            create_test_profile("frontend", vec!["src/components/**/*.tsx"]),
        ];
        let matcher = ProfileMatcher::new(&profiles).unwrap();

        // Plik pasujący tylko do pierwszego profilu
        let changed_ts = vec!["src/ui/App.ts".to_string()];
        let matched_ts = matcher.match_changed_files(&changed_ts);
        assert_eq!(
            matched_ts.len(),
            1,
            "Plik .ts powinien pasować do pierwszego profilu frontend"
        );
        assert_eq!(matched_ts[0], "frontend");

        // Plik pasujący tylko do drugiego profilu
        let changed_tsx = vec!["src/components/Button.tsx".to_string()];
        let matched_tsx = matcher.match_changed_files(&changed_tsx);
        assert_eq!(
            matched_tsx.len(),
            1,
            "Plik .tsx powinien pasować do drugiego profilu frontend"
        );
        assert_eq!(matched_tsx[0], "frontend");

        // Dwa pliki pasujące do obu profili — oba profile zwracane (duplikaty nazw)
        let changed_both = vec![
            "src/ui/App.ts".to_string(),
            "src/components/Button.tsx".to_string(),
        ];
        let matched_both = matcher.match_changed_files(&changed_both);
        // UWAGA: match_changed_files zwraca profile.name dla każdego profilu który matchuje.
        // Z duplikatami nazw, oba profile "frontend" matchują → zwraca ["frontend", "frontend"]
        assert_eq!(
            matched_both.len(),
            2,
            "Oba profile 'frontend' powinny być zwrócone (duplikaty nazw dozwolone)"
        );
        assert_eq!(matched_both[0], "frontend");
        assert_eq!(matched_both[1], "frontend");
    }

    #[test]
    fn test_resolve_verify_profiles_duplicate_names() {
        // Test resolve_verify_profiles z duplikatami nazw
        let all_profiles = vec![
            create_test_profile("frontend", vec!["src/ui/**/*.ts"]),
            create_test_profile("frontend", vec!["src/components/**/*.tsx"]),
            create_test_profile("backend", vec!["src/api/**/*.rs"]),
        ];

        // task_profiles i git_matched zawierają "frontend"
        let task_profiles = vec!["frontend".to_string()];
        let git_matched = vec!["frontend".to_string(), "backend".to_string()];

        let result = resolve_verify_profiles(&task_profiles, &git_matched, &all_profiles);

        // resolve_verify_profiles używa HashSet z seen.insert(name.clone())
        // Więc deduplikuje po nazwie — ale find() zwraca pierwszy match
        // Zachowanie: pierwszy profil "frontend" z all_profiles jest wzięty,
        // drugi "frontend" jest pomijany (duplicate w seen),
        // następnie "backend" jest dodany
        assert_eq!(
            result.len(),
            2,
            "Deduplikacja po nazwie — tylko pierwszy 'frontend' i 'backend'"
        );
        assert_eq!(result[0].name, "frontend");
        assert_eq!(
            result[0].paths,
            vec!["src/ui/**/*.ts"],
            "Powinien zwrócić pierwszy profil 'frontend'"
        );
        assert_eq!(result[1].name, "backend");
    }
}
