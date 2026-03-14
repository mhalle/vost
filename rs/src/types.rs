use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Mode constants
// ---------------------------------------------------------------------------

/// Regular file mode (non-executable).
pub const MODE_BLOB: u32 = 0o100644;
/// Executable file mode.
pub const MODE_BLOB_EXEC: u32 = 0o100755;
/// Symbolic link mode.
pub const MODE_LINK: u32 = 0o120000;
/// Directory (tree) mode.
pub const MODE_TREE: u32 = 0o040000;

// ---------------------------------------------------------------------------
// FileType
// ---------------------------------------------------------------------------

/// The type of a git tree entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// Regular file (`0o100644`).
    Blob,
    /// Executable file (`0o100755`).
    Executable,
    /// Symbolic link (`0o120000`).
    Link,
    /// Directory / subtree (`0o040000`).
    Tree,
}

impl FileType {
    /// Convert a raw git mode to a `FileType`.
    pub fn from_mode(mode: u32) -> Option<Self> {
        match mode {
            MODE_BLOB => Some(Self::Blob),
            MODE_BLOB_EXEC => Some(Self::Executable),
            MODE_LINK => Some(Self::Link),
            MODE_TREE => Some(Self::Tree),
            _ => None,
        }
    }

    /// Return the raw git filemode for this type.
    pub fn filemode(self) -> u32 {
        match self {
            Self::Blob => MODE_BLOB,
            Self::Executable => MODE_BLOB_EXEC,
            Self::Link => MODE_LINK,
            Self::Tree => MODE_TREE,
        }
    }

    /// Whether this type represents a regular file (blob or executable).
    pub fn is_file(self) -> bool {
        matches!(self, Self::Blob | Self::Executable)
    }

    /// Whether this type represents a directory.
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Tree)
    }

    /// Whether this type represents a symlink.
    pub fn is_link(self) -> bool {
        matches!(self, Self::Link)
    }
}

// ---------------------------------------------------------------------------
// WalkEntry
// ---------------------------------------------------------------------------

/// An entry yielded when walking a tree (by [`Fs::walk`](crate::fs::Fs::walk)
/// and [`Fs::listdir`](crate::fs::Fs::listdir)).
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Entry name (file or directory basename).
    pub name: String,
    /// Raw git object ID.
    pub oid: git2::Oid,
    /// Git filemode integer (e.g. `0o100644`).
    pub mode: u32,
}

impl WalkEntry {
    /// Return the [`FileType`] for this entry, or `None` for unknown modes.
    pub fn file_type(&self) -> Option<FileType> {
        FileType::from_mode(self.mode)
    }
}

// ---------------------------------------------------------------------------
// WalkDirEntry
// ---------------------------------------------------------------------------

/// An entry yielded by os.walk-style directory traversal.
///
/// Each entry represents one directory and its immediate contents,
/// split into subdirectory names and non-directory file entries.
#[derive(Debug, Clone)]
pub struct WalkDirEntry {
    /// Directory path (empty string for root).
    pub dirpath: String,
    /// Subdirectory names in this directory.
    pub dirnames: Vec<String>,
    /// Non-directory entries in this directory.
    pub files: Vec<WalkEntry>,
}

// ---------------------------------------------------------------------------
// StatResult
// ---------------------------------------------------------------------------

/// Result of a stat() call — single-call getattr for FUSE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatResult {
    /// Raw git filemode.
    pub mode: u32,
    /// Parsed file type.
    pub file_type: FileType,
    /// Size in bytes (blob length, or number of entries for directories).
    pub size: u64,
    /// 40-char hex SHA of the object.
    pub hash: String,
    /// Number of hard links (2 + subdirectory count for dirs, 1 for files).
    pub nlink: u32,
    /// Commit timestamp (POSIX epoch seconds).
    pub mtime: u64,
}

// ---------------------------------------------------------------------------
// WriteEntry
// ---------------------------------------------------------------------------

/// Data to be written to the store.
#[derive(Debug, Clone)]
pub struct WriteEntry {
    /// Raw content (for blobs).
    pub data: Option<Vec<u8>>,
    /// Symlink target.
    pub target: Option<String>,
    /// Git file mode.
    pub mode: u32,
}

