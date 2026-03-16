use std::io::{self, Read as _, Write as _};

use clap::Args;

use crate::fs::BatchOptions;
use crate::types::{FileType, MODE_BLOB_EXEC};

use super::error::CliError;
use super::helpers::*;

// ---------------------------------------------------------------------------
// zip
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ZipArgs {
    /// Output zip path (use '-' for stdout).
    pub filename: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_zip(repo_path: &str, args: &ZipArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = resolve_fs(&store, &branch, &args.snap)?;
    do_export_zip(&fs, &args.filename, verbose)
}

// ---------------------------------------------------------------------------
// unzip
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct UnzipArgs {
    /// Path to zip file.
    pub filename: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    #[command(flatten)]
    pub tag_args: TagArgs,
}

pub fn cmd_unzip(
    repo_opt: &Option<String>,
    args: &UnzipArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let repo_path = require_repo(repo_opt)?;
    let store = if args.no_create {
        open_store(&repo_path)?
    } else {
        open_or_create_store(&repo_path, args.branch.as_deref().unwrap_or("main"))?
    };
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let new_fs = do_import_zip(&store, &branch, &args.filename, args.message.as_deref(), verbose)?;
    if let Some(ref tag) = args.tag_args.tag {
        apply_tag(&store, &new_fs, tag, args.tag_args.force_tag)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// tar
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct TarArgs {
    /// Output tar path (use '-' for stdout).
    pub filename: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_tar(repo_path: &str, args: &TarArgs, verbose: bool) -> Result<(), CliError> {
    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = resolve_fs(&store, &branch, &args.snap)?;
    do_export_tar(&fs, &args.filename, verbose)
}

// ---------------------------------------------------------------------------
// untar
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct UntarArgs {
    /// Path to tar file (use '-' for stdin, default).
    #[arg(default_value = "-")]
    pub filename: String,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    #[command(flatten)]
    pub tag_args: TagArgs,
}

pub fn cmd_untar(
    repo_opt: &Option<String>,
    args: &UntarArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let repo_path = require_repo(repo_opt)?;
    let store = if args.no_create {
        open_store(&repo_path)?
    } else {
        open_or_create_store(&repo_path, args.branch.as_deref().unwrap_or("main"))?
    };
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let new_fs = do_import_tar(&store, &branch, &args.filename, args.message.as_deref(), verbose)?;
    if let Some(ref tag) = args.tag_args.tag {
        apply_tag(&store, &new_fs, tag, args.tag_args.force_tag)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// archive_out
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ArchiveOutArgs {
    /// Output archive path (use '-' for stdout).
    pub filename: String,
    /// Archive format (auto-detected from extension).
    #[arg(long, value_parser = ["zip", "tar"])]
    pub format: Option<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
}

pub fn cmd_archive_out(
    repo_path: &str,
    args: &ArchiveOutArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let fmt = if let Some(ref f) = args.format {
        f.clone()
    } else if args.filename == "-" {
        return Err(CliError::new("Use --format with stdout (-)"));
    } else {
        detect_archive_format(&args.filename)?
    };

    let store = open_store(repo_path)?;
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));
    let fs = resolve_fs(&store, &branch, &args.snap)?;

    if fmt == "zip" {
        do_export_zip(&fs, &args.filename, verbose)
    } else {
        do_export_tar(&fs, &args.filename, verbose)
    }
}

// ---------------------------------------------------------------------------
// archive_in
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ArchiveInArgs {
    /// Input archive path (use '-' or omit for stdin).
    pub filename: Option<String>,
    /// Archive format (auto-detected from extension).
    #[arg(long, value_parser = ["zip", "tar"])]
    pub format: Option<String>,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Commit message.
    #[arg(short, long)]
    pub message: Option<String>,
    /// Do not auto-create the repository.
    #[arg(long)]
    pub no_create: bool,
    #[command(flatten)]
    pub tag_args: TagArgs,
}

pub fn cmd_archive_in(
    repo_opt: &Option<String>,
    args: &ArchiveInArgs,
    verbose: bool,
) -> Result<(), CliError> {
    let filename = args.filename.as_deref().unwrap_or("-");
    let fmt = if let Some(ref f) = args.format {
        f.clone()
    } else if filename == "-" {
        return Err(CliError::new("Use --format when reading from stdin"));
    } else {
        detect_archive_format(filename)?
    };

    let repo_path = require_repo(repo_opt)?;
    let store = if args.no_create {
        open_store(&repo_path)?
    } else {
        open_or_create_store(&repo_path, args.branch.as_deref().unwrap_or("main"))?
    };
    let branch = args
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&store));

    let new_fs = if fmt == "zip" {
        do_import_zip(&store, &branch, filename, args.message.as_deref(), verbose)?
    } else {
        do_import_tar(&store, &branch, filename, args.message.as_deref(), verbose)?
    };

    if let Some(ref tag) = args.tag_args.tag {
        apply_tag(&store, &new_fs, tag, args.tag_args.force_tag)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Export helpers
// ---------------------------------------------------------------------------

fn do_export_zip(
    fs: &crate::Fs,
    filename: &str,
    verbose: bool,
) -> Result<(), CliError> {
    let walk = fs.walk("").map_err(CliError::from)?;
    let to_stdout = filename == "-";

    let mut buf = io::Cursor::new(Vec::new());
    let mut count = 0u64;

    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        for wde in &walk {
            for fe in &wde.files {
                let repo_path = if wde.dirpath.is_empty() {
                    fe.name.clone()
                } else {
                    format!("{}/{}", wde.dirpath, fe.name)
                };
                let data = fs.read(&repo_path).map_err(CliError::from)?;

                let ft = fe.file_type();
                if ft == Some(FileType::Link) {
                    // Symlink: use add_symlink which correctly sets S_IFLNK
                    let target = String::from_utf8(data.clone()).map_err(|_| {
                        CliError::new(format!(
                            "Symlink target for {} is not valid UTF-8",
                            repo_path
                        ))
                    })?;
                    zw.add_symlink(&repo_path, &target, options)
                        .map_err(|e| CliError::new(e.to_string()))?;
                } else {
                    let mode = if ft == Some(FileType::Executable) {
                        0o755
                    } else {
                        0o644
                    };
                    let opts = options.unix_permissions(mode);
                    zw.start_file(&repo_path, opts)
                        .map_err(|e| CliError::new(e.to_string()))?;
                    zw.write_all(&data)
                        .map_err(|e| CliError::new(e.to_string()))?;
                }
                count += 1;
            }
        }

        zw.finish().map_err(|e| CliError::new(e.to_string()))?;
    }

    let data = buf.into_inner();
    if to_stdout {
        io::stdout().write_all(&data)?;
    } else {
        std::fs::write(filename, &data)?;
    }

    status(verbose, &format!("Wrote {} file(s) to {}", count, filename));
    Ok(())
}

fn do_export_tar(
    fs: &crate::Fs,
    filename: &str,
    verbose: bool,
) -> Result<(), CliError> {
    let walk = fs.walk("").map_err(CliError::from)?;
    let to_stdout = filename == "-";

    let mut buf = Vec::new();
    let mut count = 0u64;

    {
        let mut builder = tar::Builder::new(&mut buf);

        for wde in &walk {
            for fe in &wde.files {
                let repo_path = if wde.dirpath.is_empty() {
                    fe.name.clone()
                } else {
                    format!("{}/{}", wde.dirpath, fe.name)
                };
                let data = fs.read(&repo_path).map_err(CliError::from)?;
                let ft = fe.file_type();

                if ft == Some(FileType::Link) {
                    let target = String::from_utf8(data)
                        .map_err(|_| CliError::new(format!("Symlink target for {} is not valid UTF-8", repo_path)))?;
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    header.set_mode(0o777);
                    header.set_cksum();
                    builder
                        .append_link(&mut header, &repo_path, &target)
                        .map_err(|e| CliError::new(e.to_string()))?;
                } else {
                    let mode = if ft == Some(FileType::Executable) {
                        0o755
                    } else {
                        0o644
                    };
                    let mut header = tar::Header::new_gnu();
                    header.set_size(data.len() as u64);
                    header.set_mode(mode);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, &repo_path, data.as_slice())
                        .map_err(|e| CliError::new(e.to_string()))?;
                }
                count += 1;
            }
        }

        builder
            .finish()
            .map_err(|e| CliError::new(e.to_string()))?;
    }

    if to_stdout {
        io::stdout().write_all(&buf)?;
    } else {
        // Determine compression from filename
        let lower = filename.to_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            use flate2::write::GzEncoder;
            let file = std::fs::File::create(filename)?;
            let mut enc = GzEncoder::new(file, flate2::Compression::default());
            enc.write_all(&buf)?;
            enc.finish()?;
        } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
            use bzip2::write::BzEncoder;
            let file = std::fs::File::create(filename)?;
            let mut enc = BzEncoder::new(file, bzip2::Compression::default());
            enc.write_all(&buf)?;
            enc.finish()?;
        } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
            use xz2::write::XzEncoder;
            let file = std::fs::File::create(filename)?;
            let mut enc = XzEncoder::new(file, 6);
            enc.write_all(&buf)?;
            enc.finish()?;
        } else {
            std::fs::write(filename, &buf)?;
        }
    }

    status(verbose, &format!("Wrote {} file(s) to {}", count, filename));
    Ok(())
}

