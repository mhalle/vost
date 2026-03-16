use clap::Args;

use crate::fs::Fs;
use crate::store::GitStore;
use crate::types::OpenOptions;

use super::error::CliError;

// ---------------------------------------------------------------------------
// RefPath
// ---------------------------------------------------------------------------

/// Parsed `ref:path` specification.
///
/// * `ref_name == None` → local filesystem path
/// * `ref_name == Some("")` → current/default branch
/// * `ref_name == Some("xyz")` → explicit branch/tag/commit
#[derive(Debug, Clone)]
pub struct RefPath {
    pub ref_name: Option<String>,
    pub back: usize,
    pub path: String,
}

impl RefPath {
    pub fn is_repo(&self) -> bool {
        self.ref_name.is_some()
    }

    /// Parse a `ref:path` string.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        let colon = raw.find(':');
        match colon {
            None => Ok(RefPath {
                ref_name: None,
                back: 0,
                path: raw.to_string(),
            }),
            Some(0) => Ok(RefPath {
                ref_name: Some(String::new()),
                back: 0,
                path: raw[1..].to_string(),
            }),
            Some(pos) => {
                let before = &raw[..pos];
                let after = &raw[pos + 1..];

                // Windows drive letter
                if before.len() == 1
                    && before.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
                    && after.starts_with('/')
                    || after.starts_with('\\')
                {
                    return Ok(RefPath {
                        ref_name: None,
                        back: 0,
                        path: raw.to_string(),
                    });
                }

                // Slash before colon → local path
                if before.contains('/') || before.contains('\\') {
                    return Ok(RefPath {
                        ref_name: None,
                        back: 0,
                        path: raw.to_string(),
                    });
                }

                // Parse ref with ~N suffix
                let mut ref_part = before.to_string();
                let mut back = 0usize;
                if let Some(tilde_pos) = ref_part.rfind('~') {
                    let suffix = &ref_part[tilde_pos + 1..];
                    if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
                        return Err(CliError::new(format!(
                            "Invalid ancestor suffix '~{}' — must be a positive integer",
                            suffix
                        )));
                    }
                    let n: usize = suffix.parse().unwrap();
                    if n == 0 {
                        return Err(CliError::new(format!(
                            "Invalid ancestor '~0' — use '{}:{}' instead",
                            &ref_part[..tilde_pos],
                            after
                        )));
                    }
                    ref_part = ref_part[..tilde_pos].to_string();
                    back = n;
                }

                Ok(RefPath {
                    ref_name: Some(ref_part),
                    back,
                    path: after.to_string(),
                })
            }
        }
    }

    /// Parse a bare string (no `:`) as a ref rather than a local path.
    pub fn parse_bare_as_ref(raw: &str) -> Result<Self, CliError> {
        let rp = Self::parse(raw)?;
        if rp.is_repo() {
            return Ok(rp);
        }
        // No colon → treat as ref, not local path. Parse ~N.
        let mut ref_part = raw.to_string();
        let mut back = 0usize;
        if let Some(tilde_pos) = ref_part.rfind('~') {
            let suffix = &ref_part[tilde_pos + 1..];
            if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
                return Err(CliError::new(format!(
                    "Invalid ancestor suffix '~{}' — must be a positive integer",
                    suffix
                )));
            }
            let n: usize = suffix.parse().unwrap();
            if n == 0 {
                return Err(CliError::new(format!(
                    "Invalid ancestor '~0' — use '{}:' instead",
                    &ref_part[..tilde_pos]
                )));
            }
            ref_part = ref_part[..tilde_pos].to_string();
            back = n;
        }
        Ok(RefPath {
            ref_name: Some(ref_part),
            back,
            path: String::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Shared arg groups
// ---------------------------------------------------------------------------

#[derive(Args, Clone, Default, Debug)]
pub struct SnapshotArgs {
    /// Branch, tag, or commit hash to read from.
    #[arg(long = "ref")]
    pub ref_name: Option<String>,
    /// Use latest commit that changed this path.
    #[arg(long = "path")]
    pub at_path: Option<String>,
    /// Use latest commit matching this message pattern (* and ?).
    #[arg(long = "match")]
    pub match_pattern: Option<String>,
    /// Use latest commit on or before this date (ISO 8601).
    #[arg(long)]
    pub before: Option<String>,
    /// Walk back N commits.
    #[arg(long, default_value_t = 0)]
    pub back: usize,
}

#[derive(Args, Clone, Default, Debug)]
pub struct TagArgs {
    /// Create a tag at the resulting commit.
    #[arg(long)]
    pub tag: Option<String>,
    /// Overwrite tag if it already exists.
    #[arg(long)]
    pub force_tag: bool,
}

#[derive(Args, Clone, Default, Debug)]
pub struct ParentArgs {
    /// Additional parent ref (branch/tag/hash). Repeatable.
    #[arg(long = "parent")]
    pub parent_refs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Store helpers
// ---------------------------------------------------------------------------

pub fn require_repo(repo: &Option<String>) -> Result<String, CliError> {
    match repo {
        Some(s) if !s.is_empty() => Ok(s.clone()),
        _ => Err(CliError::new(
            "No repository specified. Use --repo or set VOST_REPO.",
        )),
    }
}

pub fn open_store(path: &str) -> Result<GitStore, CliError> {
    GitStore::open(
        path,
        OpenOptions {
            create: false,
            ..Default::default()
        },
    )
    .map_err(|_| CliError::new(format!("Repository not found: {}", path)))
}

pub fn open_or_create_store(path: &str, branch: &str) -> Result<GitStore, CliError> {
    GitStore::open(
        path,
        OpenOptions {
            create: true,
            branch: Some(branch.to_string()),
            ..Default::default()
        },
    )
    .map_err(CliError::from)
}

pub fn open_or_create_bare(path: &str) -> Result<GitStore, CliError> {
    GitStore::open(
        path,
        OpenOptions {
            create: true,
            ..Default::default()
        },
    )
    .map_err(CliError::from)
}

pub fn current_branch(store: &GitStore) -> String {
    store
        .branches()
        .get_current_name()
        .ok()
        .flatten()
        .unwrap_or_else(|| "main".to_string())
}

pub fn get_branch_fs(store: &GitStore, branch: &str) -> Result<Fs, CliError> {
    store
        .branches()
        .get(branch)
        .map_err(|_| CliError::new(format!("Branch not found: {}", branch)))
}

pub fn get_fs(
    store: &GitStore,
    branch: &str,
    ref_name: Option<&str>,
) -> Result<Fs, CliError> {
    if let Some(r) = ref_name {
        resolve_ref(store, r)
    } else {
        get_branch_fs(store, branch)
    }
}

pub fn resolve_ref(store: &GitStore, ref_str: &str) -> Result<Fs, CliError> {
    store.fs(ref_str).map_err(|_| {
        // Check if it looks like a hex hash and exists as a non-commit object
        let is_hex = !ref_str.is_empty()
            && ref_str.len() >= 4
            && ref_str.chars().all(|c| c.is_ascii_hexdigit());
        if is_hex {
            // Try to see if the object exists but is not a commit
            let repo = store.inner.repo.lock().ok();
            if let Some(repo) = repo {
                let found = if ref_str.len() < 40 {
                    repo.revparse_single(ref_str).ok()
                } else {
                    git2::Oid::from_str(ref_str)
                        .ok()
                        .and_then(|oid| repo.find_object(oid, None).ok())
                };
                if let Some(obj) = found {
                    if obj.kind() != Some(git2::ObjectType::Commit) {
                        let kind = obj
                            .kind()
                            .map(|k| format!("{:?}", k).to_lowercase())
                            .unwrap_or_else(|| "unknown".to_string());
                        return CliError::new(format!(
                            "{} is a {}, not a commit",
                            ref_str, kind
                        ));
                    }
                }
            }
        }
        CliError::new(format!("Unknown ref: {}", ref_str))
    })
}

pub fn resolve_fs(
    store: &GitStore,
    branch: &str,
    snap: &SnapshotArgs,
) -> Result<Fs, CliError> {
    let mut fs = get_fs(store, branch, snap.ref_name.as_deref())?;
    fs = apply_snapshot_filters(fs, snap)?;
    Ok(fs)
}

pub fn apply_snapshot_filters(
    mut fs: Fs,
    snap: &SnapshotArgs,
) -> Result<Fs, CliError> {
    let at_path = snap.at_path.as_deref().map(normalize_repo_path).transpose()?;
    let before = parse_before(snap.before.as_deref())?;

    if at_path.is_some() || snap.match_pattern.is_some() || before.is_some() {
        use crate::fs::LogOptions;
        let entries = fs.log(LogOptions {
            path: at_path.clone(),
            match_pattern: snap.match_pattern.clone(),
            before,
            ..Default::default()
        })?;
        if entries.is_empty() {
            return Err(CliError::new("No matching commits found"));
        }
        // Reconstruct Fs from first matching commit
        let hash = &entries[0].commit_hash;
        fs = fs.at_commit(hash).map_err(CliError::from)?;
    }

    if snap.back > 0 {
        fs = fs
            .back(snap.back)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("not enough") || msg.contains("no parent") {
                    CliError::new("history too short")
                } else {
                    CliError::new(msg)
                }
            })?;
    }

    Ok(fs)
}

pub fn resolve_ref_path(
    store: &GitStore,
    rp: &RefPath,
    default_ref: Option<&str>,
    default_branch: &str,
    snap: &SnapshotArgs,
) -> Result<Fs, CliError> {
    let mut fs = if rp.ref_name.as_deref() == Some("") {
        get_fs(store, default_branch, default_ref)?
    } else {
        resolve_ref(store, rp.ref_name.as_deref().unwrap_or(default_branch))?
    };
    if rp.back > 0 {
        fs = fs
            .back(rp.back)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("not enough") || msg.contains("no parent") {
                    CliError::new("history too short")
                } else {
                    CliError::new(msg)
                }
            })?;
    }
    fs = apply_snapshot_filters(fs, snap)?;
    Ok(fs)
}

