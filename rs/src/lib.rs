//! A git-backed file store library.
//!
//! `vost` provides a high-level API for using bare git repositories as
//! persistent, versioned key-value stores. Every write is an atomic commit,
//! giving you full history, branching, and rollback for free.
//!
//! # Key types
//!
//! - [`GitStore`] — opens (or creates) a bare git repository and provides
//!   access to branches, tags, and notes.
//! - [`Fs`] — an immutable snapshot of a committed tree. Read operations
//!   never mutate state; write operations return a **new** `Fs` with the
//!   changes committed.
//! - [`Batch`] — accumulates multiple writes and commits them atomically.
//! - [`RefDict`] — dictionary-like access to branches or tags.
//!
//! # Quick example
//!
//! ```rust,no_run
//! use vost::{GitStore, Fs, OpenOptions};
//! use vost::fs::WriteOptions;
//!
//! let store = GitStore::open("/tmp/my-repo", OpenOptions::default()).unwrap();
//! let fs = store.branches().get("main").unwrap();
//!
//! // Read
//! let data = fs.read("hello.txt").unwrap();
//!
//! // Write (returns a new snapshot)
//! let fs2 = fs.write("hello.txt", b"world", WriteOptions::default()).unwrap();
//! assert_eq!(fs2.read_text("hello.txt").unwrap(), "world");
//! ```

pub mod batch;
#[cfg(feature = "cli")]
pub mod cli;
pub mod copy;
pub mod error;
pub mod exclude;
pub mod fileobj;
pub mod fs;
pub mod glob;
pub mod lock;
pub mod mirror;
pub mod notes;
pub mod paths;
pub mod refdict;
pub mod reflog;
pub mod store;
pub mod tree;
pub mod types;

// Re-export primary public types at crate root.
pub use error::{Error, Result};
pub use store::GitStore;
pub use fs::Fs;
pub use batch::Batch;
pub use refdict::RefDict;
pub use notes::{NoteDict, NoteNamespace, NotesBatch};
pub use fileobj::{FsWriter, BatchWriter};
pub use types::*;
pub use copy::{disk_glob, disk_glob_ext};
pub use exclude::ExcludeFilter;
pub use mirror::resolve_credentials;
pub use tree::hash_blob;