// ---------------------------------------------------------------------------
// Import helpers
// ---------------------------------------------------------------------------

fn do_import_zip(
    store: &crate::GitStore,
    branch: &str,
    filename: &str,
    message: Option<&str>,
    verbose: bool,
) -> Result<crate::Fs, CliError> {
    let fs = get_branch_fs(store, branch)?;

    let data = if filename == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        std::fs::read(filename)?
    };

    let cursor = io::Cursor::new(&data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| CliError::new(format!("Not a valid zip file: {}", e)))?;

    let mut batch = fs.batch(BatchOptions {
        message: message.map(|s| s.to_string()),
        operation: Some("ar".to_string()),
        ..Default::default()
    });

    let mut count = 0u64;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| CliError::new(e.to_string()))?;

        if file.is_dir() {
            continue;
        }

        let repo_path = clean_archive_path(file.name())?;
        let unix_mode = file.unix_mode().unwrap_or(0o644);

        let mut file_data = Vec::new();
        file.read_to_end(&mut file_data)?;

        if (unix_mode & 0o170000) == 0o120000 {
            // Symlink
            let target = String::from_utf8(file_data)
                .map_err(|_| CliError::new("Symlink target is not valid UTF-8"))?;
            batch
                .write_symlink(&repo_path, &target)
                .map_err(CliError::from)?;
        } else {
            let mode = if unix_mode & 0o111 != 0 {
                Some(MODE_BLOB_EXEC)
            } else {
                None
            };
            if let Some(m) = mode {
                batch
                    .write_with_mode(&repo_path, &file_data, m)
                    .map_err(CliError::from)?;
            } else {
                batch
                    .write(&repo_path, &file_data)
                    .map_err(CliError::from)?;
            }
        }
        count += 1;
    }

    if count == 0 {
        return Err(CliError::new("Zip file contains no files"));
    }

    let result_fs = batch.commit().map_err(CliError::from)?;
    status(
        verbose,
        &format!("Imported {} file(s) from {}", count, filename),
    );
    Ok(result_fs)
}

