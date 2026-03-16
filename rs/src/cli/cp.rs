use clap::Args;

use crate::fs::{BatchOptions, CopyInOptions, CopyOutOptions};
use crate::types::ChangeActionKind;

use super::error::CliError;
use super::helpers::*;

#[derive(Args, Debug)]
pub struct CpArgs {
    /// Source and destination paths (last is dest).
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
    /// File type override.
    #[arg(long = "type", value_parser = ["blob", "executable"])]
    pub file_type: Option<String>,
    /// Follow symlinks instead of preserving them.
    #[arg(long)]
    pub follow_symlinks: bool,
    /// Show what would change without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Skip files that already exist at destination.
    #[arg(long)]
    pub ignore_existing: bool,
    /// Delete dest files not in source (excluded files are preserved).
    #[arg(long)]
    pub delete: bool,
    /// Exclude files matching pattern.
    #[arg(long)]
    pub exclude: Vec<String>,
    /// Read exclude patterns from file.
    #[arg(long)]
    pub exclude_from: Option<String>,
    /// Skip files that fail and continue.
    #[arg(long)]
    pub ignore_errors: bool,
    /// Compare by checksum instead of mtime.
    #[arg(short = 'c', long)]
    pub checksum: bool,
    /// Deprecated file mode override (644 or 755).
    #[arg(long, hide = true, value_parser = ["644", "755"])]
    pub mode: Option<String>,
    /// Treat paths as literal (no glob expansion).
    #[arg(long)]
    pub no_glob: bool,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    #[command(flatten)]
    pub tag_args: TagArgs,
    #[command(flatten)]
    pub parent_args: ParentArgs,
}

