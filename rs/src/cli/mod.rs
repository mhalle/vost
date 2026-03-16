pub mod archive;
pub mod basic;
pub mod cp;
pub mod error;
pub mod helpers;
pub mod history;
pub mod mirror;
pub mod notes;
pub mod output;
pub mod refs;
pub mod serve;
pub mod sync_cmd;

use clap::{Parser, Subcommand};
use error::CliError;
use helpers::require_repo;

#[derive(Parser)]
#[command(name = "vost", version, about = "A git-backed versioned file store")]
pub struct Cli {
    /// Path to bare git repository (or set VOST_REPO).
    #[arg(short, long, env = "VOST_REPO", global = true)]
    pub repo: Option<String>,
    /// Verbose output on stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new bare git repository.
    Init(basic::InitArgs),
    /// Remove a bare git repository.
    Destroy(basic::DestroyArgs),
    /// Clean up and pack loose objects.
    Gc,
    /// Pack loose objects into a packfile.
    Pack,
    /// List files/directories.
    Ls(basic::LsArgs),
    /// Concatenate file contents to stdout.
    Cat(basic::CatArgs),
    /// Print the SHA hash of a commit, tree, or blob.
    Hash(basic::HashArgs),
    /// Remove files from the repo.
    Rm(basic::RmArgs),
    /// Move/rename files in the repo.
    Mv(basic::MvArgs),
    /// Write stdin to a file in the repo.
    Write(basic::WriteArgs),
    /// Show commit log.
    Log(history::LogArgs),
    /// Show files that differ between snapshots.
    Diff(history::DiffArgs),
    /// Compare two files by content hash.
    Cmp(history::CmpArgs),
    /// Move branch back N commits.
    Undo(history::UndoArgs),
    /// Move branch forward N steps in reflog.
    Redo(history::RedoArgs),
    /// Show reflog entries for a branch.
    Reflog(history::ReflogArgs),
    /// Copy files between disk and repo, or between refs.
    Cp(cp::CpArgs),
    /// Make one path identical to another.
    Sync(sync_cmd::SyncArgs),
    /// Export repo contents to a zip file.
    Zip(archive::ZipArgs),
    /// Import a zip file into the repo.
    Unzip(archive::UnzipArgs),
    /// Export repo contents to a tar archive.
    Tar(archive::TarArgs),
    /// Import a tar archive into the repo.
    Untar(archive::UntarArgs),
    /// Export repo contents to an archive file.
    #[command(alias = "archive_out")]
    ArchiveOut(archive::ArchiveOutArgs),
    /// Import an archive file into the repo.
    #[command(alias = "archive_in")]
    ArchiveIn(archive::ArchiveInArgs),
    /// Push refs to a remote URL or write a bundle file.
    Backup(mirror::BackupArgs),
    /// Fetch refs from a remote URL or import a bundle file.
    Restore(mirror::RestoreArgs),
    /// Manage branches (default: list).
    #[command(subcommand_required = false)]
    Branch {
        #[command(subcommand)]
        command: Option<refs::BranchCommand>,
    },
    /// Manage tags (default: list).
    #[command(subcommand_required = false)]
    Tag {
        #[command(subcommand)]
        command: Option<refs::TagCommand>,
    },
    /// Serve repository files over HTTP.
    Serve(serve::ServeArgs),
    /// Manage git notes on commits.
    #[command(subcommand)]
    Note(notes::NoteCommand),
}

pub fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let v = cli.verbose;

    match &cli.command {
        Commands::Init(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_init(&rp, args, v)
        }
        Commands::Destroy(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_destroy(&rp, args, v)
        }
        Commands::Gc => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_gc(&rp, v)
        }
        Commands::Pack => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_pack(&rp, v)
        }
        Commands::Ls(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_ls(&rp, args, v)
        }
        Commands::Cat(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_cat(&rp, args, v)
        }
        Commands::Hash(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_hash(&rp, args, v)
        }
        Commands::Write(args) => basic::cmd_write(&cli.repo, args, v),
        Commands::Rm(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_rm(&rp, args, v)
        }
        Commands::Mv(args) => {
            let rp = require_repo(&cli.repo)?;
            basic::cmd_mv(&rp, args, v)
        }
        Commands::Log(args) => {
            let rp = require_repo(&cli.repo)?;
            history::cmd_log(&rp, args, v)
        }
        Commands::Diff(args) => {
            let rp = require_repo(&cli.repo)?;
            history::cmd_diff(&rp, args, v)
        }
        Commands::Cmp(args) => history::cmd_cmp(&cli.repo, args, v),
        Commands::Undo(args) => {
            let rp = require_repo(&cli.repo)?;
            history::cmd_undo(&rp, args, v)
        }
        Commands::Redo(args) => {
            let rp = require_repo(&cli.repo)?;
            history::cmd_redo(&rp, args, v)
        }
        Commands::Reflog(args) => {
            let rp = require_repo(&cli.repo)?;
            history::cmd_reflog(&rp, args, v)
        }
        Commands::Cp(args) => cp::cmd_cp(&cli.repo, args, v),
        Commands::Sync(args) => sync_cmd::cmd_sync(&cli.repo, args, v),
        Commands::Zip(args) => {
            let rp = require_repo(&cli.repo)?;
            archive::cmd_zip(&rp, args, v)
        }
        Commands::Unzip(args) => archive::cmd_unzip(&cli.repo, args, v),
        Commands::Tar(args) => {
            let rp = require_repo(&cli.repo)?;
            archive::cmd_tar(&rp, args, v)
        }
        Commands::Untar(args) => archive::cmd_untar(&cli.repo, args, v),
        Commands::ArchiveOut(args) => {
            let rp = require_repo(&cli.repo)?;
            archive::cmd_archive_out(&rp, args, v)
        }
        Commands::ArchiveIn(args) => archive::cmd_archive_in(&cli.repo, args, v),
        Commands::Backup(args) => {
            let rp = require_repo(&cli.repo)?;
            mirror::cmd_backup(&rp, args, v)
        }
        Commands::Restore(args) => mirror::cmd_restore(&cli.repo, args, v),
        Commands::Branch { command } => {
            let rp = require_repo(&cli.repo)?;
            refs::cmd_branch(&rp, command.as_ref().unwrap_or(&refs::BranchCommand::List), v)
        }
        Commands::Tag { command } => {
            let rp = require_repo(&cli.repo)?;
            refs::cmd_tag(&rp, command.as_ref().unwrap_or(&refs::TagCommand::List), v)
        }
        Commands::Serve(args) => {
            let rp = require_repo(&cli.repo)?;
            serve::cmd_serve(&rp, args, v)
        }
        Commands::Note(cmd) => {
            let rp = require_repo(&cli.repo)?;
            notes::cmd_note(&rp, cmd, v)
        }
    }
}
