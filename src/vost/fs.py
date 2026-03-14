"""FS: immutable snapshot of a committed tree state."""

from __future__ import annotations

import os
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TYPE_CHECKING, Iterator

from ._glob import _glob_match
from ._lock import repo_lock
from .exceptions import StaleSnapshotError
from .tree import (
    GIT_FILEMODE_BLOB,
    GIT_FILEMODE_BLOB_EXECUTABLE,
    GIT_FILEMODE_LINK,
    GIT_FILEMODE_TREE,
    GIT_OBJECT_TREE,
    WalkEntry,
    _count_subdirs,
    _entry_at_path,
    _is_root_path,
    _mode_from_disk,
    _normalize_path,
    _walk_to,
    read_blob_at_path,
    list_tree_at_path,
    list_entries_at_path,
    walk_tree,
    exists_at_path,
    rebuild_tree,
)

from .copy._types import FileType

if TYPE_CHECKING:
    from ._exclude import ExcludeFilter
    from .copy._types import ChangeReport
    from .repo import GitStore

__all__ = ["FS", "StatResult", "WriteEntry", "retry_write"]


@dataclass(frozen=True, slots=True)
class StatResult:
    """POSIX-like stat result for a vost path.

    Attributes:
        mode: Raw git filemode (e.g. ``0o100644``, ``0o040000``).
        file_type: :class:`FileType` enum value.
        size: Object size in bytes (0 for directories).
        hash: 40-char hex SHA of the object (inode proxy).
        nlink: 1 for files/symlinks, ``2 + subdirs`` for directories.
        mtime: Commit timestamp as POSIX epoch seconds.
    """

    mode: int
    file_type: FileType
    size: int
    hash: str
    nlink: int
    mtime: float


@dataclass(frozen=True, slots=True)
class WriteEntry:
    """Describes a single file write for :meth:`FS.apply`.

    Exactly one of *data* or *target* must be provided.

    *data* may be ``bytes``, ``str`` (UTF-8 text), or a :class:`~pathlib.Path`
    to a local file.  *mode* optionally overrides the filemode
    (e.g. ``FileType.EXECUTABLE``).

    *target* creates a symbolic link entry; *mode* is not allowed with it.
    """

    data: bytes | str | Path | None = None
    mode: FileType | int | None = None
    target: str | None = None

    def __post_init__(self):
        if self.data is not None and self.target is not None:
            raise ValueError("Cannot specify both data and target")
        if self.data is None and self.target is None:
            raise ValueError("Must specify either data or target")
        if self.target is not None and self.mode is not None:
            raise ValueError("Cannot specify mode for symlinks")