pub fn require_writable_ref(
    store: &GitStore,
    rp: &RefPath,
    default_branch: &str,
) -> Result<(Fs, String), CliError> {
    if rp.back > 0 {
        return Err(CliError::new(
            "Cannot write to a historical commit (remove ~N from destination)",
        ));
    }
    let branch = match rp.ref_name.as_deref() {
        Some("") => default_branch.to_string(),
        Some(r) => {
            if store.branches().has(r).unwrap_or(false) {
                r.to_string()
            } else if store.tags().has(r).unwrap_or(false) {
                return Err(CliError::new(format!(
                    "Cannot write to tag '{}' — use a branch",
                    r
                )));
            } else {
                return Err(CliError::new(format!("Branch not found: {}", r)));
            }
        }
        None => default_branch.to_string(),
    };
    let fs = get_branch_fs(store, &branch)?;
    Ok((fs, branch))
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn normalize_repo_path(path: &str) -> Result<String, CliError> {
    if path.is_empty() {
        return Err(CliError::new("Repo path must not be empty"));
    }
    crate::paths::normalize_path(path)
        .map_err(|e| CliError::new(format!("Invalid repo path: {}", e)))
}

pub fn strip_colon(raw: &str) -> String {
    if let Ok(rp) = RefPath::parse(raw) {
        rp.path
    } else {
        raw.to_string()
    }
}

pub fn clean_archive_path(raw: &str) -> Result<String, CliError> {
    let mut s = raw;
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    normalize_repo_path(s)
}

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

pub fn parse_before(value: Option<&str>) -> Result<Option<u64>, CliError> {
    match value {
        None => Ok(None),
        Some(s) => {
            use chrono::prelude::*;
            // Try parsing as datetime first, then as date
            if let Ok(dt) = s.parse::<DateTime<FixedOffset>>() {
                return Ok(Some(dt.timestamp() as u64));
            }
            if let Ok(dt) = s.parse::<DateTime<Utc>>() {
                return Ok(Some(dt.timestamp() as u64));
            }
            if let Ok(ndt) = s.parse::<NaiveDateTime>() {
                return Ok(Some(ndt.and_utc().timestamp() as u64));
            }
            if let Ok(nd) = s.parse::<NaiveDate>() {
                let dt = nd
                    .and_hms_opt(23, 59, 59)
                    .unwrap()
                    .and_utc();
                return Ok(Some(dt.timestamp() as u64));
            }
            Err(CliError::new(format!(
                "Invalid date: {} (use ISO 8601, e.g. 2024-01-15 or 2024-01-15T14:30:00)",
                s
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Archive format detection
// ---------------------------------------------------------------------------

pub fn detect_archive_format(filename: &str) -> Result<String, CliError> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        return Ok("zip".to_string());
    }
    for ext in &[
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz",
    ] {
        if lower.ends_with(ext) {
            return Ok("tar".to_string());
        }
    }
    Err(CliError::new(format!(
        "Cannot detect archive format from extension: {}\nUse --format zip or --format tar",
        filename
    )))
}

// ---------------------------------------------------------------------------
// Tag helper
// ---------------------------------------------------------------------------

pub fn apply_tag(
    store: &GitStore,
    fs: &Fs,
    tag: &str,
    force: bool,
) -> Result<(), CliError> {
    if force && store.tags().has(tag).unwrap_or(false) {
        store.tags().delete(tag).map_err(CliError::from)?;
    }
    store
        .tags()
        .set(tag, fs)
        .map_err(|_| CliError::new(format!("Tag already exists: {} (use --force-tag to overwrite)", tag)))
}

// ---------------------------------------------------------------------------
// Glob expansion
// ---------------------------------------------------------------------------

pub fn expand_sources_repo(fs: &Fs, sources: &[String]) -> Result<Vec<String>, CliError> {
    let mut result = Vec::new();
    for src in sources {
        if src.contains('*') || src.contains('?') {
            let expanded = fs.glob(src).map_err(CliError::from)?;
            if expanded.is_empty() {
                return Err(CliError::new(format!(
                    "No matches for pattern in repo: {}",
                    src
                )));
            }
            result.extend(expanded);
        } else {
            result.push(src.clone());
        }
    }
    Ok(result)
}

pub fn expand_sources_disk(sources: &[String]) -> Result<Vec<String>, CliError> {
    let mut result = Vec::new();
    for src in sources {
        if src.contains('*') || src.contains('?') {
            let expanded = disk_glob(src)?;
            if expanded.is_empty() {
                return Err(CliError::new(format!("No matches for pattern: {}", src)));
            }
            result.extend(expanded);
        } else {
            result.push(src.clone());
        }
    }
    Ok(result)
}

/// Expand a glob pattern against the local filesystem.
///
/// Same dotfile rules as the repo-side `fs.glob()`: wildcards do not match
/// names starting with `.` unless the pattern segment itself starts with `.`.
///
/// Supports `*`, `?`, and `**` (recursive descent).  The `/./ ` rsync-style
/// pivot marker is preserved in the output paths — it is not interpreted here
/// (the library handles it at copy/sync time).
///
/// Returns a sorted list of matching paths.
pub fn disk_glob(pattern: &str) -> Result<Vec<String>, CliError> {
    let pattern = pattern.trim_end_matches('/');
    if pattern.is_empty() {
        return Ok(Vec::new());
    }

    // Normalise separators to `/` for splitting.
    let pattern = pattern.replace('\\', "/");

    let (segments, prefix): (Vec<&str>, String) = if pattern.starts_with('/') {
        let rest = pattern.trim_start_matches('/');
        if rest.is_empty() {
            (Vec::new(), "/".to_string())
        } else {
            (rest.split('/').collect(), "/".to_string())
        }
    } else {
        (pattern.split('/').collect(), String::new())
    };

    if segments.is_empty() {
        return Ok(Vec::new());
    }

    let mut results = disk_glob_walk(&segments, &prefix);
    results.sort();
    Ok(results)
}

/// Recursive segment-by-segment glob walker.
fn disk_glob_walk(segments: &[&str], prefix: &str) -> Vec<String> {
    if segments.is_empty() {
        return Vec::new();
    }

    let seg = segments[0];
    let rest = &segments[1..];
    let scan_dir = if prefix.is_empty() { "." } else { prefix };

    // `**` — match zero or more directory levels, skipping dotfiles.
    if seg == "**" {
        let entries = match std::fs::read_dir(scan_dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };
        let mut names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }

        let mut results = Vec::new();

        // Zero dirs matched: try rest at this level.
        if !rest.is_empty() {
            results.extend(disk_glob_walk(rest, prefix));
        } else {
            // Terminal **: collect all non-dot entries at this level.
            for name in &names {
                if name.starts_with('.') {
                    continue;
                }
                let full = join_path(prefix, name);
                results.push(full);
            }
        }

        // One+ dirs: recurse into non-dot subdirs (keep ** segment).
        for name in &names {
            if name.starts_with('.') {
                continue;
            }
            let full = join_path(prefix, name);
            if std::path::Path::new(&full).is_dir() {
                results.extend(disk_glob_walk(segments, &full));
            }
        }
        return results;
    }

    let has_wild = seg.contains('*') || seg.contains('?');

    if has_wild {
        let entries = match std::fs::read_dir(scan_dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };
        let mut results = Vec::new();
        for entry in entries.flatten() {
            let name = match entry.file_name().to_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !crate::glob::glob_match(seg, &name) {
                continue;
            }
            let full = join_path(prefix, &name);
            if rest.is_empty() {
                results.push(full);
            } else {
                results.extend(disk_glob_walk(rest, &full));
            }
        }
        results
    } else {
        // Literal segment — just descend.
        let full = join_path(prefix, seg);
        if rest.is_empty() {
            if std::path::Path::new(&full).exists() {
                vec![full]
            } else {
                Vec::new()
            }
        } else {
            disk_glob_walk(rest, &full)
        }
    }
}

/// Join a prefix and name with `/`, handling the empty-prefix case.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if prefix.ends_with('/') {
        format!("{}{}", prefix, name)
    } else {
        format!("{}/{}", prefix, name)
    }
}

// ---------------------------------------------------------------------------
// Resolve parents
// ---------------------------------------------------------------------------

pub fn resolve_parents(
    store: &GitStore,
    parent_refs: &[String],
) -> Result<Vec<Fs>, CliError> {
    parent_refs
        .iter()
        .map(|r| resolve_ref(store, r))
        .collect()
}

// ---------------------------------------------------------------------------
// Resolve same branch (for rm/mv)
// ---------------------------------------------------------------------------

pub fn resolve_same_branch(
    store: &GitStore,
    parsed: &[RefPath],
    default_branch: &str,
    operation: &str,
) -> Result<String, CliError> {
    let mut explicit_ref: Option<String> = None;
    for rp in parsed {
        if let Some(ref r) = rp.ref_name {
            if r.is_empty() {
                continue;
            }
            if !store.branches().has(r).unwrap_or(false) {
                if store.tags().has(r).unwrap_or(false) {
                    return Err(CliError::new(format!(
                        "Cannot {} in tag '{}' — use a branch",
                        operation, r
                    )));
                }
                return Err(CliError::new(format!("Branch not found: {}", r)));
            }
            if let Some(ref prev) = explicit_ref {
                if prev != r {
                    return Err(CliError::new(
                        "All paths must target the same branch",
                    ));
                }
            }
            explicit_ref = Some(r.clone());
        }
    }
    Ok(explicit_ref.unwrap_or_else(|| default_branch.to_string()))
}

// ---------------------------------------------------------------------------
// Check ref conflicts
// ---------------------------------------------------------------------------

pub fn check_ref_conflicts(
    parsed: &[&RefPath],
    ref_name: Option<&str>,
    branch: Option<&str>,
    back: usize,
) -> Result<(), CliError> {
    let repo_paths: Vec<_> = parsed.iter().filter(|rp| rp.is_repo()).collect();
    let explicit_refs: Vec<_> = repo_paths
        .iter()
        .filter(|rp| {
            rp.ref_name
                .as_ref()
                .map_or(false, |s| !s.is_empty())
        })
        .collect();
    let tilde_paths: Vec<_> = repo_paths.iter().filter(|rp| rp.back > 0).collect();

    if !explicit_refs.is_empty() {
        if ref_name.is_some() {
            return Err(CliError::new(
                "Cannot use --ref with explicit ref: in path",
            ));
        }
        if branch.is_some() {
            return Err(CliError::new(
                "Cannot use -b/--branch with explicit ref: in path",
            ));
        }
    }
    if !tilde_paths.is_empty() && back > 0 {
        return Err(CliError::new("Cannot use --back with ~N in path"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Status output
// ---------------------------------------------------------------------------

pub fn status(verbose: bool, msg: &str) {
    if verbose {
        eprintln!("{}", msg);
    }
}

// ---------------------------------------------------------------------------
// Message placeholder expansion
// ---------------------------------------------------------------------------

pub fn expand_message(
    msg: &str,
    report: Option<&crate::types::ChangeReport>,
    op: &str,
) -> String {
    let (add, update, delete, total) = match report {
        Some(r) => (r.add.len(), r.update.len(), r.delete.len(), r.total()),
        None => (0, 0, 0, 0),
    };
    // Generate default message from the report
    let default_msg = if let Some(r) = report {
        let actions = r.actions();
        if actions.len() == 1 {
            let a = &actions[0];
            let prefix = match a.kind {
                crate::types::ChangeActionKind::Add => "+",
                crate::types::ChangeActionKind::Update => "~",
                crate::types::ChangeActionKind::Delete => "-",
            };
            format!("{} {}", prefix, a.path)
        } else {
            format!("{} file(s) changed", total)
        }
    } else {
        String::new()
    };

    msg.replace("{default}", &default_msg)
        .replace("{add_count}", &add.to_string())
        .replace("{update_count}", &update.to_string())
        .replace("{delete_count}", &delete.to_string())
        .replace("{total_count}", &total.to_string())
        .replace("{op}", op)
}
