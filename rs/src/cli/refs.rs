use clap::{Args, Subcommand};

use crate::fs::Fs;
use crate::store::GitStore;

use super::error::CliError;
use super::helpers::*;

// ---------------------------------------------------------------------------
// Branch
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum BranchCommand {
    /// List all branches.
    List,
    /// Create or update a branch.
    Set(BranchSetArgs),
    /// Check if branch exists (exit 0 if yes, 1 if no).
    Exists(BranchNameArg),
    /// Delete a branch.
    Delete(BranchNameArg),
    /// Print the commit hash of a branch.
    Hash(BranchHashArgs),
    /// Show or set the current branch.
    Current(BranchCurrentArgs),
}

#[derive(Args, Debug)]
pub struct BranchNameArg {
    pub name: String,
}

#[derive(Args, Debug)]
pub struct BranchSetArgs {
    pub name: String,
    /// Branch to fork from.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Overwrite if branch already exists.
    #[arg(short, long)]
    pub force: bool,
    /// Create an empty root branch (no parent commit).
    #[arg(long)]
    pub empty: bool,
    /// Squash to a single commit (no history).
    #[arg(long)]
    pub squash: bool,
    /// Append source tree on branch tip. Requires --squash.
    #[arg(long)]
    pub append: bool,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

#[derive(Args, Debug)]
pub struct BranchHashArgs {
    pub name: String,
    /// Walk back N commits.
    #[arg(long, default_value_t = 0)]
    pub back: usize,
    /// Use latest commit on or before this date.
    #[arg(long)]
    pub before: Option<String>,
    /// Use latest commit matching this message pattern.
    #[arg(long = "match")]
    pub match_pattern: Option<String>,
    /// Use latest commit that changed this path.
    #[arg(long = "path")]
    pub at_path: Option<String>,
}

#[derive(Args, Debug)]
pub struct BranchCurrentArgs {
    /// Set the current branch to this name.
    #[arg(short, long)]
    pub branch: Option<String>,
}

pub fn cmd_branch(
    repo_path: &str,
    cmd: &BranchCommand,
    verbose: bool,
) -> Result<(), CliError> {
    match cmd {
        BranchCommand::List => cmd_branch_list(repo_path),
        BranchCommand::Set(args) => cmd_branch_set(repo_path, args, verbose),
        BranchCommand::Exists(args) => cmd_branch_exists(repo_path, &args.name),
        BranchCommand::Delete(args) => cmd_branch_delete(repo_path, &args.name, verbose),
        BranchCommand::Hash(args) => cmd_branch_hash(repo_path, args),
        BranchCommand::Current(args) => cmd_branch_current(repo_path, args, verbose),
    }
}

fn cmd_branch_list(repo_path: &str) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let names = store.branches().list().map_err(CliError::from)?;
    for name in names {
        println!("{}", name);
    }
    Ok(())
}

fn cmd_branch_set(
    repo_path: &str,
    args: &BranchSetArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let store = open_store(repo_path)?;

    if args.append {
        if args.empty {
            return Err(CliError::new("--append cannot be combined with --empty"));
        }
        if !args.squash {
            return Err(CliError::new(
                "--append without --squash (replay full commit chain) is not yet implemented",
            ));
        }
        if !store.branches().has(&args.name).unwrap_or(false) {
            return Err(CliError::new(format!(
                "--append requires existing branch: {}",
                args.name
            )));
        }
        let branch = args
            .branch
            .clone()
            .unwrap_or_else(|| current_branch(&store));
        let source_fs = resolve_fs(&store, &branch, &args.snap)?;
        let tip = store
            .branches()
            .get(&args.name)
            .map_err(CliError::from)?;
        let squashed = source_fs
            .squash(Some(&tip), None)
            .map_err(CliError::from)?;
        store
            .branches()
            .set(&args.name, &squashed)
            .map_err(CliError::from)?;
    } else if store.branches().has(&args.name).unwrap_or(false) && !args.force {
        return Err(CliError::new(format!(
            "Branch already exists: {}",
            args.name
        )));
    } else if args.empty {
        if args.snap.ref_name.is_some()
            || args.snap.at_path.is_some()
            || args.snap.match_pattern.is_some()
            || args.snap.before.is_some()
            || args.snap.back > 0
        {
            return Err(CliError::new(
                "--empty cannot be combined with --ref/--path/--match/--before/--back",
            ));
        }
        // Delete existing branch if --force
        if args.force && store.branches().has(&args.name).unwrap_or(false) {
            store.branches().delete(&args.name).map_err(CliError::from)?;
        }
        store.create_empty_branch(&args.name).map_err(CliError::from)?;
    } else {
        let branch = args
            .branch
            .clone()
            .unwrap_or_else(|| current_branch(&store));
        let source_fs = resolve_fs(&store, &branch, &args.snap)?;
        let target_fs = if args.squash {
            source_fs.squash(None, None).map_err(CliError::from)?
        } else {
            source_fs
        };
        // Delete existing if force
        if args.force && store.branches().has(&args.name).unwrap_or(false) {
            store
                .branches()
                .delete(&args.name)
                .map_err(CliError::from)?;
        }
        store
            .branches()
            .set(&args.name, &target_fs)
            .map_err(CliError::from)?;
    }

    status(verbose, &format!("Set branch {}", args.name));
    Ok(())
}

