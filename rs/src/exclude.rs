//! Gitignore-style exclude filter for disk-to-repo copy/sync operations.
//!
//! [`ExcludeFilter`] supports `!` negation, trailing `/` for directory-only
//! patterns, and anchored patterns (patterns containing `/` are matched against
//! the full relative path; others are matched against the basename only).
//! Last matching rule wins, enabling negation overrides.

use std::fs;
use std::path::Path;

use crate::Result;
use crate::glob::fnmatch;

// ---------------------------------------------------------------------------
// Internal representation
// ---------------------------------------------------------------------------

/// A single parsed gitignore-style pattern.
#[derive(Debug, Clone)]
struct Pattern {
    /// The pattern string with the leading `!` and trailing `/` stripped.
    raw: String,
    /// True if this pattern was prefixed with `!` (re-includes previously excluded paths).
    negated: bool,
    /// True if this pattern had a trailing `/` (only matches directories).
    dir_only: bool,
}

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// Gitignore-style exclude filter for disk-to-repo copy/sync operations.
///
/// Patterns follow the same rules as `.gitignore`:
///
/// - Blank lines and lines starting with `#` are ignored.
/// - A leading `!` negates the pattern (re-includes a previously excluded path).
/// - A trailing `/` restricts the pattern to directories only.
/// - If a pattern contains `/` (other than a trailing one), it is matched
///   against the full relative path; otherwise it is matched against the
///   basename only.
/// - `*` matches any sequence of characters; `?` matches a single character.
/// - Unlike [`crate::glob::glob_match`], matching here does **not** apply
///   dotfile protection, so `*.pyc` matches `.hidden.pyc`.
/// - The last matching rule wins, so negation patterns placed after positive
///   ones can re-include specific paths.
///
/// # Example
///
/// ```rust
/// use vost::ExcludeFilter;
///
/// let mut f = ExcludeFilter::new();
/// f.add_patterns(&["*.log", "!important.log"]);
///
/// assert!(f.is_excluded("debug.log", false));
/// assert!(!f.is_excluded("important.log", false));
/// assert!(!f.is_excluded("src/main.rs", false));
/// ```
#[derive(Debug, Clone, Default)]
pub struct ExcludeFilter {
    patterns: Vec<Pattern>,
    /// When `true`, `.gitignore` files found during directory walks are loaded
    /// and their rules applied.
    pub gitignore: bool,
    /// Per-directory `.gitignore` patterns, keyed by relative directory path.
    /// Empty string key = root directory.
    gitignore_filters: std::collections::HashMap<String, Vec<Pattern>>,
}

impl ExcludeFilter {
    /// Create an empty filter that excludes nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a filter pre-loaded with inline `patterns` and/or patterns read
    /// from the file at `exclude_from`.
    ///
    /// Either argument may be `None`.  If `exclude_from` points to a file that
    /// does not exist the call still succeeds (same behaviour as
    /// [`load_from_file`](Self::load_from_file)).
    ///
    /// # Errors
    ///
    /// Returns an error only if `exclude_from` exists but cannot be read.
    pub fn with_options(
        patterns: Option<&[&str]>,
        exclude_from: Option<&Path>,
    ) -> Result<Self> {
        let mut filter = Self::new();
        if let Some(pats) = patterns {
            filter.add_patterns(pats);
        }
        if let Some(path) = exclude_from {
            filter.load_from_file(path)?;
        }
        Ok(filter)
    }

