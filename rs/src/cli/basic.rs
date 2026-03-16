use std::io::{self, Read as _, Write as _};

use clap::Args;

use crate::fs::{self, WriteOptions};
use crate::store::GitStore;
use crate::types::{ChangeActionKind, OpenOptions};

use super::error::CliError;
use super::helpers::*;
use super::output::OutputFormat;

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Initial branch name (default: main).
    #[arg(short, long, default_value = "main")]
    pub branch: String,
    /// Destroy existing repo and recreate.
    #[arg(short, long)]
    pub force: bool,
}

pub fn cmd_init(repo_path: &str, args: &InitArgs, verbose: bool) -> Result<(), CliError> {
    let path = std::path::Path::new(repo_path);
    if args.force && path.exists() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        return Err(CliError::new(format!(
            "Repository already exists: {}",
            repo_path
        )));
    }
    GitStore::open(
        repo_path,
        OpenOptions {
            create: true,
            branch: Some(args.branch.clone()),
            ..Default::default()
        },
    )
    .map_err(CliError::from)?;
    status(verbose, &format!("Initialized {}", repo_path));
    Ok(())
}

// ---------------------------------------------------------------------------
// destroy
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct DestroyArgs {
    /// Required to destroy a non-empty repo.
    #[arg(short, long)]
    pub force: bool,
}

pub fn cmd_destroy(repo_path: &str, args: &DestroyArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;

    if !args.force {
        let has_tags = !store.tags().list().unwrap_or_default().is_empty();
        let has_data = has_tags
            || store
                .branches()
                .iter()
                .unwrap_or_default()
                .iter()
                .any(|(_, fs)| !fs.ls("").unwrap_or_default().is_empty());
        if has_data {
            return Err(CliError::new(
                "Repository is not empty. Use -f to destroy.",
            ));
        }
    }

    std::fs::remove_dir_all(repo_path)?;
    status(verbose, &format!("Destroyed {}", repo_path));
    Ok(())
}

// ---------------------------------------------------------------------------
// gc
// ---------------------------------------------------------------------------

pub fn cmd_gc(repo_path: &str, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let count = store.gc().map_err(CliError::from)?;
    status(verbose, &format!("gc: packed {} object(s)", count));
    Ok(())
}

// ---------------------------------------------------------------------------
// pack
// ---------------------------------------------------------------------------

pub fn cmd_pack(repo_path: &str, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let count = store.pack().map_err(CliError::from)?;
    status(verbose, &format!("pack: packed {} object(s)", count));
    Ok(())
}