fn cmd_branch_exists(repo_path: &str, name: &str) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    if !store.branches().has(name).unwrap_or(false) {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_branch_delete(repo_path: &str, name: &str, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    store
        .branches()
        .delete(name)
        .map_err(|_| CliError::new(format!("Branch not found: {}", name)))?;
    status(verbose, &format!("Deleted branch {}", name));
    Ok(())
}

fn cmd_branch_hash(repo_path: &str, args: &BranchHashArgs) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let fs = store
        .branches()
        .get(&args.name)
        .map_err(|_| CliError::new(format!("Branch not found: {}", args.name)))?;

    let snap = SnapshotArgs {
        at_path: args.at_path.clone(),
        match_pattern: args.match_pattern.clone(),
        before: args.before.clone(),
        back: args.back,
        ..Default::default()
    };
    let fs = apply_snapshot_filters(fs, &snap)?;
    println!(
        "{}",
        fs.commit_hash()
            .ok_or_else(|| CliError::new("No commit"))?
    );
    Ok(())
}

fn cmd_branch_current(
    repo_path: &str,
    args: &BranchCurrentArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    if let Some(ref branch) = args.branch {
        if !store.branches().has(branch).unwrap_or(false) {
            return Err(CliError::new(format!("Branch not found: {}", branch)));
        }
        store
            .branches()
            .set_current(branch)
            .map_err(CliError::from)?;
        status(verbose, &format!("Current branch set to {}", branch));
    } else {
        let name = store
            .branches()
            .get_current_name()
            .map_err(CliError::from)?
            .ok_or_else(|| {
                CliError::new("HEAD does not point to an existing branch")
            })?;
        println!("{}", name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum TagCommand {
    /// List all tags.
    List,
    /// Create or update a tag.
    Set(TagSetArgs),
    /// Check if tag exists (exit 0 if yes, 1 if no).
    Exists(TagNameArg),
    /// Delete a tag.
    Delete(TagNameArg),
    /// Print the commit hash of a tag.
    Hash(TagNameArg),
}

#[derive(Args, Debug)]
pub struct TagNameArg {
    pub name: String,
}

#[derive(Args, Debug)]
pub struct TagSetArgs {
    pub name: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Overwrite if tag already exists.
    #[arg(short, long)]
    pub force: bool,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_tag(
    repo_path: &str,
    cmd: &TagCommand,
    verbose: bool,
) -> Result<(), CliError> {
    match cmd {
        TagCommand::List => cmd_tag_list(repo_path),
        TagCommand::Set(args) => cmd_tag_set(repo_path, args, verbose),
        TagCommand::Exists(args) => cmd_tag_exists(repo_path, &args.name),
        TagCommand::Delete(args) => cmd_tag_delete(repo_path, &args.name, verbose),
        TagCommand::Hash(args) => cmd_tag_hash(repo_path, &args.name),
    }
}

fn cmd_tag_list(repo_path: &str) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let names = store.tags().list().map_err(CliError::from)?;
    for name in names {
        println!("{}", name);
    }
    Ok(())
}

fn cmd_tag_set(
    repo_path: &str,
    args: &TagSetArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let store = open_store(repo_path)?;

    if store.tags().has(&args.name).unwrap_or(false) && !args.force {
        return Err(CliError::new(format!("Tag already exists: {}", args.name)));
    }

    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let source_fs = resolve_fs(&store, &branch, &args.snap)?;

    // Delete existing if force
    if args.force && store.tags().has(&args.name).unwrap_or(false) {
        store.tags().delete(&args.name).map_err(CliError::from)?;
    }
    store
        .tags()
        .set(&args.name, &source_fs)
        .map_err(CliError::from)?;
    status(verbose, &format!("Set tag {}", args.name));
    Ok(())
}

fn cmd_tag_exists(repo_path: &str, name: &str) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    if !store.tags().has(name).unwrap_or(false) {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_tag_delete(repo_path: &str, name: &str, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    store
        .tags()
        .delete(name)
        .map_err(|_| CliError::new(format!("Tag not found: {}", name)))?;
    status(verbose, &format!("Deleted tag {}", name));
    Ok(())
}

fn cmd_tag_hash(repo_path: &str, name: &str) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let fs = store
        .tags()
        .get(name)
        .map_err(|_| CliError::new(format!("Tag not found: {}", name)))?;
    println!(
        "{}",
        fs.commit_hash()
            .ok_or_else(|| CliError::new("No commit"))?
    );
    Ok(())
}
