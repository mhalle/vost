use clap::Args;
use serde::Serialize;

use crate::fs::LogOptions;

use super::error::CliError;
use super::helpers::*;
use super::output::OutputFormat;

// ---------------------------------------------------------------------------
// log
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct LogArgs {
    /// Optional ref, ref:path, or :path specification.
    pub target: Option<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
    /// Output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Serialize)]
struct LogEntryDto {
    hash: String,
    message: String,
    time: String,
    author_name: String,
    author_email: String,
    branch: Option<String>,
}

pub fn cmd_log(repo_path: &str, args: &LogArgs, _verbose: bool) -> Result<(), CliError> {
    let mut ref_override = args.snap.ref_name.clone();
    let mut back = args.snap.back;
    let mut at_path = args.snap.at_path.clone();

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
            if at_path.is_some() {
                return Err(CliError::new(
                    "Cannot specify both positional path and --path",
                ));
            }
            at_path = Some(rp.path.clone());
        }
    }

    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let mut fs = get_fs(&store, &branch, ref_override.as_deref())?;
    if back > 0 {
        fs = fs
            .back(back)
            .map_err(|e| CliError::new(e.to_string()))?;
    }

    let norm_path = at_path
        .as_deref()
        .map(normalize_repo_path)
        .transpose()?;

    let before = parse_before(args.snap.before.as_deref())?;

    let entries = fs
        .log(LogOptions {
            path: norm_path,
            match_pattern: args.snap.match_pattern.clone(),
            before,
            ..Default::default()
        })
        .map_err(CliError::from)?;

    match args.format {
        OutputFormat::Json => {
            let dtos: Vec<LogEntryDto> = entries
                .iter()
                .map(|e| to_log_dto(e, &fs))
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&dtos).unwrap()
            );
        }
        OutputFormat::Jsonl => {
            for e in &entries {
                let dto = to_log_dto(e, &fs);
                println!("{}", serde_json::to_string(&dto).unwrap());
            }
        }
        OutputFormat::Text => {
            for e in &entries {
                let time_str = format_timestamp(e.time.unwrap_or(0));
                println!(
                    "{}  {}  {}",
                    &e.commit_hash[..7.min(e.commit_hash.len())],
                    time_str,
                    e.message
                );
            }
        }
    }
    Ok(())
}

fn to_log_dto(
    e: &crate::types::CommitInfo,
    fs: &crate::Fs,
) -> LogEntryDto {
    LogEntryDto {
        hash: e.commit_hash.clone(),
        message: e.message.clone(),
        time: format_timestamp(e.time.unwrap_or(0)),
        author_name: e.author_name.clone().unwrap_or_default(),
        author_email: e.author_email.clone().unwrap_or_default(),
        branch: fs.ref_name().map(|s| s.to_string()),
    }
}

fn format_timestamp(ts: u64) -> String {
    use chrono::prelude::*;
    let dt = DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap());
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// reflog
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ReflogArgs {
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Limit number of entries shown.
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,
    /// Output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Serialize)]
struct ReflogEntryDto {
    old_sha: String,
    new_sha: String,
    committer: String,
    timestamp: u64,
    time: String,
    message: String,
}

pub fn cmd_reflog(repo_path: &str, args: &ReflogArgs, _verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));

    // Verify branch exists before reading reflog
    if !store.branches().has(&branch).unwrap_or(false) {
        return Err(CliError::new(format!("Branch not found: {}", branch)));
    }

    let mut entries = store
        .branches()
        .reflog(&branch)
        .map_err(|_| CliError::new(format!("Branch not found: {}", branch)))?;

    if let Some(limit) = args.limit {
        let len = entries.len();
        if limit < len {
            entries = entries[len - limit..].to_vec();
        }
    }

    if entries.is_empty() {
        match args.format {
            OutputFormat::Json => println!("[]"),
            OutputFormat::Jsonl => {}
            OutputFormat::Text => println!("No reflog entries for branch '{}'", branch),
        }
        return Ok(());
    }

    match args.format {
        OutputFormat::Json => {
            let dtos: Vec<ReflogEntryDto> = entries.iter().map(to_reflog_dto).collect();
            println!("{}", serde_json::to_string_pretty(&dtos).unwrap());
        }
        OutputFormat::Jsonl => {
            for e in &entries {
                let dto = to_reflog_dto(e);
                println!("{}", serde_json::to_string(&dto).unwrap());
            }
        }
        OutputFormat::Text => {
            println!(
                "Reflog for branch '{}' ({} entries):\n",
                branch,
                entries.len()
            );
            for (i, e) in entries.iter().enumerate() {
                let new = &e.new_sha[..7.min(e.new_sha.len())];
                let time_str = format_timestamp(e.timestamp);
                println!("  [{}] {} ({})", i, new, time_str);
                println!("      {}", e.message);
                println!();
            }
        }
    }
    Ok(())
}