class FS:
    """An immutable snapshot of a committed tree.

    Read-only when ``writable`` is False (tag snapshot).
    Writable when ``writable`` is True — writes auto-commit and return a new FS.
    """

    def __init__(self, gitstore: GitStore, commit_oid, ref_name: str | None = None, *, writable: bool | None = None):
        self._store = gitstore
        self._commit_oid = commit_oid
        self._ref_name = ref_name
        self._writable = writable if writable is not None else (ref_name is not None)
        commit = gitstore._repo[commit_oid]
        self._tree_oid = commit.tree
        self._changes = None
        self.__sizer = None
        self.__commit_time = None

    def _readonly_error(self, verb: str) -> PermissionError:
        if self._ref_name:
            return PermissionError(f"Cannot {verb} read-only snapshot (ref {self._ref_name!r})")
        return PermissionError(f"Cannot {verb} read-only snapshot")

    def __repr__(self) -> str:
        short = self._commit_oid.decode()[:7]
        parts = []
        if self._ref_name:
            parts.append(f"ref_name={self._ref_name!r}")
        parts.append(f"commit={short}")
        if not self._writable:
            parts.append("readonly")
        return f"FS({', '.join(parts)})"

    @property
    def writable(self) -> bool:
        """Whether this snapshot can be written to."""
        return self._writable

    @property
    def commit_hash(self) -> str:
        """The 40-character hex SHA of this snapshot's commit."""
        return self._commit_oid.decode()

    @property
    def ref_name(self) -> str | None:
        """The branch or tag name, or ``None`` for detached snapshots."""
        return self._ref_name

    @property
    def message(self) -> str:
        """The commit message (trailing newline stripped)."""
        return self._store._repo[self._commit_oid].message.decode().rstrip("\n")

    @property
    def time(self) -> datetime:
        """Timezone-aware commit timestamp."""
        commit = self._store._repo[self._commit_oid]
        tz = timezone(timedelta(minutes=commit.commit_timezone // 60))
        return datetime.fromtimestamp(commit.commit_time, tz=tz)

    @property
    def author_name(self) -> str:
        """The commit author's name."""
        ident = self._store._repo[self._commit_oid].author.decode()
        name, _, _ = ident.partition(" <")
        return name

    @property
    def author_email(self) -> str:
        """The commit author's email address."""
        ident = self._store._repo[self._commit_oid].author.decode()
        _, _, email_part = ident.partition(" <")
        return email_part.rstrip(">")

    @property
    def changes(self) -> ChangeReport | None:
        """Report of the operation that created this snapshot."""
        return self._changes

    @property
    def _sizer(self):
        if self.__sizer is None:
            from ._objsize import ObjectSizer
            self.__sizer = ObjectSizer(self._store._repo.object_store)
        return self.__sizer

    def _get_commit_time(self) -> float:
        if self.__commit_time is None:
            commit = self._store._repo[self._commit_oid]
            self.__commit_time = float(commit.commit_time)
        return self.__commit_time

    def close(self) -> None:
        """Release cached resources (ObjectSizer file descriptors)."""
        if self.__sizer is not None:
            self.__sizer.close()
            self.__sizer = None

    # --- Read operations ---

    def read(self, path: str | os.PathLike[str], *, offset: int = 0, size: int | None = None) -> bytes:
        """Read file contents as bytes.

        Args:
            path: File path in the repo.
            offset: Byte offset to start reading from.
            size: Maximum number of bytes to return (``None`` for all).

        Raises:
            FileNotFoundError: If *path* does not exist.
            IsADirectoryError: If *path* is a directory.
        """
        data = read_blob_at_path(self._store._repo, self._tree_oid, path)
        if offset or size is not None:
            end = (offset + size) if size is not None else None
            return data[offset:end]
        return data

    def read_text(self, path: str | os.PathLike[str], encoding: str = "utf-8") -> str:
        """Read file contents as a string.

        Args:
            path: File path in the repo.
            encoding: Text encoding (default ``"utf-8"``).
        """
        return self.read(path).decode(encoding)

    def ls(self, path: str | os.PathLike[str] | None = None) -> list[str]:
        """List entry names at *path* (or root if ``None``).

        Args:
            path: Directory path, or ``None`` for the repo root.

        Raises:
            NotADirectoryError: If *path* is a file.
        """
        return list_tree_at_path(self._store._repo, self._tree_oid, path)

    def walk(self, path: str | os.PathLike[str] | None = None) -> Iterator[tuple[str, list[str], list[WalkEntry]]]:
        """Walk the repo tree recursively, like :func:`os.walk`.

        Yields ``(dirpath, dirnames, file_entries)`` tuples.  Each file
        entry is a :class:`WalkEntry` with ``name``, ``oid``, and ``mode``.

        Args:
            path: Subtree to walk, or ``None`` for root.

        Raises:
            NotADirectoryError: If *path* is a file.
        """
        if path is None or _is_root_path(path):
            yield from walk_tree(self._store._repo, self._tree_oid)
        else:
            path = _normalize_path(path)
            obj = _walk_to(self._store._repo, self._tree_oid, path)
            if obj.type_num != GIT_OBJECT_TREE:
                raise NotADirectoryError(path)
            yield from walk_tree(self._store._repo, obj.id, path)

    def exists(self, path: str | os.PathLike[str]) -> bool:
        """Return ``True`` if *path* exists (file or directory)."""
        return exists_at_path(self._store._repo, self._tree_oid, path)

    def is_dir(self, path: str | os.PathLike[str]) -> bool:
        """Return True if *path* is a directory (tree) in the repo."""
        path = _normalize_path(path)
        entry = _entry_at_path(self._store._repo, self._tree_oid, path)
        if entry is None:
            return False
        return entry[1] == GIT_FILEMODE_TREE

    def file_type(self, path: str | os.PathLike[str]) -> FileType:
        """Return the :class:`FileType` of *path*.

        Returns ``FileType.BLOB``, ``FileType.EXECUTABLE``,
        ``FileType.LINK``, or ``FileType.TREE``.

        Raises :exc:`FileNotFoundError` if the path does not exist.
        """
        path = _normalize_path(path)
        entry = _entry_at_path(self._store._repo, self._tree_oid, path)
        if entry is None:
            raise FileNotFoundError(path)
        return FileType.from_filemode(entry[1])

    def size(self, path: str | os.PathLike[str]) -> int:
        """Return the size in bytes of the object at *path*.

        Works without reading the full blob into memory.

        Raises :exc:`FileNotFoundError` if the path does not exist.
        """
        path = _normalize_path(path)
        entry = _entry_at_path(self._store._repo, self._tree_oid, path)
        if entry is None:
            raise FileNotFoundError(path)
        oid, _filemode = entry
        return self._sizer.size(oid)

    def object_hash(self, path: str | os.PathLike[str]) -> str:
        """Return the 40-character hex SHA of the object at *path*.

        For files this is the blob SHA; for directories the tree SHA.

        Raises :exc:`FileNotFoundError` if the path does not exist.
        """
        path = _normalize_path(path)
        entry = _entry_at_path(self._store._repo, self._tree_oid, path)
        if entry is None:
            raise FileNotFoundError(path)
        return entry[0].decode()

    def stat(self, path: str | os.PathLike[str] | None = None) -> StatResult:
        """Return a :class:`StatResult` for *path* (or root if ``None``).

        Combines file_type, size, oid, nlink, and mtime in a single call —
        the hot path for FUSE ``getattr``.
        """
        repo = self._store._repo
        mtime = self._get_commit_time()

        if path is None or _is_root_path(path):
            oid = self._tree_oid
            nlink = 2 + _count_subdirs(repo, oid)
            return StatResult(
                mode=GIT_FILEMODE_TREE,
                file_type=FileType.TREE,
                size=0,
                hash=oid.decode(),
                nlink=nlink,
                mtime=mtime,
            )

        path = _normalize_path(path)
        entry = _entry_at_path(repo, self._tree_oid, path)
        if entry is None:
            raise FileNotFoundError(path)
        oid, filemode = entry

        ft = FileType.from_filemode(filemode)
        if filemode == GIT_FILEMODE_TREE:
            nlink = 2 + _count_subdirs(repo, oid)
            size = 0
        else:
            nlink = 1
            size = self._sizer.size(oid)

        return StatResult(
            mode=filemode,
            file_type=ft,
            size=size,
            hash=oid.decode(),
            nlink=nlink,
            mtime=mtime,
        )

    def listdir(self, path: str | os.PathLike[str] | None = None) -> list[WalkEntry]:
        """List directory entries with name, oid, and mode.

        Like :meth:`ls` but returns :class:`WalkEntry` objects so callers
        get entry types (useful for FUSE ``readdir`` ``d_type``).
        """
        return list_entries_at_path(self._store._repo, self._tree_oid, path)

    @property
    def tree_hash(self) -> str:
        """The 40-char hex SHA of the root tree."""
        return self._tree_oid.decode()

    def read_by_hash(self, hash: str | bytes, *, offset: int = 0, size: int | None = None) -> bytes:
        """Read raw blob data by hash, bypassing tree lookup.

        FUSE pattern: ``stat()`` → cache hash → ``read_by_hash(hash)``.
        """
        if isinstance(hash, str):
            hash = hash.encode()
        data = self._store._repo[hash].data
        if offset or size is not None:
            end = (offset + size) if size is not None else None
            return data[offset:end]
        return data

    def iglob(self, pattern: str) -> Iterator[str]:
        """Expand a glob pattern against the repo tree, yielding unique matches.

        Like :meth:`glob` but returns an unordered iterator instead of a
        sorted list.  Useful when you only need to iterate once and don't
        need sorted output.

        A ``/./`` pivot marker (rsync ``-R`` style) is preserved in the
        output so that callers can reconstruct partial source paths.
        """
        pattern = pattern.strip("/")
        if not pattern:
            return
        pivot_idx = pattern.find("/./")
        if pivot_idx > 0:
            base = pattern[:pivot_idx]
            rest = pattern[pivot_idx + 3:]
            flat = f"{base}/{rest}" if rest else base
            base_prefix = base + "/"
            seen: set[str] = set()
            for path in self._iglob_walk(flat.split("/"), None, self._tree_oid):
                if path not in seen:
                    seen.add(path)
                    yield f"{base}/./{path[len(base_prefix):]}" if path.startswith(base_prefix) else f"{base}/./{path}"
            return
        seen: set[str] = set()
        for path in self._iglob_walk(pattern.split("/"), None, self._tree_oid):
            if path not in seen:
                seen.add(path)
                yield path

    def glob(self, pattern: str) -> list[str]:
        """Expand a glob pattern against the repo tree.

        Supports ``*``, ``?``, and ``**``.  ``*`` and ``?`` do not match
        a leading ``.`` unless the pattern segment itself starts with ``.``.
        ``**`` matches zero or more directory levels, skipping directories
        whose names start with ``.``.
        Returns a sorted, deduplicated list of matching paths (files and directories).
        """
        return sorted(self.iglob(pattern))

    def _iglob_entries(self, tree_oid) -> list[tuple[str, bool, object]]:
        """Return [(name, is_dir, oid), ...] for entries in a tree."""
        repo = self._store._repo
        tree = repo[tree_oid]
        return [(e.path.decode(), e.mode == GIT_FILEMODE_TREE, e.sha) for e in tree.iteritems()]

    def _iglob_walk(self, segments: list[str], prefix: str | None, tree_oid) -> Iterator[str]:
        """Recursive glob generator — carries tree OID to avoid root walks."""
        if not segments:
            return
        seg = segments[0]
        rest = segments[1:]
        repo = self._store._repo

        if seg == "**":
            try:
                entries = self._iglob_entries(tree_oid)
            except (KeyError, TypeError):
                return
            if rest:
                # Zero dirs: match rest[0] against entries we already have
                yield from self._iglob_match_entries(rest, prefix, entries)
            else:
                # ** alone at end: yield non-dot entries at this level
                for name, _is_dir, _oid in entries:
                    if name.startswith("."):
                        continue
                    yield f"{prefix}/{name}" if prefix else name
            # One+ dirs: recurse into non-dot subdirs
            for name, entry_is_dir, oid in entries:
                if name.startswith("."):
                    continue
                full = f"{prefix}/{name}" if prefix else name
                if entry_is_dir:
                    yield from self._iglob_walk(segments, full, oid)  # keep **
            return

        has_wild = "*" in seg or "?" in seg

        if has_wild:
            try:
                entries = self._iglob_entries(tree_oid)
            except (KeyError, TypeError):
                return
            for name, is_dir, oid in entries:
                if not _glob_match(seg, name):
                    continue
                full = f"{prefix}/{name}" if prefix else name
                if rest:
                    if is_dir:
                        yield from self._iglob_walk(rest, full, oid)
                else:
                    yield full
        else:
            # Literal segment — look up directly in current tree
            try:
                tree = repo[tree_oid]
                mode, sha = tree[seg.encode()]
            except (KeyError, TypeError):
                return
            full = f"{prefix}/{seg}" if prefix else seg
            if rest:
                if mode == GIT_FILEMODE_TREE:
                    yield from self._iglob_walk(rest, full, sha)
            else:
                yield full

    def _iglob_match_entries(
        self,
        segments: list[str],
        prefix: str | None,
        entries: list[tuple[str, bool, object]],
    ) -> Iterator[str]:
        """Match segments against already-fetched entries (avoids re-listing)."""
        seg = segments[0]
        rest = segments[1:]
        has_wild = "*" in seg or "?" in seg

        if has_wild:
            for name, _is_dir, oid in entries:
                if not _glob_match(seg, name):
                    continue
                full = f"{prefix}/{name}" if prefix else name
                if rest:
                    yield from self._iglob_walk(rest, full, oid)
                else:
                    yield full
        else:
            # Literal — look up in entries
            for name, _is_dir, oid in entries:
                if name == seg:
                    full = f"{prefix}/{seg}" if prefix else seg
                    if rest:
                        yield from self._iglob_walk(rest, full, oid)
                    else:
                        yield full
                    return

    def readlink(self, path: str | os.PathLike[str]) -> str:
        """Read the target of a symlink."""
        path = _normalize_path(path)
        entry = _entry_at_path(self._store._repo, self._tree_oid, path)
        if entry is None:
            raise FileNotFoundError(path)
        _oid, filemode = entry
        if filemode != GIT_FILEMODE_LINK:
            raise ValueError(f"Not a symlink: {path}")
        return self._store._repo[_oid].data.decode()

    def writer(self, path: str | os.PathLike[str], mode: str = "wb"):
        """Return a writable file-like that commits on close.

        ``"wb"`` accepts bytes; ``"w"`` accepts strings (UTF-8 encoded).

        Example::

            with fs.writer("output.bin") as f:
                f.write(b"chunk1")
                f.write(b"chunk2")
            fs = f.fs  # new snapshot

        Args:
            path: Destination path in the repo.
            mode: ``"wb"`` (binary, default) or ``"w"`` (text).

        Raises:
            PermissionError: If the snapshot is read-only.
        """
        if not self._writable:
            raise self._readonly_error("write to")
        if mode == "wb":
            from ._fileobj import WritableFile
            return WritableFile(self, str(path))
        elif mode == "w":
            from ._fileobj import WritableFile
            return WritableFile(self, str(path), encoding="utf-8")
        else:
            raise ValueError(f"writer() mode must be 'wb' or 'w', got {mode!r}")

    # --- Write operations ---

    def _build_changes(
        self,
        writes: dict[str, bytes | tuple[bytes, int] | bytes | tuple[bytes, int]],
        removes: set[str],
    ):
        """Build ChangeReport from writes and removes with type detection."""
        from .copy._types import ChangeReport, FileEntry, FileType

        repo = self._store._repo
        add_entries = []
        update_entries = []

        for path, value in writes.items():
            # Extract data/oid and mode from value
            if isinstance(value, tuple):
                data_or_oid, mode = value
            else:
                data_or_oid, mode = value, GIT_FILEMODE_BLOB

            existing = _entry_at_path(repo, self._tree_oid, path)
            if existing is not None:
                # Compare OID + mode to skip unchanged files
                existing_oid, existing_mode = existing
                from .tree import BlobOid
                if isinstance(data_or_oid, BlobOid):
                    new_oid = data_or_oid
                else:
                    new_oid = repo.create_blob(data_or_oid)
                if new_oid == existing_oid and mode == existing_mode:
                    continue  # identical — not a real update
                update_entries.append(FileEntry.from_mode(path, mode))
            else:
                add_entries.append(FileEntry.from_mode(path, mode))

        # For deletes, query the repo to get types before deletion
        delete_entries = []
        for path in removes:
            entry = _entry_at_path(repo, self._tree_oid, path)
            if entry:
                file_entry = FileEntry.from_mode(path, entry[1])
                delete_entries.append(file_entry)
            else:
                # Shouldn't happen, but handle gracefully
                delete_entries.append(FileEntry(path, FileType.BLOB))

        return ChangeReport(add=add_entries, update=update_entries, delete=delete_entries)

    def _commit_changes(
        self,
        writes: dict[str, bytes | tuple[bytes, int] | bytes | tuple[bytes, int]],
        removes: set[str],
        message: str | None,
        operation: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        if not self._writable:
            raise self._readonly_error("write to")

        from .copy._types import format_commit_message

        from .tree import BlobOid

        repo = self._store._repo
        sig = self._store._signature

        # Pre-hash raw bytes into BlobOid so both _build_changes and
        # rebuild_tree see BlobOid values and skip duplicate create_blob.
        pre_writes: dict[str, bytes | tuple[bytes, int]] = {}
        for path, value in writes.items():
            if isinstance(value, tuple):
                data_or_oid, mode = value
                if not isinstance(data_or_oid, BlobOid):
                    pre_writes[path] = (BlobOid(repo.create_blob(data_or_oid)), mode)
                else:
                    pre_writes[path] = value
            else:
                if not isinstance(value, BlobOid):
                    pre_writes[path] = BlobOid(repo.create_blob(value))
                else:
                    pre_writes[path] = value
        writes = pre_writes

        # Build changes
        changes = self._build_changes(writes, removes)

        # Generate message if not provided
        final_message = format_commit_message(changes, message, operation)

        new_tree_oid = rebuild_tree(repo, self._tree_oid, writes, removes)

        # Atomic check-and-update under file lock
        ref_name = f"refs/heads/{self._ref_name}"
        with repo_lock(repo.path):
            ref = repo.references[ref_name]
            if ref.resolve().target != self._commit_oid:
                raise StaleSnapshotError(
                    f"Branch {self._ref_name!r} has advanced since this snapshot"
                )

            if new_tree_oid == self._tree_oid:
                return self  # nothing changed, branch is current

            # Create commit object and move the ref
            parent_oids = [self._commit_oid]
            if parents:
                for p in parents:
                    parent_oids.append(p._commit_oid)
            new_commit_oid = repo.create_commit(
                None,
                sig,
                sig,
                final_message,
                new_tree_oid,
                parent_oids,
            )
            # Pass commit message to reflog
            ref.set_target(new_commit_oid, message=f"commit: {final_message}".encode(), committer=sig._identity)

        new_fs = FS(self._store, new_commit_oid, ref_name=self._ref_name, writable=self._writable)
        new_fs._changes = changes
        return new_fs

    def write(
        self,
        path: str | os.PathLike[str],
        data: bytes,
        *,
        message: str | None = None,
        mode: FileType | int | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Write *data* to *path* and commit, returning a new :class:`FS`.

        Args:
            path: Destination path in the repo.
            data: Raw bytes to write.
            message: Commit message (auto-generated if ``None``).
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).

        Raises:
            PermissionError: If this snapshot is read-only.
            StaleSnapshotError: If the branch has advanced since this snapshot.
        """
        from .copy._types import FileType
        if isinstance(mode, FileType):
            mode = mode.filemode
        path = _normalize_path(path)
        value: bytes | tuple[bytes, int] = (data, mode) if mode is not None else data
        return self._commit_changes({path: value}, set(), message, parents=parents)

    def write_text(
        self,
        path: str | os.PathLike[str],
        text: str,
        *,
        encoding: str = "utf-8",
        message: str | None = None,
        mode: FileType | int | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Write *text* to *path* and commit, returning a new :class:`FS`.

        Args:
            path: Destination path in the repo.
            text: String content (encoded with *encoding*).
            encoding: Text encoding (default ``"utf-8"``).
            message: Commit message (auto-generated if ``None``).
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).

        Raises:
            PermissionError: If this snapshot is read-only.
            StaleSnapshotError: If the branch has advanced since this snapshot.
        """
        return self.write(path, text.encode(encoding), message=message, mode=mode, parents=parents)

    def write_from_file(
        self,
        path: str | os.PathLike[str],
        local_path: str | os.PathLike[str],
        *,
        message: str | None = None,
        mode: FileType | int | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Write a local file into the repo and commit, returning a new :class:`FS`.

        Executable permission is auto-detected from disk unless *mode* is set.

        Args:
            path: Destination path in the repo.
            local_path: Path to the local file.
            message: Commit message (auto-generated if ``None``).
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).

        Raises:
            PermissionError: If this snapshot is read-only.
            StaleSnapshotError: If the branch has advanced since this snapshot.
        """
        from .copy._types import FileType
        if isinstance(mode, FileType):
            mode = mode.filemode
        path = _normalize_path(path)
        local_path = os.fspath(local_path)
        detected_mode = _mode_from_disk(local_path)
        if mode is None:
            mode = detected_mode
        repo = self._store._repo
        blob_oid = repo.create_blob_fromdisk(local_path)
        value: bytes | tuple[bytes, int] = (blob_oid, mode) if mode != GIT_FILEMODE_BLOB else blob_oid
        return self._commit_changes({path: value}, set(), message, parents=parents)

    def write_symlink(
        self,
        path: str | os.PathLike[str],
        target: str,
        *,
        message: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Create a symbolic link entry and commit, returning a new :class:`FS`.

        Args:
            path: Symlink path in the repo.
            target: The symlink target string.
            message: Commit message (auto-generated if ``None``).

        Raises:
            PermissionError: If this snapshot is read-only.
            StaleSnapshotError: If the branch has advanced since this snapshot.
        """
        path = _normalize_path(path)
        data = target.encode()
        return self._commit_changes(
            {path: (data, GIT_FILEMODE_LINK)}, set(),
            message, parents=parents,
        )

    def apply(
        self,
        writes: dict[str, WriteEntry | bytes | str | Path] | None = None,
        removes: str | list[str] | set[str] | None = None,
        *,
        message: str | None = None,
        operation: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Apply multiple writes and removes in a single atomic commit.

        *writes* maps repo paths to content.  Values may be:

        - ``bytes`` — raw blob data
        - ``str`` — UTF-8 text (encoded automatically)
        - :class:`~pathlib.Path` — read from local file (mode auto-detected)
        - :class:`WriteEntry` — full control over source, mode, and symlinks

        *removes* lists repo paths to delete (``str``, ``list``, or ``set``).

        Returns a new :class:`FS` snapshot with the changes committed.
        """
        from .copy._types import FileType

        repo = self._store._repo
        internal_writes: dict[str, bytes | tuple[bytes, int] | bytes | tuple[bytes, int]] = {}

        for path, value in (writes or {}).items():
            path = _normalize_path(path)

            # Wrap bare values into WriteEntry
            if isinstance(value, (bytes, str, Path)):
                value = WriteEntry(data=value)

            if not isinstance(value, WriteEntry):
                raise TypeError(
                    f"Expected WriteEntry, bytes, str, or Path for {path!r}, "
                    f"got {type(value).__name__}"
                )

            if value.target is not None:
                # Symlink entry
                blob_oid = repo.create_blob(value.target.encode())
                internal_writes[path] = (blob_oid, GIT_FILEMODE_LINK)
            elif isinstance(value.data, Path):
                # Local file
                local_path = os.fspath(value.data)
                mode = value.mode
                if isinstance(mode, FileType):
                    mode = mode.filemode
                if mode is None:
                    mode = _mode_from_disk(local_path)
                blob_oid = repo.create_blob_fromdisk(local_path)
                internal_writes[path] = (blob_oid, mode) if mode != GIT_FILEMODE_BLOB else blob_oid
            else:
                # bytes or str data
                data = value.data
                if isinstance(data, str):
                    data = data.encode("utf-8")
                mode = value.mode
                if isinstance(mode, FileType):
                    mode = mode.filemode
                if mode is not None:
                    blob_oid = repo.create_blob(data)
                    internal_writes[path] = (blob_oid, mode)
                else:
                    internal_writes[path] = data

        # Normalize removes
        if removes is None:
            remove_set: set[str] = set()
        elif isinstance(removes, str):
            remove_set = {_normalize_path(removes)}
        else:
            remove_set = {_normalize_path(r) for r in removes}

        return self._commit_changes(internal_writes, remove_set, message, operation, parents=parents)

    def batch(self, message: str | None = None, operation: str | None = None, parents: list[FS] | None = None):
        """Return a :class:`Batch` context manager for multiple writes in one commit.

        Args:
            message: Commit message (auto-generated if ``None``).
            operation: Operation name for auto-generated messages.
            parents: Additional parent :class:`FS` snapshots (advisory merge parents).

        Raises:
            PermissionError: If this snapshot is read-only.
        """
        from .batch import Batch
        return Batch(self, message=message, operation=operation, parents=parents)

    # --- Copy / Sync / Remove / Move ---

    def copy_in(
        self,
        sources: str | list[str],
        dest: str,
        *,
        dry_run: bool = False,
        follow_symlinks: bool = False,
        message: str | None = None,
        mode: FileType | int | None = None,
        ignore_existing: bool = False,
        delete: bool = False,
        ignore_errors: bool = False,
        checksum: bool = True,
        exclude: ExcludeFilter | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Copy local files into the repo.

        Sources must be literal paths; use :func:`~vost.disk_glob` to
        expand patterns before calling.

        Args:
            sources: Local path(s). Trailing ``/`` copies contents; ``/./``
                is a pivot marker.
            dest: Destination path in the repo.
            dry_run: Preview only; returned FS has ``.changes`` set.
            follow_symlinks: Dereference symlinks on disk.
            message: Commit message (auto-generated if ``None``).
            mode: Override file mode for all files.
            ignore_existing: Skip files that already exist at dest.
            delete: Remove repo files under *dest* not in source.
            ignore_errors: Collect errors instead of aborting.
            checksum: Compare by content hash (default ``True``).
            exclude: Gitignore-style exclude filter.

        Returns:
            A new :class:`FS` with ``.changes`` set.

        Raises:
            PermissionError: If this snapshot is read-only.
        """
        from .copy._ops import _copy_in
        return _copy_in(
            self, sources, dest, dry_run=dry_run,
            follow_symlinks=follow_symlinks, message=message, mode=mode,
            ignore_existing=ignore_existing, delete=delete,
            ignore_errors=ignore_errors, checksum=checksum, exclude=exclude,
            parents=parents,
        )

    def copy_out(
        self,
        sources: str | list[str],
        dest: str,
        *,
        dry_run: bool = False,
        ignore_existing: bool = False,
        delete: bool = False,
        ignore_errors: bool = False,
        checksum: bool = True,
    ) -> FS:
        """Copy repo files to local disk.

        Sources must be literal repo paths; use :meth:`glob` to expand
        patterns before calling.

        Args:
            sources: Repo path(s). Trailing ``/`` copies contents; ``/./``
                is a pivot marker.
            dest: Local destination directory.
            dry_run: Preview only; returned FS has ``.changes`` set.
            ignore_existing: Skip files that already exist at dest.
            delete: Remove local files under *dest* not in source.
            ignore_errors: Collect errors instead of aborting.
            checksum: Compare by content hash (default ``True``).

        Returns:
            This :class:`FS` with ``.changes`` set.
        """
        from .copy._ops import _copy_out
        return _copy_out(
            self, sources, dest, dry_run=dry_run,
            ignore_existing=ignore_existing, delete=delete,
            ignore_errors=ignore_errors, checksum=checksum,
        )

    def sync_in(
        self,
        local_path: str,
        repo_path: str,
        *,
        dry_run: bool = False,
        message: str | None = None,
        ignore_errors: bool = False,
        checksum: bool = True,
        exclude: ExcludeFilter | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Make *repo_path* identical to *local_path* (including deletes).

        Args:
            local_path: Local directory to sync from.
            repo_path: Repo directory to sync to.
            dry_run: Preview only; returned FS has ``.changes`` set.
            message: Commit message (auto-generated if ``None``).
            ignore_errors: Collect errors instead of aborting.
            checksum: Compare by content hash (default ``True``).
            exclude: Gitignore-style exclude filter.

        Returns:
            A new :class:`FS` with ``.changes`` set.

        Raises:
            PermissionError: If this snapshot is read-only.
        """
        from .copy._ops import _sync_in
        return _sync_in(
            self, local_path, repo_path, dry_run=dry_run,
            message=message, ignore_errors=ignore_errors,
            checksum=checksum, exclude=exclude,
            parents=parents,
        )

    def sync_out(
        self,
        repo_path: str,
        local_path: str,
        *,
        dry_run: bool = False,
        ignore_errors: bool = False,
        checksum: bool = True,
    ) -> FS:
        """Make *local_path* identical to *repo_path* (including deletes).

        Args:
            repo_path: Repo directory to sync from.
            local_path: Local directory to sync to.
            dry_run: Preview only; returned FS has ``.changes`` set.
            ignore_errors: Collect errors instead of aborting.
            checksum: Compare by content hash (default ``True``).

        Returns:
            This :class:`FS` with ``.changes`` set.
        """
        from .copy._ops import _sync_out
        return _sync_out(
            self, repo_path, local_path, dry_run=dry_run,
            ignore_errors=ignore_errors, checksum=checksum,
        )

    def remove(
        self,
        sources: str | list[str],
        *,
        recursive: bool = False,
        dry_run: bool = False,
        message: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Remove files from the repo.

        Sources must be literal paths; use :meth:`glob` to expand patterns
        before calling.

        Args:
            sources: Repo path(s) to remove.
            recursive: Allow removing directories.
            dry_run: Preview only; returned FS has ``.changes`` set.
            message: Commit message (auto-generated if ``None``).

        Returns:
            A new :class:`FS` with ``.changes`` set.

        Raises:
            PermissionError: If this snapshot is read-only.
            FileNotFoundError: If no source paths match.
        """
        from .copy._ops import _remove
        return _remove(
            self, sources, dry_run=dry_run,
            recursive=recursive, message=message,
            parents=parents,
        )

    def move(
        self,
        sources: str | list[str],
        dest: str,
        *,
        recursive: bool = False,
        dry_run: bool = False,
        message: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Move or rename files within the repo.

        Sources must be literal paths; use :meth:`glob` to expand patterns
        before calling.

        Args:
            sources: Repo path(s) to move.
            dest: Destination path in the repo.
            recursive: Allow moving directories.
            dry_run: Preview only; returned FS has ``.changes`` set.
            message: Commit message (auto-generated if ``None``).

        Returns:
            A new :class:`FS` with ``.changes`` set.

        Raises:
            PermissionError: If this snapshot is read-only.
        """
        from .copy._ops import _move
        return _move(
            self, sources, dest, dry_run=dry_run,
            recursive=recursive, message=message,
            parents=parents,
        )

    def copy_from_ref(
        self,
        source: FS | str,
        sources: str | list[str] = "",
        dest: str = "",
        *,
        delete: bool = False,
        dry_run: bool = False,
        message: str | None = None,
        parents: list[FS] | None = None,
    ) -> FS:
        """Copy files from *source* into this branch in a single atomic commit.

        Follows the same rsync trailing-slash conventions as
        ``copy_in``/``copy_out``:

        - ``"config"``  → directory mode — copies ``config/`` *as* ``config/``
          under *dest*.
        - ``"config/"`` → contents mode — pours the *contents* of ``config/``
          into *dest*.
        - ``"file.txt"`` → file mode — copies the single file into *dest*.
        - ``""`` or ``"/"`` → root contents mode — copies everything.

        Since both snapshots share the same object store, blobs are referenced
        by OID — no data is read into memory regardless of file size.

        Args:
            source: Any FS (branch, tag, detached commit), or a branch/tag
                name string that will be resolved to an FS. Read-only; not
                modified.
            sources: Source path(s) in *source*. Accepts a single string or a
                list of strings.  Defaults to ``""`` (root = everything).
            dest: Destination path in this branch.  Defaults to ``""`` (root).
            delete: Remove dest files under the target that aren't in source.
            dry_run: Compute changes but don't commit. Returned FS has ``.changes`` set.
            message: Commit message (auto-generated if ``None``).

        Returns:
            A new :class:`FS` for the dest branch with the commit applied.

        Raises:
            ValueError: If *source* belongs to a different repo or cannot be resolved.
            FileNotFoundError: If a source path does not exist.
            PermissionError: If this FS is read-only.
        """
        from .copy._resolve import _resolve_repo_sources, _walk_repo
        from .copy._types import _finalize_changes
        from .tree import BlobOid

        # Resolve string to FS
        if isinstance(source, str):
            store = self._store
            try:
                source = store.branches[source]
            except KeyError:
                try:
                    source = store.tags[source]
                except KeyError:
                    raise ValueError(
                        f"Cannot resolve '{source}': not a branch or tag"
                    )

        # Validate same repo
        try:
            same = os.path.samefile(source._store._repo.path, self._store._repo.path)
        except OSError:
            same = False
        if not same:
            raise ValueError("source must belong to the same repo as self")

        # Normalize sources to list
        if isinstance(sources, str):
            sources_list = [sources]
        else:
            sources_list = list(sources)

        # Normalize dest
        if dest:
            dest = _normalize_path(dest)

        # Resolve sources using rsync conventions
        resolved = _resolve_repo_sources(source, sources_list)

        # Enumerate source files → {dest_path: (oid, mode)}
        src_mapped: dict[str, tuple[bytes, int]] = {}
        for repo_path, mode, prefix in resolved:
            _dest = "/".join(p for p in (dest, prefix) if p)

            if mode == "file":
                name = repo_path.rsplit("/", 1)[-1]
                dest_file = f"{_dest}/{name}" if _dest else name
                dest_file = _normalize_path(dest_file)
                entry = source.stat(repo_path)
                src_mapped[dest_file] = (entry.hash.encode(), entry.mode)
            elif mode == "dir":
                dirname = repo_path.rsplit("/", 1)[-1]
                target = f"{_dest}/{dirname}" if _dest else dirname
                for dirpath, _dirs, files in source.walk(repo_path):
                    for fe in files:
                        store_path = f"{dirpath}/{fe.name}" if dirpath else fe.name
                        if repo_path and store_path.startswith(repo_path + "/"):
                            rel = store_path[len(repo_path) + 1:]
                        else:
                            rel = store_path
                        dest_file = _normalize_path(f"{target}/{rel}")
                        src_mapped[dest_file] = (fe.oid, fe.mode)
            elif mode == "contents":
                walk_path = repo_path or None
                for dirpath, _dirs, files in source.walk(walk_path):
                    for fe in files:
                        store_path = f"{dirpath}/{fe.name}" if dirpath else fe.name
                        if repo_path and store_path.startswith(repo_path + "/"):
                            rel = store_path[len(repo_path) + 1:]
                        else:
                            rel = store_path
                        dest_file = f"{_dest}/{rel}" if _dest else rel
                        dest_file = _normalize_path(dest_file)
                        src_mapped[dest_file] = (fe.oid, fe.mode)

        # Determine the dest subtree(s) to walk for delete support.
        # For delete mode we need to know what's currently under the dest
        # area(s) so we can remove files not present in source.
        dest_files: dict[str, tuple[bytes, int]] = {}
        if delete or src_mapped:
            # Walk all destination areas that are covered by source mappings
            dest_prefixes: set[str] = set()
            for repo_path, mode, prefix in resolved:
                _dest = "/".join(p for p in (dest, prefix) if p)
                if mode == "dir":
                    dirname = repo_path.rsplit("/", 1)[-1]
                    dest_prefixes.add(f"{_dest}/{dirname}" if _dest else dirname)
                else:
                    dest_prefixes.add(_dest)

            for dp in dest_prefixes:
                for rel, entry in _walk_repo(self, dp).items():
                    full = f"{dp}/{rel}" if dp else rel
                    dest_files[full] = entry

        # Build writes and removes
        writes: dict[str, tuple[bytes, int]] = {}
        removes: set[str] = set()

        for dest_path, (oid, mode) in src_mapped.items():
            dest_entry = dest_files.get(dest_path)
            if dest_entry is None or dest_entry != (oid, mode):
                writes[dest_path] = (BlobOid(oid), mode)

        if delete:
            for full in dest_files:
                if full not in src_mapped:
                    removes.add(full)

        if dry_run:
            changes = self._build_changes(writes, removes)
            self._changes = _finalize_changes(changes)
            return self

        return self._commit_changes(writes, removes, message, operation="cp", parents=parents)

    # --- History ---

    @property
    def parent(self) -> FS | None:
        """The parent snapshot, or ``None`` for the initial commit."""
        commit = self._store._repo[self._commit_oid]
        if not commit.parents:
            return None
        return FS(self._store, commit.parents[0], ref_name=self._ref_name, writable=self._writable)

    def back(self, n: int = 1) -> FS:
        """Return the FS at the *n*-th ancestor commit.

        Raises ValueError if *n* < 0 or history is too short.
        """
        if n < 0:
            raise ValueError(f"back() requires n >= 0, got {n}")
        fs = self
        for _ in range(n):
            p = fs.parent
            if p is None:
                raise ValueError(
                    f"Cannot go back {n} commits — history too short")
            fs = p
        return fs

    def undo(self, steps: int = 1) -> FS:
        """Move branch back N commits.

        Walks back through parent commits and updates the branch pointer.
        Automatically writes a reflog entry.

        Args:
            steps: Number of commits to undo (default 1)

        Returns:
            New FS snapshot at the parent commit

        Raises:
            PermissionError: If called on read-only snapshot (tag)
            ValueError: If not enough history exists

        Example:
            >>> fs = repo.branches["main"]
            >>> fs = fs.undo()  # Go back 1 commit
            >>> fs = fs.undo(3)  # Go back 3 commits
        """
        if steps < 1:
            raise ValueError(f"steps must be >= 1, got {steps}")
        if not self._writable:
            raise self._readonly_error("undo on")

        # Walk back N parents (safe to do outside the lock — read-only)
        current = self
        for i in range(steps):
            if current.parent is None:
                raise ValueError(
                    f"Cannot undo {steps} steps - only {i} commit(s) in history"
                )
            current = current.parent

        # Atomic stale-check + ref update under a single lock
        repo = self._store._repo
        ref_name = f"refs/heads/{self._ref_name}"
        with repo_lock(repo.path):
            ref = repo.references[ref_name]
            if ref.resolve().target != self._commit_oid:
                raise StaleSnapshotError(
                    f"Branch {self._ref_name!r} has advanced since this snapshot"
                )
            ref.set_target(current._commit_oid, message=b"undo: move back", committer=self._store._signature._identity)

        return current

    def redo(self, steps: int = 1) -> FS:
        """Move branch forward N steps using reflog.

        Reads the reflog to find where the branch was before the last N movements.
        This can resurrect "orphaned" commits after undo.

        The reflog tracks all branch movements chronologically. Each redo step
        moves back one entry in the reflog (backwards in time through the log,
        but forward in commit history).

        Args:
            steps: Number of reflog entries to go back (default 1)

        Returns:
            New FS snapshot at the target position

        Raises:
            PermissionError: If called on read-only snapshot (tag)
            ValueError: If not enough redo history exists

        Example:
            >>> fs = fs.undo(2)  # Creates 1 reflog entry moving back 2 commits
            >>> fs = fs.redo()   # Go back 1 reflog entry (to before the undo)
        """
        if steps < 1:
            raise ValueError(f"steps must be >= 1, got {steps}")
        if not self._writable:
            raise self._readonly_error("redo on")

        # Early stale check (fast-fail; authoritative check under lock below)
        ref_name = f"refs/heads/{self._ref_name}"
        ref = self._store._repo.references[ref_name]
        if ref.resolve().target != self._commit_oid:
            raise StaleSnapshotError(
                f"Branch {self._ref_name!r} has advanced since this snapshot"
            )

        # Read reflog for this branch (safe to do outside the lock — read-only)
        ref_bytes = f"refs/heads/{self._ref_name}".encode()
        entries = list(self._store._repo._drepo.read_reflog(ref_bytes))
        if not entries:
            raise ValueError(f"No reflog found for branch {self._ref_name!r}")

        # Find current position in reflog (search backwards to get most recent)
        current_sha = self._commit_oid
        current_index = None

        for i in range(len(entries) - 1, -1, -1):
            if entries[i].new_sha == current_sha:
                current_index = i
                break

        if current_index is None:
            raise ValueError(
                f"Cannot redo - current commit not in reflog (you may have a stale snapshot)"
            )

        # To redo, we want to go to where the branch was N steps ago
        # Each step back in the reflog shows us old_sha (where it was before that movement)
        # So we walk back N steps, taking the old_sha at each step
        target_sha = current_sha
        index = current_index

        from dulwich.protocol import ZERO_SHA as _ZERO_SHA

        for step in range(steps):
            if index < 0:
                raise ValueError(
                    f"Cannot redo {steps} steps - only {step} step(s) available"
                )
            target_sha = entries[index].old_sha
            if target_sha == _ZERO_SHA:
                raise ValueError(
                    f"Cannot redo {steps} step(s) — reaches branch creation point (no prior commit)"
                )
            index -= 1

        target_fs = FS(self._store, target_sha, ref_name=self._ref_name, writable=self._writable)

        # Atomic stale-check + ref update under a single lock
        repo = self._store._repo
        ref_name = f"refs/heads/{self._ref_name}"
        with repo_lock(repo.path):
            ref = repo.references[ref_name]
            if ref.resolve().target != self._commit_oid:
                raise StaleSnapshotError(
                    f"Branch {self._ref_name!r} has advanced since this snapshot"
                )
            ref.set_target(target_sha, message=b"redo: move forward", committer=self._store._signature._identity)

        return target_fs

    def log(
        self,
        path: str | os.PathLike[str] | None = None,
        *,
        match: str | None = None,
        before: datetime | None = None,
    ) -> Iterator[FS]:
        """Walk the commit history, yielding ancestor :class:`FS` snapshots.

        All filters are optional and combine with AND.

        Args:
            path: Only yield commits that changed this file.
            match: Message pattern (``*``/``?`` wildcards via :func:`fnmatch`).
            before: Only yield commits on or before this time.
        """
        if before is not None and before.tzinfo is None:
            before = before.replace(tzinfo=timezone.utc)
        filter_path = path
        if filter_path is not None:
            filter_path = _normalize_path(filter_path)
        repo = self._store._repo
        if match is not None:
            from fnmatch import fnmatch as _fnmatch
        past_cutoff = False
        current: FS | None = self
        while current is not None:
            if not past_cutoff and before is not None:
                if current.time > before:
                    current = current.parent
                    continue
                past_cutoff = True
            if filter_path is not None:
                current_entry = _entry_at_path(repo, current._tree_oid, filter_path)
                parent = current.parent
                parent_entry = _entry_at_path(repo, parent._tree_oid, filter_path) if parent else None
                if current_entry == parent_entry:
                    current = current.parent
                    continue
            if match is not None and not _fnmatch(current.message, match):
                current = current.parent
                continue
            yield current
            current = current.parent


def retry_write(
    store: GitStore,
    branch: str,
    path: str | os.PathLike[str],
    data: bytes,
    *,
    message: str | None = None,
    mode: FileType | int | None = None,
    retries: int = 5,
    parents: list[FS] | None = None,
) -> FS:
    """Write data to a branch with automatic retry on concurrent modification.

    Re-fetches the branch FS on each attempt.  Uses exponential backoff
    with jitter (base 10ms, factor 2x, cap 200ms) to avoid thundering-herd.

    Raises ``StaleSnapshotError`` if all attempts are exhausted.
    Raises ``KeyError`` if the branch does not exist.
    """
    import random
    import time

    for attempt in range(retries):
        fs = store.branches[branch]
        try:
            return fs.write(path, data, message=message, mode=mode, parents=parents)
        except StaleSnapshotError:
            if attempt == retries - 1:
                raise
            delay = min(0.01 * (2 ** attempt), 0.2)
            time.sleep(random.uniform(0, delay))