// ---------------------------------------------------------------------------
// cat
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CatArgs {
    /// File paths (use : prefix for repo paths).
    #[arg(required = true)]
    pub paths: Vec<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_cat(repo_path: &str, args: &CatArgs, _verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let mut default_fs: Option<crate::Fs> = None;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for path in &args.paths {
        let rp = RefPath::parse(path)?;
        let fs = if rp.is_repo()
            && rp.ref_name.as_deref().map_or(false, |s| !s.is_empty())
            || rp.back > 0
        {
            resolve_ref_path(
                &store,
                &rp,
                args.snap.ref_name.as_deref(),
                &branch,
                &args.snap,
            )?
        } else {
            if default_fs.is_none() {
                default_fs = Some(resolve_fs(&store, &branch, &args.snap)?);
            }
            default_fs.clone().unwrap()
        };
        let repo_path = normalize_repo_path(if rp.is_repo() {
            &rp.path
        } else {
            path
        })?;
        let data = fs.read(&repo_path).map_err(|e| match e {
            crate::Error::NotFound(_) => CliError::new(format!("File not found: {}", repo_path)),
            crate::Error::IsADirectory(_) => {
                CliError::new(format!("{} is a directory, not a file", repo_path))
            }
            other => CliError::from(other),
        })?;
        out.write_all(&data)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// hash
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct HashArgs {
    /// ref, ref:path, or :path specification.
    pub target: Option<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_hash(repo_path: &str, args: &HashArgs, _verbose: bool) -> Result<(), CliError> {
    let mut ref_override = args.snap.ref_name.clone();
    let mut back = args.snap.back;
    let mut object_path: Option<String> = None;

    if let Some(ref target) = args.target {
        let rp = RefPath::parse_bare_as_ref(target)?;
        if rp.ref_name.as_deref().map_or(false, |s| !s.is_empty()) {
            if ref_override.is_some() {
                return Err(CliError::new(
                    "Cannot specify both positional ref and --ref",
                ));
            }
            if args.branch.is_some() {
                return Err(CliError::new(
                    "Cannot use -b/--branch with explicit ref in target",
                ));
            }
            ref_override = rp.ref_name.clone();
        }
        if rp.back > 0 {
            if back > 0 {
                return Err(CliError::new(
                    "Cannot specify both positional ~N and --back",
                ));
            }
            back = rp.back;
        }
        if !rp.path.is_empty() {
            object_path = Some(normalize_repo_path(&rp.path)?);
        }
    }

    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let snap = SnapshotArgs {
        ref_name: ref_override,
        back,
        ..args.snap.clone()
    };
    let fs = resolve_fs(&store, &branch, &snap)?;

    if let Some(ref p) = object_path {
        let st = fs.stat(p).map_err(|e| match e {
            crate::Error::NotFound(_) => CliError::new(format!("Path not found: {}", p)),
            other => CliError::from(other),
        })?;
        println!("{}", st.hash);
    } else {
        println!(
            "{}",
            fs.commit_hash()
                .ok_or_else(|| CliError::new("No commit in snapshot"))?
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct WriteArgs {
    /// Destination path (use : prefix for repo path).
    pub path: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    /// Echo stdin to stdout (tee mode for pipelines).
    #[arg(short = 'p', long)]
    pub passthrough: bool,
    #[command(flatten)]
    pub tag_args: TagArgs,
    #[command(flatten)]
    pub parent_args: ParentArgs,
}

pub fn cmd_write(
    repo_opt: &Option<String>,
    args: &WriteArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let rp = RefPath::parse(&args.path)?;
    let mut branch = args.branch.clone();
    if rp.is_repo() && rp.ref_name.as_deref().map_or(false, |s| !s.is_empty()) {
        branch = rp.ref_name.clone();
    }
    if rp.is_repo() && rp.back > 0 {
        return Err(CliError::new(
            "Cannot write to a historical commit (remove ~N)",
        ));
    }

    let repo_path = require_repo(repo_opt)?;

    let store = if args.no_create {
        open_store(&repo_path)?
    } else {
        open_or_create_store(&repo_path, branch.as_deref().unwrap_or("main"))?
    };
    let branch = branch.unwrap_or_else(|| current_branch(&store));

    let stripped = strip_colon(&args.path);
    let repo_path_norm = normalize_repo_path(if rp.is_repo() {
        &rp.path
    } else {
        &stripped
    })?;

    // Read stdin
    let data = if args.passthrough {
        let mut buf = Vec::new();
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut stdin_lock = stdin.lock();
        let mut stdout_lock = stdout.lock();
        let mut chunk = [0u8; 8192];
        loop {
            let n = stdin_lock.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            stdout_lock.write_all(&chunk[..n])?;
            stdout_lock.flush()?;
            buf.extend_from_slice(&chunk[..n]);
        }
        buf
    } else {
        let mut buf = Vec::new();
        io::stdin().lock().read_to_end(&mut buf)?;
        buf
    };

    let parents = if args.parent_args.parent_refs.is_empty() {
        Vec::new()
    } else {
        resolve_parents(&store, &args.parent_args.parent_refs)?
    };

    // Use retry_write
    let new_fs = fs::retry_write(|| {
        let fs = store.branches().get(&branch)
            .map_err(|_| crate::Error::key_not_found(format!("Branch not found: {}", branch)))?;
        let opts = WriteOptions {
            message: args.message.clone(),
            parents: parents.clone(),
            ..Default::default()
        };
        fs.write(&repo_path_norm, &data, opts)
    })
    .map_err(CliError::from)?;

    if let Some(ref tag) = args.tag_args.tag {
        apply_tag(&store, &new_fs, tag, args.tag_args.force_tag)?;
    }
    status(verbose, &format!("Wrote :{}", repo_path_norm));
    Ok(())
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Paths to list (or root if omitted).
    pub paths: Vec<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// List all files recursively with full paths.
    #[arg(short = 'R', long)]
    pub recursive: bool,
    /// Show file sizes, types, and hashes.
    #[arg(short, long)]
    pub long: bool,
    /// Show full 40-character object hashes.
    #[arg(long)]
    pub full_hash: bool,
    /// Output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,
    /// Treat source paths as literal (no glob expansion).
    #[arg(long)]
    pub no_glob: bool,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_ls(repo_path: &str, args: &LsArgs, _verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = resolve_fs(&store, &branch, &args.snap)?;

    let hash_len: usize = if args.full_hash { 40 } else { 7 };

    // Collect results: name -> Option<(hash, size, type_str)>
    let mut results: std::collections::BTreeMap<String, Option<LsEntry>> =
        std::collections::BTreeMap::new();

    let paths = if args.paths.is_empty() {
        vec![None]
    } else {
        args.paths.iter().map(|p| Some(p.as_str())).collect()
    };

    for path_opt in &paths {
        let (repo_path_str, fs_for_path) = if let Some(raw) = path_opt {
            let rp = RefPath::parse(raw)?;
            let actual_fs = if rp.is_repo()
                && (rp.ref_name.as_deref() != Some("") || rp.back > 0)
            {
                resolve_ref_path(
                    &store,
                    &rp,
                    args.snap.ref_name.as_deref(),
                    &branch,
                    &args.snap,
                )?
            } else {
                fs.clone()
            };
            let p = if rp.is_repo() {
                if rp.path.is_empty() { None } else { Some(rp.path.clone()) }
            } else {
                Some(raw.to_string())
            };
            (p, actual_fs)
        } else {
            (None, fs.clone())
        };

        let has_glob = !args.no_glob
            && repo_path_str
                .as_ref()
                .map_or(false, |p| p.contains('*') || p.contains('?'));

        if has_glob {
            let pattern = repo_path_str.as_deref().unwrap();
            let matches = fs_for_path.iglob(pattern).map_err(CliError::from)?;
            for m in &matches {
                if args.recursive && fs_for_path.is_dir(m).unwrap_or(false) {
                    walk_into(&fs_for_path, Some(m), args.long, &mut results)?;
                } else {
                    add_entry(&fs_for_path, m, args.long, &mut results)?;
                }
            }
            // When recursive, also find directories matching the glob and walk them
            if args.recursive {
                let dir_matches = ls_glob_dirs(&fs_for_path, pattern)?;
                for dm in &dir_matches {
                    walk_into(&fs_for_path, Some(dm), args.long, &mut results)?;
                }
            }
        } else if args.recursive {
            let norm = repo_path_str
                .as_ref()
                .map(|p| normalize_repo_path(p))
                .transpose()?;
            match walk_into(&fs_for_path, norm.as_deref(), args.long, &mut results) {
                Ok(()) => {}
                Err(e) if e.message.contains("not found") || e.message.contains("Not found") => {
                    // Single file? Check if it exists first.
                    if let Some(ref p) = norm {
                        if fs_for_path.stat(p).is_ok() {
                            add_entry(&fs_for_path, p, args.long, &mut results)?;
                        } else {
                            return Err(CliError::new(format!("not found: {}", p)));
                        }
                    } else {
                        return Err(e);
                    }
                }
                Err(e) if e.message.contains("not a directory") || e.message.contains("Not a directory") => {
                    // It's a file, list it
                    if let Some(ref p) = norm {
                        add_entry(&fs_for_path, p, args.long, &mut results)?;
                    } else {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            let norm = repo_path_str
                .as_ref()
                .map(|p| normalize_repo_path(p))
                .transpose()?;
            let path_str = norm.as_deref().unwrap_or("");
            match fs_for_path.ls(path_str) {
                Ok(names) => {
                    if args.long {
                        let entries = fs_for_path.listdir(path_str).map_err(CliError::from)?;
                        for we in entries {
                            let display = if we.file_type().map_or(false, |ft| ft.is_dir()) {
                                format!("{}/", we.name)
                            } else {
                                we.name.clone()
                            };
                            let full_path = if path_str.is_empty() {
                                we.name.clone()
                            } else {
                                format!("{}/{}", path_str, we.name)
                            };
                            let is_link = we.file_type() == Some(crate::types::FileType::Link);
                            let link_target = if is_link {
                                fs_for_path.readlink(&full_path).ok()
                            } else {
                                None
                            };
                            results.entry(display).or_insert_with(|| {
                                Some(LsEntry {
                                    hash: we.oid.to_string(),
                                    size: if we.file_type().map_or(true, |ft| !ft.is_dir()) {
                                        fs_for_path.size(&full_path).ok()
                                    } else {
                                        None
                                    },
                                    type_str: we.file_type().map(|ft| format!("{:?}", ft).to_lowercase()).unwrap_or_default(),
                                    target: link_target,
                                })
                            });
                        }
                    } else {
                        for name in names {
                            results.entry(name).or_insert(None);
                        }
                    }
                }
                Err(crate::Error::NotFound(_)) => {
                    return Err(CliError::new(format!(
                        "Path not found: {}",
                        path_str
                    )));
                }
                Err(crate::Error::NotADirectory(_)) => {
                    // It's a file, just add it
                    if let Some(ref p) = norm {
                        add_entry(&fs_for_path, p, args.long, &mut results)?;
                    }
                }
                Err(e) => return Err(CliError::from(e)),
            }
        }
    }

    // Output
    let names: Vec<&String> = results.keys().collect();
    match args.format {
        OutputFormat::Text if !args.long => {
            for name in &names {
                println!("{}", name);
            }
        }
        OutputFormat::Text => {
            let mut rows: Vec<(String, String, String)> = Vec::new();
            for name in &names {
                if let Some(Some(ref entry)) = results.get(*name) {
                    let h = &entry.hash[..hash_len.min(entry.hash.len())];
                    let size_str = entry
                        .size
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let display = if let Some(ref t) = entry.target {
                        format!("{} -> {}", name, t)
                    } else {
                        name.to_string()
                    };
                    rows.push((h.to_string(), size_str, display));
                } else {
                    rows.push((String::new(), String::new(), name.to_string()));
                }
            }
            let width = rows
                .iter()
                .map(|(_, s, _)| s.len())
                .max()
                .unwrap_or(0);
            for (hash, size, display) in &rows {
                println!("{:width_h$}  {:>width_s$}  {}", hash, size, display,
                         width_h = hash_len, width_s = width);
            }
        }
        OutputFormat::Json if !args.long => {
            println!("{}", serde_json::to_string(&names).unwrap());
        }
        OutputFormat::Json => {
            let entries: Vec<serde_json::Value> = names
                .iter()
                .map(|name| {
                    if let Some(Some(ref entry)) = results.get(*name) {
                        let mut m = serde_json::Map::new();
                        m.insert("name".into(), serde_json::Value::String(name.to_string()));
                        m.insert("hash".into(), serde_json::Value::String(entry.hash.clone()));
                        m.insert("type".into(), serde_json::Value::String(entry.type_str.clone()));
                        if let Some(sz) = entry.size {
                            m.insert("size".into(), serde_json::json!(sz));
                        }
                        if let Some(ref t) = entry.target {
                            m.insert("target".into(), serde_json::Value::String(t.clone()));
                        }
                        serde_json::Value::Object(m)
                    } else {
                        serde_json::json!({"name": name.to_string()})
                    }
                })
                .collect();
            println!("{}", serde_json::to_string(&entries).unwrap());
        }
        OutputFormat::Jsonl if !args.long => {
            for name in &names {
                println!("{}", serde_json::to_string(name).unwrap());
            }
        }
        OutputFormat::Jsonl => {
            for name in &names {
                if let Some(Some(ref entry)) = results.get(*name) {
                    let mut m = serde_json::Map::new();
                    m.insert("name".into(), serde_json::Value::String(name.to_string()));
                    m.insert("hash".into(), serde_json::Value::String(entry.hash.clone()));
                    m.insert("type".into(), serde_json::Value::String(entry.type_str.clone()));
                    if let Some(sz) = entry.size {
                        m.insert("size".into(), serde_json::json!(sz));
                    }
                    if let Some(ref t) = entry.target {
                        m.insert("target".into(), serde_json::Value::String(t.clone()));
                    }
                    println!("{}", serde_json::Value::Object(m));
                } else {
                    println!("{}", serde_json::json!({"name": name.to_string()}));
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct LsEntry {
    hash: String,
    size: Option<u64>,
    type_str: String,
    target: Option<String>,
}

fn walk_into(
    fs: &crate::Fs,
    root: Option<&str>,
    long: bool,
    results: &mut std::collections::BTreeMap<String, Option<LsEntry>>,
) -> Result<(), CliError> {
    let walk = fs.walk(root.unwrap_or("")).map_err(CliError::from)?;
    for wde in walk {
        for fe in &wde.files {
            let name = if wde.dirpath.is_empty() {
                fe.name.clone()
            } else {
                format!("{}/{}", wde.dirpath, fe.name)
            };
            if long {
                let is_link = fe.file_type() == Some(crate::types::FileType::Link);
                let link_target = if is_link {
                    fs.readlink(&name).ok()
                } else {
                    None
                };
                results.entry(name.clone()).or_insert_with(|| {
                    Some(LsEntry {
                        hash: fe.oid.to_string(),
                        size: fs.size(&name).ok(),
                        type_str: fe
                            .file_type()
                            .map(|ft| format!("{:?}", ft).to_lowercase())
                            .unwrap_or_default(),
                        target: link_target,
                    })
                });
            } else {
                results.entry(name).or_insert(None);
            }
        }
    }
    Ok(())
}

/// Find directories matching a glob pattern in the repo tree.
/// `iglob` only returns files; this companion returns directories.
fn ls_glob_dirs(fs: &crate::Fs, pattern: &str) -> Result<Vec<String>, CliError> {
    // Split into parent dir + last segment
    let parts: Vec<&str> = pattern.rsplitn(2, '/').collect();
    let (parent, seg) = if parts.len() == 2 {
        (parts[1], parts[0])
    } else {
        ("", parts[0])
    };
    let entries = fs.listdir(parent).map_err(CliError::from)?;
    let mut dirs = Vec::new();
    for we in entries {
        if we.file_type().map_or(false, |ft| ft.is_dir()) {
            if crate::glob::glob_match(seg, &we.name) {
                let full = if parent.is_empty() {
                    we.name.clone()
                } else {
                    format!("{}/{}", parent, we.name)
                };
                dirs.push(full);
            }
        }
    }
    Ok(dirs)
}

fn add_entry(
    fs: &crate::Fs,
    path: &str,
    long: bool,
    results: &mut std::collections::BTreeMap<String, Option<LsEntry>>,
) -> Result<(), CliError> {
    if long {
        if let Ok(st) = fs.stat(path) {
            let link_target = if st.file_type == crate::types::FileType::Link {
                fs.readlink(path).ok()
            } else {
                None
            };
            results.entry(path.to_string()).or_insert_with(|| {
                Some(LsEntry {
                    hash: st.hash.clone(),
                    size: if st.file_type.is_dir() {
                        None
                    } else {
                        Some(st.size)
                    },
                    type_str: format!("{:?}", st.file_type).to_lowercase(),
                    target: link_target,
                })
            });
        } else {
            results.entry(path.to_string()).or_insert(None);
        }
    } else {
        results.entry(path.to_string()).or_insert(None);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// rm
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct RmArgs {
    /// Paths to remove.
    #[arg(required = true)]
    pub paths: Vec<String>,
    /// Remove directories recursively.
    #[arg(short = 'R', long)]
    pub recursive: bool,
    /// Show what would change without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Treat paths as literal (no glob expansion).
    #[arg(long)]
    pub no_glob: bool,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    #[command(flatten)]
    pub tag_args: TagArgs,
    #[command(flatten)]
    pub parent_args: ParentArgs,
}

pub fn cmd_rm(repo_path: &str, args: &RmArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch_default = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));

    let parsed: Vec<RefPath> = args
        .paths
        .iter()
        .map(|p| RefPath::parse(p))
        .collect::<Result<_, _>>()?;
    let branch = resolve_same_branch(&store, &parsed, &branch_default, "remove")?;
    let fs = get_branch_fs(&store, &branch)?;

    let mut patterns: Vec<String> = parsed
        .iter()
        .zip(args.paths.iter())
        .map(|(rp, raw)| {
            normalize_repo_path(if rp.is_repo() { &rp.path } else { raw })
        })
        .collect::<Result<_, _>>()?;

    if !args.no_glob {
        patterns = expand_sources_repo(&fs, &patterns)?;
    }

    let parents = if args.parent_args.parent_refs.is_empty() {
        Vec::new()
    } else {
        resolve_parents(&store, &args.parent_args.parent_refs)?
    };

    use crate::fs::RemoveOptions;
    let opts = RemoveOptions {
        recursive: args.recursive,
        dry_run: args.dry_run,
        message: args.message.clone(),
        parents,
    };

    let pattern_refs: Vec<&str> = patterns.iter().map(|s| s.as_str()).collect();
    let result_fs = fs.remove(&pattern_refs, opts).map_err(|e| match e {
        crate::Error::NotFound(ref p) => CliError::new(format!("not found: {}", p)),
        crate::Error::IsADirectory(ref p) => {
            CliError::new(format!("is a directory: {} — use -R to remove recursively", p))
        }
        other => CliError::from(other),
    })?;

    if args.dry_run {
        if let Some(changes) = result_fs.changes() {
            for action in changes.actions() {
                println!("- :{}", action.path);
            }
        }
    } else {
        if let Some(ref tag) = args.tag_args.tag {
            apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
        }
        let n = result_fs
            .changes()
            .map(|c| c.delete.len())
            .unwrap_or(0);
        status(verbose, &format!("Removed {} file(s)", n));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// mv
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct MvArgs {
    /// Source and destination paths (last is dest).
    #[arg(required = true)]
    pub args: Vec<String>,
    /// Move directories recursively.
    #[arg(short = 'R', long)]
    pub recursive: bool,
    /// Show what would change without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Treat paths as literal (no glob expansion).
    #[arg(long)]
    pub no_glob: bool,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    #[command(flatten)]
    pub tag_args: TagArgs,
    #[command(flatten)]
    pub parent_args: ParentArgs,
}

pub fn cmd_mv(repo_path: &str, args: &MvArgs, verbose: bool) -> Result<(), CliError> {
    if args.args.len() < 2 {
        return Err(CliError::new(
            "mv requires at least two arguments (SRC... DEST)",
        ));
    }

    let parsed: Vec<RefPath> = args
        .args
        .iter()
        .map(|p| RefPath::parse(p))
        .collect::<Result<_, _>>()?;

    // All must be repo paths
    for (i, rp) in parsed.iter().enumerate() {
        if !rp.is_repo() {
            return Err(CliError::new(format!(
                "All paths must be repo paths (colon prefix required): {}",
                args.args[i]
            )));
        }
        if rp.back > 0 {
            return Err(CliError::new(
                "Cannot move to/from a historical commit (remove ~N)",
            ));
        }
    }

    let store = open_store(repo_path)?;
    let branch_default = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let branch = resolve_same_branch(&store, &parsed, &branch_default, "move")?;
    let fs = get_branch_fs(&store, &branch)?;

    let mut source_patterns: Vec<String> = parsed[..parsed.len() - 1]
        .iter()
        .map(|rp| {
            if rp.path.is_empty() {
                Ok(String::new())
            } else {
                normalize_repo_path(&rp.path)
            }
        })
        .collect::<Result<_, CliError>>()?;

    if !args.no_glob {
        source_patterns = expand_sources_repo(&fs, &source_patterns)?;
    }

    let dest_rp = &parsed[parsed.len() - 1];
    let mut dest_path = dest_rp.path.clone();
    if !dest_path.is_empty() {
        let trailing = dest_path.ends_with('/');
        let norm = normalize_repo_path(dest_path.trim_end_matches('/'))?;
        dest_path = if trailing {
            format!("{}/", norm)
        } else {
            norm
        };
    }

    let parents = if args.parent_args.parent_refs.is_empty() {
        Vec::new()
    } else {
        resolve_parents(&store, &args.parent_args.parent_refs)?
    };

    use crate::fs::MoveOptions;
    let opts = MoveOptions {
        recursive: args.recursive,
        dry_run: args.dry_run,
        message: args.message.clone(),
        parents,
    };

    let source_refs: Vec<&str> = source_patterns.iter().map(|s| s.as_str()).collect();
    let result_fs = fs
        .move_paths(&source_refs, &dest_path, opts)
        .map_err(|e| match e {
            crate::Error::NotFound(ref p) => CliError::new(format!("not found: {}", p)),
            crate::Error::IsADirectory(ref p) => {
                CliError::new(format!("is a directory: {} — use -R to move recursively", p))
            }
            other => CliError::from(other),
        })?;

    if args.dry_run {
        if let Some(changes) = result_fs.changes() {
            for action in changes.actions() {
                let prefix = match action.kind {
                    ChangeActionKind::Add => "+",
                    ChangeActionKind::Delete => "-",
                    _ => "~",
                };
                println!("{} :{}", prefix, action.path);
            }
        }
    } else {
        if let Some(ref tag) = args.tag_args.tag {
            apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
        }
        let changes = result_fs.changes();
        let n_add = changes.map(|c| c.add.len()).unwrap_or(0);
        let n_del = changes.map(|c| c.delete.len()).unwrap_or(0);
        status(verbose, &format!("Moved {} -> {} file(s)", n_del, n_add));
    }
    Ok(())
}
