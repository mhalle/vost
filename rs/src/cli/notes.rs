use clap::{Args, Subcommand};

use super::error::CliError;
use super::helpers::*;

#[derive(Subcommand, Debug)]
pub enum NoteCommand {
    /// Get the note for a commit hash or ref name.
    Get(NoteGetArgs),
    /// Set the note for a commit hash or ref name.
    Set(NoteSetArgs),
    /// Delete the note for a commit hash or ref name.
    Delete(NoteDeleteArgs),
    /// List commit hashes that have notes.
    List(NoteListArgs),
}

#[derive(Args, Debug)]
pub struct NoteGetArgs {
    /// Target commit hash or ref name. Omit for current branch.
    pub target: Option<String>,
    /// Notes namespace.
    #[arg(short = 'N', long, default_value = "commits")]
    pub namespace: String,
}

#[derive(Args, Debug)]
pub struct NoteSetArgs {
    /// Target and text. Use "TEXT" or "TARGET TEXT".
    #[arg(required = true)]
    pub args: Vec<String>,
    /// Notes namespace.
    #[arg(short = 'N', long, default_value = "commits")]
    pub namespace: String,
}

#[derive(Args, Debug)]
pub struct NoteDeleteArgs {
    /// Target commit hash or ref name. Omit for current branch.
    pub target: Option<String>,
    /// Notes namespace.
    #[arg(short = 'N', long, default_value = "commits")]
    pub namespace: String,
}

#[derive(Args, Debug)]
pub struct NoteListArgs {
    /// Notes namespace.
    #[arg(short = 'N', long, default_value = "commits")]
    pub namespace: String,
}

fn resolve_note_target(
    store: &crate::GitStore,
    target: Option<&str>,
) -> String {
    match target {
        None | Some(":") => current_branch(store),
        Some(t) if t.ends_with(':') => t[..t.len() - 1].to_string(),
        Some(t) => t.to_string(),
    }
}

pub fn cmd_note(
    repo_path: &str,
    cmd: &NoteCommand,
    verbose: bool,
) -> Result<(), CliError> {
    match cmd {
        NoteCommand::Get(args) => {
            let store = open_store(repo_path)?;
            let resolved = resolve_note_target(&store, args.target.as_deref());
            let ns = store.notes().namespace(&args.namespace);
            let text = ns
                .get(&resolved)
                .map_err(|_| {
                    CliError::new(format!(
                        "No note for {} in namespace '{}'",
                        resolved, args.namespace
                    ))
                })?;
            print!("{}", text);
            Ok(())
        }
        NoteCommand::Set(args) => {
            let (target, text) = match args.args.len() {
                1 => (None, args.args[0].as_str()),
                2 => (Some(args.args[0].as_str()), args.args[1].as_str()),
                _ => {
                    return Err(CliError::new(
                        "Usage: vost note set [TARGET] TEXT",
                    ))
                }
            };
            let store = open_store(repo_path)?;
            let resolved = resolve_note_target(&store, target);
            let ns = store.notes().namespace(&args.namespace);
            ns.set(&resolved, text).map_err(CliError::from)?;
            status(verbose, &format!("Note set for {}", resolved));
            Ok(())
        }
        NoteCommand::Delete(args) => {
            let store = open_store(repo_path)?;
            let resolved = resolve_note_target(&store, args.target.as_deref());
            let ns = store.notes().namespace(&args.namespace);
            ns.delete(&resolved).map_err(|_| {
                CliError::new(format!(
                    "No note for {} in namespace '{}'",
                    resolved, args.namespace
                ))
            })?;
            status(verbose, &format!("Note deleted for {}", resolved));
            Ok(())
        }
        NoteCommand::List(args) => {
            let store = open_store(repo_path)?;
            let ns = store.notes().namespace(&args.namespace);
            let hashes = ns.list().map_err(CliError::from)?;
            let mut sorted = hashes;
            sorted.sort();
            for h in sorted {
                println!("{}", h);
            }
            Ok(())
        }
    }
}