fn do_import_tar(
    store: &crate::GitStore,
    branch: &str,
    filename: &str,
    message: Option<&str>,
    verbose: bool,
) -> Result<crate::Fs, CliError> {
    let fs = get_branch_fs(store, branch)?;

    let data = if filename == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        std::fs::read(filename)?
    };

    // Detect compression
    let reader: Box<dyn io::Read> = if data.starts_with(&[0x1f, 0x8b]) {
        // gzip
        Box::new(flate2::read::GzDecoder::new(io::Cursor::new(data)))
    } else if data.starts_with(b"BZ") {
        // bzip2
        Box::new(bzip2::read::BzDecoder::new(io::Cursor::new(data)))
    } else if data.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        // xz
        Box::new(xz2::read::XzDecoder::new(io::Cursor::new(data)))
    } else {
        Box::new(io::Cursor::new(data))
    };

    let mut archive =
        tar::Archive::new(reader);

    let mut batch = fs.batch(BatchOptions {
        message: message.map(|s| s.to_string()),
        operation: Some("ar".to_string()),
        ..Default::default()
    });

    let mut count = 0u64;
    // Track file data+mode by path for hard link resolution
    let mut file_cache: std::collections::HashMap<String, (Vec<u8>, u32)> =
        std::collections::HashMap::new();
    let entries = archive.entries().map_err(|e| CliError::new(format!("Not a valid tar archive: {}", e)))?;
    for entry_result in entries {
        let mut entry = entry_result
            .map_err(|e| CliError::new(format!("Not a valid tar archive: {}", e)))?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        let path = entry
            .path()
            .map_err(|e| CliError::new(e.to_string()))?
            .to_string_lossy()
            .to_string();

        if entry_type == tar::EntryType::Symlink {
            if let Some(link) = header.link_name().ok().flatten() {
                let repo_path = clean_archive_path(&path)?;
                let target = link.to_string_lossy().to_string();
                batch
                    .write_symlink(&repo_path, &target)
                    .map_err(CliError::from)?;
                count += 1;
            }
        } else if entry_type == tar::EntryType::Link {
            // Hard link: try to read data from the entry, fall back to cache
            let link_target = header
                .link_name()
                .ok()
                .flatten()
                .map(|l| l.to_string_lossy().to_string())
                .unwrap_or_default();

            let mut file_data = Vec::new();
            let _ = entry.read_to_end(&mut file_data);

            let (data, mode) = if !file_data.is_empty() {
                let mode = header.mode().unwrap_or(0o644);
                (file_data, mode)
            } else if let Some((cached_data, cached_mode)) = file_cache.get(&link_target) {
                (cached_data.clone(), *cached_mode)
            } else {
                // Can't resolve: skip with warning
                eprintln!(
                    "WARNING: skipping hard link {} -> {} (target not yet seen)",
                    path, link_target
                );
                continue;
            };

            let repo_path = clean_archive_path(&path)?;
            if mode & 0o111 != 0 {
                batch
                    .write_with_mode(&repo_path, &data, MODE_BLOB_EXEC)
                    .map_err(CliError::from)?;
            } else {
                batch
                    .write(&repo_path, &data)
                    .map_err(CliError::from)?;
            }
            file_cache.insert(path, (data, mode));
            count += 1;
        } else if entry_type == tar::EntryType::Regular || entry_type == tar::EntryType::GNUSparse {
            let repo_path = clean_archive_path(&path)?;
            let mut file_data = Vec::new();
            entry.read_to_end(&mut file_data)?;

            let mode = header.mode().unwrap_or(0o644);
            if mode & 0o111 != 0 {
                batch
                    .write_with_mode(&repo_path, &file_data, MODE_BLOB_EXEC)
                    .map_err(CliError::from)?;
            } else {
                batch
                    .write(&repo_path, &file_data)
                    .map_err(CliError::from)?;
            }
            file_cache.insert(path, (file_data, mode));
            count += 1;
        }
    }

    if count == 0 {
        return Err(CliError::new("Tar archive contains no files"));
    }

    let result_fs = batch.commit().map_err(CliError::from)?;
    status(
        verbose,
        &format!("Imported {} file(s) from {}", count, filename),
    );
    Ok(result_fs)
}