pub fn cmd_cp(
    repo_opt: &Option<String>,
    args: &CpArgs,
    verbose: bool,
) -> Result<(), CliError> {
    if args.args.len() < 2 {
        return Err(CliError::new(
            "cp requires at least two arguments (SRC... DEST)",
        ));
    }

    let raw_sources = &args.args[..args.args.len() - 1];
    let raw_dest = &args.args[args.args.len() - 1];

    let parsed_sources: Vec<RefPath> = raw_sources
        .iter()
        .map(|s| RefPath::parse(s))
        .collect::<Result<_, _>>()?;
    let parsed_dest = RefPath::parse(raw_dest)?;

    let all_src_local = parsed_sources.iter().all(|rp| !rp.is_repo());
    let all_src_repo = parsed_sources.iter().all(|rp| rp.is_repo());
    let dest_is_repo = parsed_dest.is_repo();

    // Determine direction
    let direction = if all_src_local && !dest_is_repo {
        return Err(CliError::new(
            "Neither sources nor DEST is a repo path — prefix repo paths with ':'",
        ));
    } else if all_src_local && dest_is_repo {
        "disk_to_repo"
    } else if all_src_repo && !dest_is_repo {
        "repo_to_disk"
    } else if all_src_repo && dest_is_repo {
        "repo_to_repo"
    } else {
        return Err(CliError::new(
            "Mixed local and repo sources are not supported — use separate cp commands",
        ));
    };

    // Validate --ref is not used for disk→repo copies
    if direction == "disk_to_repo" && args.snap.ref_name.is_some() {
        return Err(CliError::new(
            "snapshot filters only apply when reading from repo",
        ));
    }

    let repo_path = require_repo(repo_opt)?;

    // Build exclude filter
    let excl = if !args.exclude.is_empty() || args.exclude_from.is_some() {
        if direction != "disk_to_repo" {
            return Err(CliError::new(
                "--exclude/--exclude-from only apply when copying from disk to repo",
            ));
        }
        let mut ef = crate::ExcludeFilter::new();
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

    // Validate --tag is only for disk→repo
    if args.tag_args.tag.is_some() && direction != "disk_to_repo" {
        return Err(CliError::new(
            "--tag only applies when writing to repo (disk -> repo)",
        ));
    }

    let single_file_src = raw_sources.len() == 1
        && !raw_sources[0].contains('*')
        && !raw_sources[0].contains('?')
        && !raw_sources[0].ends_with('/')
        && !raw_sources[0].contains("/./");

    match direction {
        "disk_to_repo" => {
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

            let (fs, _branch) = if parsed_dest
                .ref_name
                .as_deref()
                .map_or(false, |s| !s.is_empty())
            {
                require_writable_ref(&store, &parsed_dest, &branch)?
            } else {
                (resolve_fs(&store, &branch, &args.snap)?, branch.clone())
            };

            let dest_path = {
                let p = parsed_dest.path.trim_end_matches('/');
                if p.is_empty() {
                    String::new()
                } else {
                    normalize_repo_path(p)?
                }
            };

            let source_paths_raw: Vec<String> = if !args.no_glob {
                match expand_sources_disk(&raw_sources.iter().map(|s| s.to_string()).collect::<Vec<_>>()) {
                    Ok(v) => v,
                    Err(e) if args.ignore_errors => {
                        eprintln!("ERROR: {}", e.message);
                        return Err(CliError::with_code(
                            "Some files could not be copied",
                            1,
                        ));
                    }
                    Err(e) => return Err(e),
                }
            } else {
                raw_sources.iter().map(|s| s.to_string()).collect()
            };
            // When --ignore-errors, filter out nonexistent sources and report them
            let mut had_errors = false;
            let source_paths: Vec<String> = if args.ignore_errors {
                source_paths_raw
                    .into_iter()
                    .filter(|p| {
                        let path = std::path::Path::new(p);
                        if path.exists() || path.is_symlink() {
                            true
                        } else {
                            eprintln!("ERROR: {}: No such file or directory", p);
                            had_errors = true;
                            false
                        }
                    })
                    .collect()
            } else {
                source_paths_raw
            };

            let parents = if args.parent_args.parent_refs.is_empty() {
                Vec::new()
            } else {
                resolve_parents(&store, &args.parent_args.parent_refs)?
            };

            // Check if single file source
            let is_single_file = single_file_src
                && std::path::Path::new(&source_paths[0]).is_file();

            if is_single_file && args.delete {
                return Err(CliError::new(
                    "Cannot use --delete with a single file source",
                ));
            }

            if is_single_file {
                // Single file: use batch write
                let local = std::path::Path::new(&source_paths[0]);
                let repo_file = if !dest_path.is_empty()
                    && fs.is_dir(&dest_path).unwrap_or(false)
                {
                    normalize_repo_path(&format!(
                        "{}/{}",
                        dest_path,
                        local.file_name().unwrap().to_string_lossy()
                    ))?
                } else if !dest_path.is_empty() {
                    dest_path.clone()
                } else {
                    normalize_repo_path(
                        &local.file_name().unwrap().to_string_lossy(),
                    )?
                };

                // Check exclude filter for single file
                if let Some(ref ef) = excl {
                    let filename = local.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if ef.is_excluded(&filename, false) {
                        status(verbose, &format!("Skipped (excluded): {}", local.display()));
                        return Ok(());
                    }
                }

                if args.dry_run {
                    println!("{} -> :{}", local.display(), repo_file);
                } else {
                    use crate::fs::BatchOptions;
                    use crate::types::{MODE_BLOB, MODE_BLOB_EXEC};
                    let mut batch = fs.batch(BatchOptions {
                        message: args.message.clone(),
                        operation: Some("cp".to_string()),
                        parents,
                    });
                    if !args.follow_symlinks && local.is_symlink() {
                        let target = std::fs::read_link(local)
                            .map_err(|e| CliError::new(e.to_string()))?;
                        batch
                            .write_symlink(&repo_file, &target.to_string_lossy())
                            .map_err(CliError::from)?;
                    } else {
                        // Determine mode
                        let mode = if args.file_type.as_deref() == Some("executable")
                            || args.mode.as_deref() == Some("755")
                        {
                            Some(MODE_BLOB_EXEC)
                        } else if args.follow_symlinks && local.is_symlink() {
                            // When following symlinks, use the target's metadata
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                if let Ok(meta) = std::fs::metadata(local) {
                                    if meta.permissions().mode() & 0o111 != 0 {
                                        Some(MODE_BLOB_EXEC)
                                    } else {
                                        Some(MODE_BLOB)
                                    }
                                } else {
                                    Some(MODE_BLOB)
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                Some(MODE_BLOB)
                            }
                        } else {
                            None
                        };
                        if let Some(m) = mode {
                            let data = std::fs::read(local)?;
                            batch.write_with_mode(&repo_file, &data, m)
                                .map_err(CliError::from)?;
                        } else {
                            batch.write_from_file(&repo_file, local)
                                .map_err(CliError::from)?;
                        }
                    }
                    let result_fs = batch.commit().map_err(CliError::from)?;
                    if let Some(ref tag) = args.tag_args.tag {
                        apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
                    }
                    status(verbose, &format!("Copied -> :{}", repo_file));
                }
            } else if args.delete {
                // --delete: use sync_in for delete semantics
                // sync_in expects a single source directory
                if source_paths.len() != 1 {
                    return Err(CliError::new(
                        "--delete with multiple sources is not supported; use a single source directory",
                    ));
                }
                use crate::fs::SyncOptions;
                let opts = SyncOptions {
                    exclude_filter: excl,
                    message: args.message.clone(),
                    dry_run: args.dry_run,
                    checksum: args.checksum,
                    parents,
                    ..Default::default()
                };

                let (report, result_fs) = fs
                    .sync_in(&source_paths[0], &dest_path, opts)
                    .map_err(CliError::from)?;

                if args.dry_run {
                    for action in report.actions() {
                        let prefix = match action.kind {
                            ChangeActionKind::Add => "+",
                            ChangeActionKind::Update => "~",
                            ChangeActionKind::Delete => "-",
                        };
                        println!("{} :{}", prefix, action.path);
                    }
                } else {
                    if let Some(ref tag) = args.tag_args.tag {
                        apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
                    }
                    status(
                        verbose,
                        &format!(
                            "Copied -> :{}",
                            if dest_path.is_empty() { "/" } else { &dest_path }
                        ),
                    );
                }
            } else {
                // Directory/multi-file: use copy_in
                let src_strs: Vec<&str> = source_paths.iter().map(|s| s.as_str()).collect();
                let opts = CopyInOptions {
                    exclude_filter: excl,
                    message: args.message.clone(),
                    dry_run: args.dry_run,
                    checksum: args.checksum,
                    follow_symlinks: args.follow_symlinks,
                    parents,
                    ..Default::default()
                };

                let (report, result_fs) = fs
                    .copy_in(&src_strs, &dest_path, opts)
                    .map_err(CliError::from)?;

                if args.dry_run {
                    for action in report.actions() {
                        let prefix = match action.kind {
                            ChangeActionKind::Add => "+",
                            ChangeActionKind::Update => "~",
                            ChangeActionKind::Delete => "-",
                        };
                        println!("{} :{}", prefix, action.path);
                    }
                } else {
                    if let Some(ref tag) = args.tag_args.tag {
                        apply_tag(&store, &result_fs, tag, args.tag_args.force_tag)?;
                    }
                    status(
                        verbose,
                        &format!(
                            "Copied -> :{}",
                            if dest_path.is_empty() { "/" } else { &dest_path }
                        ),
                    );
                }
            }
            if had_errors {
                return Err(CliError::with_code(
                    "Some files could not be copied",
                    1,
                ));
            }
        }

        "repo_to_disk" => {
            let store = open_store(&repo_path)?;
            let branch = args
                .branch
                .clone()
                .unwrap_or_else(|| current_branch(&store));
            let fs = resolve_fs(&store, &branch, &args.snap)?;

            let mut source_paths: Vec<String> = parsed_sources
                .iter()
                .map(|rp| rp.path.clone())
                .collect();

            if !args.no_glob {
                source_paths = expand_sources_repo(&fs, &source_paths)?;
            }

            // Single file repo→disk
            let is_single_repo_file = single_file_src
                && !source_paths[0].is_empty()
                && !fs
                    .is_dir(&normalize_repo_path(&source_paths[0]).unwrap_or_default())
                    .unwrap_or(false);

            if is_single_repo_file {
                let src_path = normalize_repo_path(&source_paths[0])?;
                let local_dest = std::path::Path::new(raw_dest);
                let out = if local_dest.is_dir() {
                    local_dest.join(
                        std::path::Path::new(&src_path)
                            .file_name()
                            .unwrap_or_default(),
                    )
                } else {
                    local_dest.to_path_buf()
                };

                if args.dry_run {
                    println!(":{} -> {}", src_path, out.display());
                } else {
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    // Remove existing file/symlink at destination
                    if out.exists() || out.is_symlink() {
                        let _ = std::fs::remove_file(&out);
                    }
                    let ft = fs.file_type(&src_path).map_err(CliError::from)?;
                    if ft == crate::types::FileType::Link {
                        let target = fs.readlink(&src_path).map_err(CliError::from)?;
                        #[cfg(unix)]
                        std::os::unix::fs::symlink(&target, &out)?;
                        #[cfg(not(unix))]
                        std::fs::write(&out, target.as_bytes())?;
                    } else {
                        let data = fs.read(&src_path).map_err(CliError::from)?;
                        std::fs::write(&out, &data)?;
                        #[cfg(unix)]
                        if ft == crate::types::FileType::Executable {
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))?;
                        }
                    }
                    status(verbose, &format!("Copied :{} -> {}", src_path, out.display()));
                }
            } else if args.dry_run {
                // Dry run: list what would be copied without writing
                for src in &source_paths {
                    let norm = if src.is_empty() { String::new() } else {
                        normalize_repo_path(src)?
                    };
                    let is_dir = norm.is_empty() || fs.is_dir(&norm).unwrap_or(false);
                    if is_dir {
                        let walk = fs.walk(&norm).map_err(CliError::from)?;
                        for wde in walk {
                            for fe in &wde.files {
                                let full = if wde.dirpath.is_empty() {
                                    fe.name.clone()
                                } else {
                                    format!("{}/{}", wde.dirpath, fe.name)
                                };
                                println!("+ {}", full);
                            }
                        }
                    } else {
                        println!("+ {}", norm);
                    }
                }
            } else if args.delete {
                // --delete: use sync_out for delete semantics
                use crate::fs::SyncOptions;
                let src_path = if source_paths.len() == 1 {
                    normalize_repo_path(&source_paths[0]).unwrap_or_default()
                } else {
                    String::new()
                };
                let opts = SyncOptions {
                    ..Default::default()
                };
                let report = fs
                    .sync_out(&src_path, raw_dest, opts)
                    .map_err(CliError::from)?;
                for action in report.actions() {
                    let prefix = match action.kind {
                        ChangeActionKind::Add => "+",
                        ChangeActionKind::Update => "~",
                        ChangeActionKind::Delete => "-",
                    };
                    println!("{} {}", prefix, action.path);
                }
                status(verbose, &format!("Copied -> {}", raw_dest));
            } else {
                // Multi-file/directory: use copy_out
                let src_strs: Vec<&str> = source_paths.iter().map(|s| s.as_str()).collect();
                let opts = CopyOutOptions::default();
                let report = fs
                    .copy_out(&src_strs, raw_dest, opts)
                    .map_err(CliError::from)?;

                for action in report.actions() {
                    let prefix = match action.kind {
                        ChangeActionKind::Add => "+",
                        ChangeActionKind::Update => "~",
                        ChangeActionKind::Delete => "-",
                    };
                    println!("{} {}", prefix, action.path);
                }
                status(verbose, &format!("Copied -> {}", raw_dest));
            }
        }

        "repo_to_repo" => {
            let store = open_store(&repo_path)?;
            let branch = args
                .branch
                .clone()
                .unwrap_or_else(|| current_branch(&store));

            let (dest_fs, _dest_branch) =
                require_writable_ref(&store, &parsed_dest, &branch)?;

            let dest_path = {
                let p = parsed_dest.path.trim_end_matches('/');
                if p.is_empty() {
                    String::new()
                } else {
                    normalize_repo_path(p)?
                }
            };

            let source_fs = resolve_fs(&store, &branch, &args.snap)?;

            let parents = if args.parent_args.parent_refs.is_empty() {
                Vec::new()
            } else {
                resolve_parents(&store, &args.parent_args.parent_refs)?
            };

            // Use copy_from_ref for each source
            use crate::fs::CopyFromRefOptions;
            let mut all_sources: Vec<String> = Vec::new();
            for rp in &parsed_sources {
                let mut srcs = vec![rp.path.clone()];
                if !args.no_glob {
                    srcs = expand_sources_repo(&source_fs, &srcs)?;
                }
                all_sources.extend(srcs);
            }

            let opts = CopyFromRefOptions {
                delete: args.delete,
                dry_run: args.dry_run,
                message: args.message.clone(),
                parents,
            };

            let src_strs: Vec<&str> = all_sources.iter().map(|s| s.as_str()).collect();
            let result_fs = dest_fs
                .copy_from_ref(&source_fs, &src_strs, &dest_path, opts)
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
                status(verbose, &format!("Copied -> :{}", if dest_path.is_empty() { "/" } else { &dest_path }));
            }
        }

        _ => unreachable!(),
    }

    Ok(())
}

fn cmd_cp_repo_to_disk(
    fs: &crate::Fs,
    source_paths: &[String],
    dest: &str,
    args: &CpArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let src_strs: Vec<&str> = source_paths.iter().map(|s| s.as_str()).collect();
    let opts = CopyOutOptions::default();
    let report = fs
        .copy_out(&src_strs, dest, opts)
        .map_err(CliError::from)?;

    for action in report.actions() {
        let prefix = match action.kind {
            ChangeActionKind::Add => "+",
            ChangeActionKind::Update => "~",
            ChangeActionKind::Delete => "-",
        };
        println!("{} {}", prefix, action.path);
    }
    status(verbose, &format!("Copied -> {}", dest));
    Ok(())
}