impl WriteEntry {
    /// Create a blob entry from raw bytes.
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: Some(data.into()),
            target: None,
            mode: MODE_BLOB,
        }
    }

    /// Create a blob entry from a UTF-8 string.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self::from_bytes(text.into().into_bytes())
    }

    /// Create a symlink entry.
    pub fn symlink(target: impl Into<String>) -> Self {
        Self {
            data: None,
            target: Some(target.into()),
            mode: MODE_LINK,
        }
    }

    /// Validate that the entry is internally consistent.
    pub fn validate(&self) -> crate::error::Result<()> {
        match self.mode {
            MODE_LINK => {
                if self.target.is_none() {
                    return Err(crate::error::Error::invalid_path(
                        "symlink entry requires a target",
                    ));
                }
                if self.data.is_some() {
                    return Err(crate::error::Error::invalid_path(
                        "symlink entry must not have data",
                    ));
                }
            }
            MODE_BLOB | MODE_BLOB_EXEC => {
                if self.data.is_none() {
                    return Err(crate::error::Error::invalid_path(
                        "blob entry requires data",
                    ));
                }
                if self.target.is_some() {
                    return Err(crate::error::Error::invalid_path(
                        "blob entry must not have a symlink target",
                    ));
                }
            }
            _ => {
                return Err(crate::error::Error::invalid_path(format!(
                    "unsupported mode: {:#o}",
                    self.mode
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FileEntry
// ---------------------------------------------------------------------------

/// Describes a file in a change report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative path within the store.
    pub path: String,
    /// Type of the file.
    pub file_type: FileType,
    /// Source path on disk (for copy_in/copy_out), if applicable.
    pub src: Option<PathBuf>,
}

impl FileEntry {
    /// Create a FileEntry without a source path.
    pub fn new(path: impl Into<String>, file_type: FileType) -> Self {
        Self {
            path: path.into(),
            file_type,
            src: None,
        }
    }

    /// Create a FileEntry with a source path.
    pub fn with_src(path: impl Into<String>, file_type: FileType, src: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file_type,
            src: Some(src.into()),
        }
    }
}

impl PartialOrd for FileEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(&other.path)
    }
}

// ---------------------------------------------------------------------------
// ChangeReport
// ---------------------------------------------------------------------------

/// Kinds of change actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeActionKind {
    /// A new file was added.
    Add,
    /// An existing file was modified.
    Update,
    /// A file was removed.
    Delete,
}

/// A single change action (kind + path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeAction {
    /// Whether this is an add, update, or delete.
    pub kind: ChangeActionKind,
    /// Relative path within the store.
    pub path: String,
}

impl ChangeAction {
    /// Create a new change action.
    pub fn new(kind: ChangeActionKind, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }
}

impl PartialOrd for ChangeAction {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ChangeAction {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(&other.path)
    }
}

/// An error encountered during a change operation.
#[derive(Debug, Clone)]
pub struct ChangeError {
    /// Path that caused the error.
    pub path: String,
    /// Human-readable error description.
    pub error: String,
}

impl ChangeError {
    /// Create a new change error.
    pub fn new(path: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            error: error.into(),
        }
    }
}

/// Report summarising the outcome of a sync / copy / import operation.
#[derive(Debug, Clone, Default)]
pub struct ChangeReport {
    /// Files that were newly added.
    pub add: Vec<FileEntry>,
    /// Files that were modified in place.
    pub update: Vec<FileEntry>,
    /// Files that were removed.
    pub delete: Vec<FileEntry>,
    /// Non-fatal errors encountered during the operation.
    pub errors: Vec<ChangeError>,
    /// Non-fatal warnings (e.g. overlapping destinations).
    pub warnings: Vec<ChangeError>,
}

impl ChangeReport {
    /// Create an empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when nothing was changed.
    pub fn in_sync(&self) -> bool {
        self.add.is_empty() && self.update.is_empty() && self.delete.is_empty()
    }

    /// Total number of changes (add + update + delete).
    pub fn total(&self) -> usize {
        self.add.len() + self.update.len() + self.delete.len()
    }

    /// Return a sorted list of all change actions.
    pub fn actions(&self) -> Vec<ChangeAction> {
        let mut out = Vec::with_capacity(self.total());
        for fe in &self.add {
            out.push(ChangeAction::new(ChangeActionKind::Add, &fe.path));
        }
        for fe in &self.update {
            out.push(ChangeAction::new(ChangeActionKind::Update, &fe.path));
        }
        for fe in &self.delete {
            out.push(ChangeAction::new(ChangeActionKind::Delete, &fe.path));
        }
        out.sort();
        out
    }

    /// Consume the report and return an error if any errors were recorded.
    pub fn finalize(self) -> crate::error::Result<Self> {
        if self.errors.is_empty() {
            Ok(self)
        } else {
            let msgs: Vec<_> = self.errors.iter().map(|e| e.error.clone()).collect();
            Err(crate::error::Error::Permission(msgs.join("; ")))
        }
    }
}

