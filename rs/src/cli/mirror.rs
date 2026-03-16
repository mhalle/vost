use clap::Args;

use crate::types::{BackupOptions, MirrorDiff, RestoreOptions};

use super::error::CliError;
use super::helpers::*;

#[derive(Args, Debug)]
pub struct BackupArgs {
    /// Destination URL, local path, or bundle file path.
    pub url: String,
    /// Show what would change without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Ref to include (repeatable). Use src:dst to rename.
    #[arg(long = "ref")]
    pub refs: Vec<String>,
    /// Force output format.
    #[arg(long, value_parser = ["bundle"])]
    pub format: Option<String>,
    /// Strip history from bundle output.
    #[arg(long)]
    pub squash: bool,
}

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// Source URL, local path, or bundle file path.
    pub url: String,
    /// Show what would change without fetching.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    /// Ref to include (repeatable). Use src:dst to rename.
    #[arg(long = "ref")]
    pub refs: Vec<String>,
    /// Force input format.
    #[arg(long, value_parser = ["bundle"])]
    pub format: Option<String>,
}

fn parse_refs(
    ref_args: &[String],
) -> Result<Option<ParsedRefs>, CliError> {
    if ref_args.is_empty() {
        return Ok(None);
    }
    let has_rename = ref_args.iter().any(|r| r.contains(':'));
    if !has_rename {
        return Ok(Some(ParsedRefs::List(
            ref_args.iter().cloned().collect(),
        )));
    }
    let mut map = std::collections::HashMap::new();
    for r in ref_args {
        if let Some((src, dst)) = r.split_once(':') {
            if src.is_empty() || dst.is_empty() {
                return Err(CliError::new(format!(
                    "Invalid ref rename '{}' — expected src:dst",
                    r
                )));
            }
            map.insert(src.to_string(), dst.to_string());
        } else {
            map.insert(r.clone(), r.clone());
        }
    }
    Ok(Some(ParsedRefs::Map(map)))
}

enum ParsedRefs {
    List(Vec<String>),
    Map(std::collections::HashMap<String, String>),
}

fn is_bundle_path(url: &str) -> bool {
    url.to_lowercase().ends_with(".bundle")
}

fn print_diff(diff: &MirrorDiff, direction: &str) {
    let verb = if direction == "push" {
        "push"
    } else {
        "pull"
    };
    if diff.in_sync() {
        println!("Nothing to {} — already in sync.", verb);
        return;
    }
    let mut changes: Vec<_> = diff.add.iter().map(|c| ("create", c)).collect();
    changes.extend(diff.update.iter().map(|c| ("update", c)));
    changes.extend(diff.delete.iter().map(|c| ("delete", c)));
    changes.sort_by(|a, b| a.1.ref_name.cmp(&b.1.ref_name));

    for (action, c) in &changes {
        match *action {
            "create" => {
                println!(
                    "  create  {}  {}",
                    c.ref_name,
                    &c.new_target.as_deref().unwrap_or("")[..7.min(c.new_target.as_deref().unwrap_or("").len())]
                );
            }
            "update" => {
                println!(
                    "  update  {}  {} -> {}",
                    c.ref_name,
                    &c.old_target.as_deref().unwrap_or("")[..7.min(c.old_target.as_deref().unwrap_or("").len())],
                    &c.new_target.as_deref().unwrap_or("")[..7.min(c.new_target.as_deref().unwrap_or("").len())]
                );
            }
            "delete" => {
                println!(
                    "  delete  {}  {}",
                    c.ref_name,
                    &c.old_target.as_deref().unwrap_or("")[..7.min(c.old_target.as_deref().unwrap_or("").len())]
                );
            }
            _ => {}
        }
    }
    println!("{} ref(s) would be changed.", diff.total());
}

pub fn cmd_backup(
    repo_path: &str,
    args: &BackupArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let parsed = parse_refs(&args.refs)?;

    let use_bundle =
        args.format.as_deref() == Some("bundle") || is_bundle_path(&args.url);
    let url = if use_bundle {
        args.url.clone()
    } else {
        crate::mirror::resolve_credentials(&args.url)
    };

    let mut opts = BackupOptions {
        dry_run: args.dry_run,
        squash: args.squash,
        format: args.format.clone(),
        ..Default::default()
    };

    match parsed {
        Some(ParsedRefs::List(list)) => {
            opts.refs = Some(list);
        }
        Some(ParsedRefs::Map(map)) => {
            opts.ref_map = Some(map);
        }
        None => {}
    }

    let diff = store.backup(&url, &opts).map_err(CliError::from)?;

    if args.dry_run {
        print_diff(&diff, "push");
    } else {
        status(verbose, &format!("Backed up to {}", args.url));
    }
    Ok(())
}

pub fn cmd_restore(
    repo_opt: &Option<String>,
    args: &RestoreArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let repo_path = require_repo(repo_opt)?;
    let store = if args.no_create {
        open_store(&repo_path)?
    } else {
        open_or_create_bare(&repo_path)?
    };

    let parsed = parse_refs(&args.refs)?;

    let use_bundle =
        args.format.as_deref() == Some("bundle") || is_bundle_path(&args.url);
    let url = if use_bundle {
        args.url.clone()
    } else {
        crate::mirror::resolve_credentials(&args.url)
    };

    let mut opts = RestoreOptions {
        dry_run: args.dry_run,
        format: args.format.clone(),
        ..Default::default()
    };

    match parsed {
        Some(ParsedRefs::List(list)) => {
            opts.refs = Some(list);
        }
        Some(ParsedRefs::Map(map)) => {
            opts.ref_map = Some(map);
        }
        None => {}
    }

    let diff = store.restore(&url, &opts).map_err(CliError::from)?;

    if args.dry_run {
        print_diff(&diff, "pull");
    } else {
        status(verbose, &format!("Restored from {}", args.url));
    }
    Ok(())
}
