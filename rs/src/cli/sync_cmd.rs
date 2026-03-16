use clap::Args;

use crate::fs::{CopyFromRefOptions, SyncOptions};
use crate::types::ChangeActionKind;

use super::error::CliError;
use super::helpers::*;

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Source and destination paths.
    #[arg(required = true)]
    pub args: Vec<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    /// Show what would change without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Exclude files matching pattern.
    #[arg(long)]
    pub exclude: Vec<String>,
    /// Read exclude patterns from file.
    #[arg(long)]
    pub exclude_from: Option<String>,
    /// Read .gitignore files from source tree.
    #[arg(long)]
    pub gitignore: bool,
    /// Skip files that fail and continue.
    #[arg(long)]
    pub ignore_errors: bool,
    /// Compare by checksum instead of mtime.
    #[arg(short = 'c', long)]
    pub checksum: bool,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    /// Watch for changes (deferred — not yet implemented).
    #[arg(long)]
    pub watch: bool,
    /// Debounce delay in ms for --watch.
    #[arg(long, default_value_t = 2000)]
    pub debounce: u64,
    #[command(flatten)]
    pub tag_args: TagArgs,
    #[command(flatten)]
    pub parent_args: ParentArgs,
}

pub fn cmd_sync(
    repo_opt: &Option<String>,
    args: &SyncArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let (direction, local_path, repo_dest, src_rp, dest_rp) = if args.args.len() == 1 {
        let rp = RefPath::parse(&args.args[0])?;
        if rp.is_repo() {
            return Err(CliError::new(
                "Single-argument sync must be a local path, not a repo path",
            ));
        }
        ("to_repo", args.args[0].clone(), String::new(), None, None)
    } else if args.args.len() == 2 {
        let s = RefPath::parse(&args.args[0])?;
        let d = RefPath::parse(&args.args[1])?;
        if s.is_repo() && d.is_repo() {
            (
                "repo_to_repo",
                String::new(),
                String::new(),
                Some(s),
                Some(d),
            )
        } else if !s.is_repo() && !d.is_repo() {
            return Err(CliError::new(
                "Neither argument is a repo path — prefix repo paths with ':'",
            ));
        } else if !s.is_repo() {
            let repo_dest = d.path.trim_end_matches('/').to_string();
            ("to_repo", args.args[0].clone(), repo_dest, None, None)
        } else {
            let repo_src = s.path.trim_end_matches('/').to_string();
            (
                "from_repo",
                args.args[1].clone(),
                repo_src,
                Some(s),
                None,
            )
        }
    } else {
        return Err(CliError::new("sync requires 1 or 2 arguments"));
    };

    // Validate --tag
    if args.tag_args.tag.is_some() && direction != "to_repo" {
        return Err(CliError::new(
            "--tag only applies when writing to repo (disk → repo)",
        ));
    }

    // Validate --watch
    if args.watch {
        if args.dry_run {
            return Err(CliError::new("--watch and --dry-run are incompatible"));
        }
        if direction != "to_repo" {
            return Err(CliError::new("--watch only supports disk → repo"));
        }
        if args.debounce < 100 {
            return Err(CliError::new("--debounce must be at least 100 ms"));
        }
        return Err(CliError::new(
            "--watch is not yet implemented in the Rust CLI",
        ));
    }

    // Validate --gitignore direction
    if args.gitignore && direction != "to_repo" {
        return Err(CliError::new(
            "--gitignore only applies when syncing from disk to repo",
        ));
    }

    let repo_path = require_repo(repo_opt)?;

    // Build exclude filter
    let excl = if !args.exclude.is_empty() || args.exclude_from.is_some() || args.gitignore {
        if direction != "to_repo" {
            return Err(CliError::new(
                "--exclude/--exclude-from only apply when syncing from disk to repo",
            ));
        }
        let mut ef = crate::ExcludeFilter::new();
        if args.gitignore {
            ef.gitignore = true;
        }
        {
            let pats: Vec<&str> = args.exclude.iter().map(|s| s.as_str()).collect();
            ef.add_patterns(&pats);
        }
        if let Some(ref path) = args.exclude_from {
            let content = std::fs::read_to_string(path)?;
            let lines: Vec<&str> = content.lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            ef.add_patterns(&lines);
        }
        Some(ef)
    } else {
        None
    };

    match direction {
        "to_repo" => {
            let store = if !args.dry_run && !args.no_create {
                open_or_create_store(
                    &repo_path,
                    args.branch.as_deref().unwrap_or("main"),
                )?
            } else {
                open_store(&repo_path)?
            };
            let branch = args
                .branch
                .clone()
                .unwrap_or_else(|| current_branch(&store));
            let fs = resolve_fs(&store, &branch, &args.snap)?;

            let parents = if args.parent_args.parent_refs.is_empty() {
                Vec::new()
            } else {
                resolve_parents(&store, &args.parent_args.parent_refs)?
            };

            let opts = SyncOptions {
                exclude_filter: excl,
                message: args.message.clone(),
                dry_run: args.dry_run,
                checksum: args.checksum,
                ignore_errors: args.ignore_errors,
                parents,
                ..Default::default()
            };

            let (report, result_fs) = fs
                .sync_in(&local_path, &repo_dest, opts)
                .map_err(CliError::from)?;

            if args.dry_run {
                for action in report.actions() {
                    let prefix = match action.kind {
                        ChangeActionKind::Add => "+",
                        ChangeActionKind::Update => "~",
                        ChangeActionKind::Delete => "-",
                    };
                    if !repo_dest.is_empty() && !action.path.is_empty() {
                        println!("{} :{}/{}", prefix, repo_dest, action.path);
                    } else {
                        println!("{} :{}{}", prefix, repo_dest, action.path);
                    }
                }
            } else {
                if let Some(ref tag) = args.tag_args.tag {
                    apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
                }
                status(
                    verbose,
                    &format!(
                        "Synced -> :{}",
                        if repo_dest.is_empty() { "/" } else { &repo_dest }
                    ),
                );
            }
        }

        "from_repo" => {
            let store = open_store(&repo_path)?;
            let branch = args
                .branch
                .clone()
                .unwrap_or_else(|| current_branch(&store));

            let fs = if let Some(ref rp) = src_rp {
                if rp.ref_name.as_deref().map_or(false, |s| !s.is_empty()) || rp.back > 0 {
                    resolve_ref_path(
                        &store,
                        rp,
                        args.snap.ref_name.as_deref(),
                        &branch,
                        &args.snap,
                    )?
                } else {
                    resolve_fs(&store, &branch, &args.snap)?
                }
            } else {
                resolve_fs(&store, &branch, &args.snap)?
            };

            let opts = SyncOptions {
                dry_run: args.dry_run,
                checksum: args.checksum,
                ..Default::default()
            };

            let report = fs
                .sync_out(&repo_dest, &local_path, opts)
                .map_err(CliError::from)?;

            if args.dry_run {
                for action in report.actions() {
                    let prefix = match action.kind {
                        ChangeActionKind::Add => "+",
                        ChangeActionKind::Update => "~",
                        ChangeActionKind::Delete => "-",
                    };
                    println!(
                        "{} {}",
                        prefix,
                        std::path::Path::new(&local_path)
                            .join(&action.path)
                            .display()
                    );
                }
            } else {
                status(verbose, &format!("Synced -> {}", local_path));
            }
        }

        "repo_to_repo" => {
            let store = open_store(&repo_path)?;
            let branch = args
                .branch
                .clone()
                .unwrap_or_else(|| current_branch(&store));

            let src_rp = src_rp.unwrap();
            let dest_rp = dest_rp.unwrap();

            let src_fs = if src_rp.ref_name.as_deref().map_or(false, |s| !s.is_empty())
                || src_rp.back > 0
            {
                resolve_ref_path(
                    &store,
                    &src_rp,
                    args.snap.ref_name.as_deref(),
                    &branch,
                    &args.snap,
                )?
            } else {
                resolve_fs(&store, &branch, &args.snap)?
            };

            let (dest_fs, _dest_branch) =
                require_writable_ref(&store, &dest_rp, &branch)?;

            let src_repo_path = src_rp.path.trim_end_matches('/');
            let dest_repo_path = dest_rp.path.trim_end_matches('/');

            let parents = if args.parent_args.parent_refs.is_empty() {
                Vec::new()
            } else {
                resolve_parents(&store, &args.parent_args.parent_refs)?
            };

            // Use copy_from_ref with delete=true for sync semantics
            let opts = CopyFromRefOptions {
                delete: true,
                dry_run: args.dry_run,
                message: args.message.clone(),
                parents,
            };

            let result_fs = dest_fs
                .copy_from_ref(&src_fs, &[src_repo_path], dest_repo_path, opts)
                .map_err(CliError::from)?;

            if args.dry_run {
                if let Some(changes) = result_fs.changes() {
                    for action in changes.actions() {
                        let prefix = match action.kind {
                            ChangeActionKind::Add => "+",
                            ChangeActionKind::Update => "~",
                            ChangeActionKind::Delete => "-",
                        };
                        println!("{} :{}", prefix, action.path);
                    }
                }
            } else {
                status(
                    verbose,
                    &format!(
                        "Synced -> :{}",
                        if dest_repo_path.is_empty() {
                            "/"
                        } else {
                            dest_repo_path
                        }
                    ),
                );
            }
        }

        _ => unreachable!(),
    }

    Ok(())
}