    /// Add gitignore-style patterns from a slice of strings.
    ///
    /// Blank lines and lines whose first non-whitespace character is `#` are
    /// silently skipped.  Each accepted pattern is parsed for a leading `!`
    /// (negation) and a trailing `/` (directory-only), which are stripped
    /// before the raw pattern is stored.  Patterns that are empty after
    /// stripping are also skipped.
    pub fn add_patterns(&mut self, patterns: &[&str]) {
        for &raw in patterns {
            // Skip blank lines and comments.
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Parse negation prefix.
            let (negated, after_neg) = if trimmed.starts_with('!') {
                (true, &trimmed[1..])
            } else {
                (false, trimmed)
            };

            // Parse directory-only suffix.
            let (dir_only, pat) = if after_neg.ends_with('/') {
                (true, &after_neg[..after_neg.len() - 1])
            } else {
                (false, after_neg)
            };

            // Skip patterns that are empty after stripping.
            if pat.is_empty() {
                continue;
            }

            self.patterns.push(Pattern {
                raw: pat.to_string(),
                negated,
                dir_only,
            });
        }
    }

    /// Load patterns from a file (one pattern per line).
    ///
    /// Trailing whitespace is stripped from each line before parsing.  If the
    /// file does not exist this method returns `Ok(())` silently, matching
    /// the behaviour of other ports and making it safe to pass a path that may
    /// not yet exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub fn load_from_file(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let contents = fs::read_to_string(path)?;
        let lines: Vec<&str> = contents
            .lines()
            .map(|l| l.trim_end())
            .filter(|l| !l.is_empty())
            .collect();

        // Borrow-checker note: `lines` borrows `contents` so we must collect
        // owned strings before calling `add_patterns`.
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        self.add_patterns(&refs);