fn to_reflog_dto(e: &crate::types::ReflogEntry) -> ReflogEntryDto {
    ReflogEntryDto {
        old_sha: e.old_sha.clone(),
        new_sha: e.new_sha.clone(),
        committer: e.committer.clone(),
        timestamp: e.timestamp,
        time: format_timestamp(e.timestamp),
        message: e.message.clone(),
    }
}

// ---------------------------------------------------------------------------
// undo
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct UndoArgs {
    /// Number of steps to undo.
    #[arg(default_value_t = 1)]
    pub steps: usize,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
}

pub fn cmd_undo(repo_path: &str, args: &UndoArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = get_branch_fs(&store, &branch)?;

    let new_fs = fs
        .undo(args.steps)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("no parent") || msg.contains("not enough") {
                CliError::new(format!("Cannot undo: history too short"))
            } else {
                CliError::new(msg)
            }
        })?;

    let step_word = if args.steps == 1 { "step" } else { "steps" };
    status(
        verbose,
        &format!("Undid {} {} on '{}'", args.steps, step_word, branch),
    );
    let msg = new_fs.message().unwrap_or_default();
    let hash = new_fs
        .commit_hash()
        .unwrap_or_default();
    println!(
        "Branch now at: {} - {}",
        &hash[..7.min(hash.len())],
        msg
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// redo
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct RedoArgs {
    /// Number of steps to redo.
    #[arg(default_value_t = 1)]
    pub steps: usize,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
}

pub fn cmd_redo(repo_path: &str, args: &RedoArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = get_branch_fs(&store, &branch)?;

    let new_fs = fs
        .redo(args.steps)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("no redo") || msg.contains("not found") {
                CliError::new(format!("Cannot redo: no step available"))
            } else {
                CliError::new(msg)
            }
        })?;

    let step_word = if args.steps == 1 { "step" } else { "steps" };
    status(
        verbose,
        &format!("Redid {} {} on '{}'", args.steps, step_word, branch),
    );
    let msg = new_fs.message().unwrap_or_default();
    let hash = new_fs
        .commit_hash()
        .unwrap_or_default();
    println!(
        "Branch now at: {} - {}",
        &hash[..7.min(hash.len())],
        msg
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Baseline ref (or ref:path).
    pub baseline: Option<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
    /// Swap comparison direction.
    #[arg(long)]
    pub reverse: bool,
}