// ---------------------------------------------------------------------------
// Signature / CommitInfo
// ---------------------------------------------------------------------------

/// Author/committer identity used for commits.
#[derive(Debug, Clone)]
pub struct Signature {
    /// Author name (e.g. `"vost"`).
    pub name: String,
    /// Author email (e.g. `"vost@localhost"`).
    pub email: String,
}

impl Default for Signature {
    fn default() -> Self {
        Self {
            name: "vost".into(),
            email: "vost@localhost".into(),
        }
    }
}

/// Information for creating a commit.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// 40-char hex commit SHA.
    pub commit_hash: String,
    /// Commit message text.
    pub message: String,
    /// POSIX epoch seconds (defaults to current time if `None`).
    pub time: Option<u64>,
    /// Override author name (uses store signature if `None`).
    pub author_name: Option<String>,
    /// Override author email (uses store signature if `None`).
    pub author_email: Option<String>,
}

// ---------------------------------------------------------------------------
// ReflogEntry
// ---------------------------------------------------------------------------

/// A single reflog entry recording a branch movement.
#[derive(Debug, Clone)]
pub struct ReflogEntry {
    /// Previous 40-char hex commit SHA.
    pub old_sha: String,
    /// New 40-char hex commit SHA.
    pub new_sha: String,
    /// Identity string of the committer (e.g. `"vost <vost@localhost>"`).
    pub committer: String,
    /// POSIX epoch seconds of the entry.
    pub timestamp: u64,
    /// Reflog message (e.g. `"commit: + file.txt"`).
    pub message: String,
}

// ---------------------------------------------------------------------------
// RefChange / MirrorDiff
// ---------------------------------------------------------------------------

/// Describes a reference change during backup/restore.
#[derive(Debug, Clone)]
pub struct RefChange {
    /// Full ref name (e.g. `"refs/heads/main"`).
    pub ref_name: String,
    /// Previous target SHA, or `None` for newly created refs.
    pub old_target: Option<String>,
    /// New target SHA, or `None` for deleted refs.
    pub new_target: Option<String>,
}

/// Summary of differences between two repositories (for mirror/backup/restore ops).
#[derive(Debug, Clone, Default)]
pub struct MirrorDiff {
    /// Refs that exist only in the source (newly added).
    pub add: Vec<RefChange>,
    /// Refs that exist in both but point to different commits.
    pub update: Vec<RefChange>,
    /// Refs that exist only in the destination (will be removed).
    pub delete: Vec<RefChange>,
}

impl MirrorDiff {
    /// Create an empty diff.
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when no refs differ between source and destination.
    pub fn in_sync(&self) -> bool {
        self.add.is_empty()
            && self.update.is_empty()
            && self.delete.is_empty()
    }

    /// Total number of ref changes (add + update + delete).
    pub fn total(&self) -> usize {
        self.add.len() + self.update.len() + self.delete.len()
    }
}

// ---------------------------------------------------------------------------
// OpenOptions
// ---------------------------------------------------------------------------

/// Options for opening or creating a `GitStore`.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Create the repository if it doesn't exist.
    pub create: bool,
    /// Default branch name.
    pub branch: Option<String>,
    /// Default author name.
    pub author: Option<String>,
    /// Default author email.
    pub email: Option<String>,
}

// ---------------------------------------------------------------------------
// BackupOptions / RestoreOptions
// ---------------------------------------------------------------------------

/// Options for [`GitStore::backup`].
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// If true, compute diff but do not push.
    pub dry_run: bool,
    /// Limit to specific refs (short names like "main" or full like "refs/heads/main").
    pub refs: Option<Vec<String>>,
    /// Force format: "bundle" for bundle file output. Auto-detected from `.bundle` extension.
    pub format: Option<String>,
    /// Rename refs during backup.  Keys are source ref names, values are
    /// destination ref names (both may be short or full).  When set, takes
    /// precedence over `refs`.
    pub ref_map: Option<std::collections::HashMap<String, String>>,
    /// If true, each ref gets a parentless commit with the same tree,
    /// stripping all history from the exported bundle.
    pub squash: bool,
}

/// Options for [`GitStore::restore`].
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// If true, compute diff but do not fetch.
    pub dry_run: bool,
    /// Limit to specific refs (short names like "main" or full like "refs/heads/main").
    pub refs: Option<Vec<String>>,
    /// Force format: "bundle" for bundle file input. Auto-detected from `.bundle` extension.
    pub format: Option<String>,
    /// Rename refs during restore.  Keys are source ref names, values are
    /// destination ref names (both may be short or full).  When set, takes
    /// precedence over `refs`.
    pub ref_map: Option<std::collections::HashMap<String, String>>,
}