        Ok(())
    }

    /// Return `true` if `rel_path` should be excluded.
    ///
    /// `rel_path` must be a forward-slash-separated relative path (e.g.
    /// `"src/main.rs"` or `"build/output.o"`).  `is_dir` should be `true`
    /// when the path refers to a directory; directory-only patterns (trailing
    /// `/`) are skipped when `is_dir` is `false`.
    ///
    /// The last matching rule wins, so a negation pattern placed after a
    /// positive pattern will re-include the path.
    pub fn is_excluded(&self, rel_path: &str, is_dir: bool) -> bool {
        let mut excluded = false;

        for p in &self.patterns {
            // Directory-only patterns don't apply to files.
            if p.dir_only && !is_dir {
                continue;
            }

            if match_pattern(&p.raw, rel_path) {
                excluded = !p.negated;
            }
        }

        excluded
    }

    /// Return `true` if at least one pattern has been loaded or gitignore
    /// mode is enabled.
    ///
    /// A filter with no patterns and gitignore disabled never excludes
    /// anything; callers may use this to skip the filtering step entirely
    /// when no patterns are active.
    pub fn active(&self) -> bool {
        !self.patterns.is_empty() || self.gitignore
    }

    /// Load `.gitignore` from `abs_dir` if gitignore mode is enabled.
    ///
    /// `rel_dir` is the relative directory path (empty string for root).
    /// Patterns from the `.gitignore` file are stored and used by
    /// [`is_excluded_in_walk`](Self::is_excluded_in_walk).
    pub fn enter_directory(&mut self, abs_dir: &Path, rel_dir: &str) {
        if !self.gitignore {
            return;
        }
        if self.gitignore_filters.contains_key(rel_dir) {
            return;
        }
        let gi = abs_dir.join(".gitignore");
        if gi.is_file() {
            if let Ok(contents) = fs::read_to_string(&gi) {
                let mut patterns = Vec::new();
                for line in contents.lines() {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    let (negated, after_neg) = if trimmed.starts_with('!') {
                        (true, &trimmed[1..])
                    } else {
                        (false, trimmed)
                    };
                    let (dir_only, pat) = if after_neg.ends_with('/') {
                        (true, &after_neg[..after_neg.len() - 1])
                    } else {
                        (false, after_neg)
                    };
                    if pat.is_empty() {
                        continue;
                    }
                    patterns.push(Pattern {
                        raw: pat.to_string(),
                        negated,
                        dir_only,
                    });
                }
                self.gitignore_filters.insert(rel_dir.to_string(), patterns);
            } else {
                self.gitignore_filters.insert(rel_dir.to_string(), Vec::new());
            }
        } else {
            self.gitignore_filters.insert(rel_dir.to_string(), Vec::new());
        }
    }

    /// Check base patterns + loaded `.gitignore` hierarchy during a walk.
    ///
    /// Called during directory walking after [`enter_directory`](Self::enter_directory)
    /// has been invoked for every ancestor directory.
    pub fn is_excluded_in_walk(&self, rel_path: &str, is_dir: bool) -> bool {
        // Check base patterns first
        let check = if is_dir {
            format!("{}/", rel_path)
        } else {
            rel_path.to_string()
        };
        let _ = &check; // suppress unused warning

        // Base patterns (--exclude / --exclude-from)
        {
            let mut excluded = false;
            for p in &self.patterns {
                if p.dir_only && !is_dir {
                    continue;
                }
                if match_pattern(&p.raw, rel_path) {
                    excluded = !p.negated;
                }
            }
            if excluded {
                return true;
            }
        }

        if !self.gitignore {
            return false;
        }

        // Auto-exclude .gitignore files themselves
        if !is_dir {
            let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
            if basename == ".gitignore" {
                return true;
            }
        }

        // Walk .gitignore filters from root to deepest ancestor.
        // Git semantics: last (deepest) matching rule wins.
        let parts: Vec<&str> = rel_path.split('/').collect();
        let mut excluded: Option<bool> = None;
        for depth in 0..parts.len() {
            let dir_key = if depth == 0 {
                String::new()
            } else {
                parts[..depth].join("/")
            };
            if let Some(dir_patterns) = self.gitignore_filters.get(&dir_key) {
                // Path relative to this .gitignore's directory
                let sub = parts[depth..].join("/");
                for p in dir_patterns {
                    if p.dir_only && !is_dir {
                        continue;
                    }
                    if match_pattern(&p.raw, &sub) {
                        excluded = Some(!p.negated);
                    }
                }
            }
        }

        excluded.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Match a single parsed pattern against a relative path.
///
/// - If `pattern` contains `/`, it is matched against the full `path`.
/// - Otherwise it is matched against the basename of `path` only.
///
/// Uses [`fnmatch`] (no dotfile protection).
fn match_pattern(pattern: &str, path: &str) -> bool {
    if pattern.contains('/') {
        fnmatch(pattern.as_bytes(), path.as_bytes())
    } else {
        let basename = path.rsplit('/').next().unwrap_or(path);
        fnmatch(pattern.as_bytes(), basename.as_bytes())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // ------------------------------------------------------------------
    // add_patterns / parsing
    // ------------------------------------------------------------------

    #[test]
    fn test_empty_filter_excludes_nothing() {
        let f = ExcludeFilter::new();
        assert!(!f.active());
        assert!(!f.is_excluded("anything.txt", false));
        assert!(!f.is_excluded("dir/file.py", true));
    }

    #[test]
    fn test_simple_wildcard() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["*.log"]);
        assert!(f.is_excluded("debug.log", false));
        assert!(f.is_excluded("subdir/error.log", false));
        assert!(!f.is_excluded("main.rs", false));
    }

    #[test]
    fn test_basename_only_match() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["build"]);
        // Matches the basename in any directory.
        assert!(f.is_excluded("build", true));
        assert!(f.is_excluded("project/build", true));
        assert!(!f.is_excluded("notbuild", false));
    }

    #[test]
    fn test_anchored_pattern_full_path() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["src/generated/*.rs"]);
        assert!(f.is_excluded("src/generated/foo.rs", false));
        assert!(!f.is_excluded("other/generated/foo.rs", false));
    }

    #[test]
    fn test_dir_only_pattern() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["build/"]);
        // Only matches when is_dir == true.
        assert!(f.is_excluded("build", true));
        assert!(!f.is_excluded("build", false));
        assert!(f.is_excluded("project/build", true));
    }

    #[test]
    fn test_negation_last_wins() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["*.log", "!important.log"]);
        assert!(f.is_excluded("debug.log", false));
        assert!(!f.is_excluded("important.log", false));
    }

    #[test]
    fn test_negation_then_re_exclude() {
        // Positive after negation re-excludes.
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["*.log", "!important.log", "important.log"]);
        assert!(f.is_excluded("important.log", false));
    }

    #[test]
    fn test_comments_and_blanks_skipped() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["", "  ", "# this is a comment", "*.pyc"]);
        // Only *.pyc should be active.
        assert_eq!(f.patterns.len(), 1);
        assert!(f.is_excluded("module.pyc", false));
    }

    #[test]
    fn test_no_dotfile_protection() {
        // Unlike glob_match, fnmatch in exclude mode matches dotfiles.
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["*.pyc"]);
        assert!(f.is_excluded(".hidden.pyc", false));
    }

    #[test]
    fn test_active() {
        let mut f = ExcludeFilter::new();
        assert!(!f.active());
        f.add_patterns(&["*.log"]);
        assert!(f.active());
    }

    // ------------------------------------------------------------------
    // load_from_file
    // ------------------------------------------------------------------

    #[test]
    fn test_load_from_nonexistent_file_is_ok() {
        let mut f = ExcludeFilter::new();
        let result = f.load_from_file(Path::new("/nonexistent/path/to/file.gitignore"));
        assert!(result.is_ok());
        assert!(!f.active());
    }

    #[test]
    fn test_load_from_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "# comment").unwrap();
        writeln!(tmp, "*.log").unwrap();
        writeln!(tmp, "!important.log").unwrap();
        writeln!(tmp, "build/").unwrap();
        tmp.flush().unwrap();

        let mut f = ExcludeFilter::new();
        f.load_from_file(tmp.path()).unwrap();

        assert!(f.active());
        assert!(f.is_excluded("app.log", false));
        assert!(!f.is_excluded("important.log", false));
        assert!(f.is_excluded("build", true));
        assert!(!f.is_excluded("build", false));
    }

    // ------------------------------------------------------------------
    // with_options constructor
    // ------------------------------------------------------------------

    #[test]
    fn test_with_options_patterns_only() {
        let f = ExcludeFilter::with_options(Some(&["*.tmp"]), None).unwrap();
        assert!(f.is_excluded("scratch.tmp", false));
        assert!(!f.is_excluded("scratch.rs", false));
    }

    #[test]
    fn test_with_options_file_only() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "*.o").unwrap();
        tmp.flush().unwrap();

        let f = ExcludeFilter::with_options(None, Some(tmp.path())).unwrap();
        assert!(f.is_excluded("main.o", false));
        assert!(!f.is_excluded("main.rs", false));
    }

    #[test]
    fn test_with_options_both() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "*.o").unwrap();
        tmp.flush().unwrap();

        let f =
            ExcludeFilter::with_options(Some(&["*.log"]), Some(tmp.path())).unwrap();
        assert!(f.is_excluded("debug.log", false));
        assert!(f.is_excluded("main.o", false));
        assert!(!f.is_excluded("main.rs", false));
    }

    #[test]
    fn test_with_options_none_none() {
        let f = ExcludeFilter::with_options(None, None).unwrap();
        assert!(!f.active());
    }

    // ------------------------------------------------------------------
    // match_pattern edge cases
    // ------------------------------------------------------------------

    #[test]
    fn test_question_mark_wildcard() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["?.log"]);
        assert!(f.is_excluded("a.log", false));
        assert!(!f.is_excluded("ab.log", false));
    }

    #[test]
    fn test_exact_pattern() {
        let mut f = ExcludeFilter::new();
        f.add_patterns(&["Makefile"]);
        assert!(f.is_excluded("Makefile", false));
        assert!(f.is_excluded("sub/Makefile", false));
        assert!(!f.is_excluded("GNUmakefile", false));
    }
}