pub fn cmd_diff(repo_path: &str, args: &DiffArgs, _verbose: bool) -> Result<(), CliError> {
    let mut ref_override = args.snap.ref_name.clone();
    let mut back = args.snap.back;

    if let Some(ref baseline) = args.baseline {
        let rp = RefPath::parse_bare_as_ref(baseline)?;
        if rp.ref_name.as_deref().map_or(false, |s| !s.is_empty()) {
            if ref_override.is_some() {
                return Err(CliError::new(
                    "Cannot specify both positional ref and --ref",
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
    }

    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));

    let head_fs = get_fs(&store, &branch, None)?;
    let snap = SnapshotArgs {
        ref_name: ref_override,
        back,
        ..args.snap.clone()
    };
    let other_fs = resolve_fs(&store, &branch, &snap)?;

    if head_fs.commit_hash() == other_fs.commit_hash() {
        return Ok(());
    }

    // Walk both trees and compare
    let new_files = walk_tree_flat(&head_fs)?;
    let old_files = walk_tree_flat(&other_fs)?;

    let (new_files, old_files) = if args.reverse {
        (old_files, new_files)
    } else {
        (new_files, old_files)
    };

    let new_keys: std::collections::BTreeSet<&String> = new_files.keys().collect();
    let old_keys: std::collections::BTreeSet<&String> = old_files.keys().collect();

    for p in new_keys.difference(&old_keys) {
        println!("A  {}", p);
    }
    for p in new_keys.intersection(&old_keys) {
        if new_files[*p] != old_files[*p] {
            println!("M  {}", p);
        }
    }
    for p in old_keys.difference(&new_keys) {
        println!("D  {}", p);
    }
    Ok(())
}

fn walk_tree_flat(
    fs: &crate::Fs,
) -> Result<std::collections::BTreeMap<String, (String, u32)>, CliError> {
    let mut result = std::collections::BTreeMap::new();
    let walk = fs.walk("").map_err(CliError::from)?;
    for wde in walk {
        for fe in &wde.files {
            let path = if wde.dirpath.is_empty() {
                fe.name.clone()
            } else {
                format!("{}/{}", wde.dirpath, fe.name)
            };
            result.insert(path, (fe.oid.to_string(), fe.mode));
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// cmp
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CmpArgs {
    /// First file (repo or disk path).
    pub file1: String,
    /// Second file (repo or disk path).
    pub file2: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_cmp(
    repo_opt: &Option<String>,
    args: &CmpArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let rp1 = RefPath::parse(&args.file1)?;
    let rp2 = RefPath::parse(&args.file2)?;

    let need_store = rp1.is_repo() || rp2.is_repo();

    let (store, default_fs) = if need_store {
        let repo_path = require_repo(repo_opt)?;
        let s = open_store(&repo_path)?;
        let branch = args
            .branch
            .clone()
            .unwrap_or_else(|| current_branch(&s));
        let fs = resolve_fs(&s, &branch, &args.snap)?;
        (Some(s), Some(fs))
    } else {
        (None, None)
    };

    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| {
            store
                .as_ref()
                .map(|s| current_branch(s))
                .unwrap_or_else(|| "main".to_string())
        });

    let resolve_arg_fs = |rp: &RefPath| -> Result<Option<crate::Fs>, CliError> {
        if !rp.is_repo() {
            return Ok(None);
        }
        if rp.ref_name.as_deref().map_or(false, |s| !s.is_empty()) || rp.back > 0 {
            let s = store.as_ref().unwrap();
            Ok(Some(resolve_ref_path(
                s,
                rp,
                args.snap.ref_name.as_deref(),
                &branch,
                &args.snap,
            )?))
        } else {
            Ok(default_fs.clone())
        }
    };

    let fs1 = resolve_arg_fs(&rp1)?;
    let fs2 = resolve_arg_fs(&rp2)?;

    let hash1 = get_blob_hash(&rp1, fs1.as_ref())?;
    let hash2 = get_blob_hash(&rp2, fs2.as_ref())?;

    if verbose {
        eprintln!("{}  {}", hash1, args.file1);
        eprintln!("{}  {}", hash2, args.file2);
    }

    if hash1 == hash2 {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

fn get_blob_hash(rp: &RefPath, fs: Option<&crate::Fs>) -> Result<String, CliError> {
    if rp.is_repo() {
        let fs = fs.ok_or_else(|| CliError::new("No FS for repo path"))?;
        let path = normalize_repo_path(&rp.path)?;
        fs.object_hash(&path)
            .map_err(|e| match e {
                crate::Error::NotFound(_) => {
                    CliError::new(format!("File not found: {}", path))
                }
                crate::Error::IsADirectory(_) => {
                    CliError::new(format!("Is a directory: {}", path))
                }
                other => CliError::from(other),
            })
    } else {
        // Local file - compute git blob hash
        let path = std::path::Path::new(&rp.path);
        if !path.exists() {
            return Err(CliError::new(format!("File not found: {}", rp.path)));
        }
        if path.is_dir() {
            return Err(CliError::new(format!("Is a directory: {}", rp.path)));
        }
        let data = std::fs::read(path)?;
        crate::hash_blob(&data).map_err(CliError::from)
    }
}
