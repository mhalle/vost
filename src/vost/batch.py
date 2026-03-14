"""Batch context manager for vost."""

from __future__ import annotations

import os
from typing import TYPE_CHECKING

from .tree import GIT_FILEMODE_BLOB, GIT_FILEMODE_LINK, GIT_OBJECT_TREE, _mode_from_disk, _normalize_path, _walk_to, exists_at_path

if TYPE_CHECKING:
    from .copy._types import FileType
    from .fs import FS


class Batch:
    """Accumulates writes and removes, committing them in a single atomic commit.

    Use as a context manager or call :meth:`commit` explicitly.  Nothing is
    committed if an exception occurs inside the ``with`` block.

    Attributes:
        fs: The resulting :class:`~vost.FS` after commit, or ``None``
            if uncommitted or aborted.
    """

    def __init__(self, fs: FS, message: str | None = None, operation: str | None = None, parents: list[FS] | None = None):
        if not fs._writable:
            raise fs._readonly_error("batch on")
        self._fs = fs
        self._repo = fs._store._repo
        self._message = message
        self._operation = operation
        self._parents = parents
        self._writes: dict[str, bytes | tuple[bytes, int] | bytes | tuple[bytes, int]] = {}
        self._removes: set[str] = set()
        self._closed = False
        self.fs: FS | None = None

    def _check_open(self) -> None:
        if self._closed:
            raise RuntimeError("Batch is closed")

    def write(self, path: str | os.PathLike[str], data: bytes, *, mode: FileType | int | None = None) -> None:
        """Stage a file write.

        Args:
            path: Destination path in the repo.
            data: Raw bytes to write.
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).
        """
        from .copy._types import FileType
        if isinstance(mode, FileType):
            mode = mode.filemode
        self._check_open()
        path = _normalize_path(path)
        self._removes.discard(path)
        blob_oid = self._repo.create_blob(data)
        self._writes[path] = (blob_oid, mode) if mode is not None else blob_oid

    def write_from_file(self, path: str | os.PathLike[str], local_path: str | os.PathLike[str], *, mode: FileType | int | None = None) -> None:
        """Stage a write from a local file.

        Executable permission is auto-detected from disk unless *mode* is set.

        Args:
            path: Destination path in the repo.
            local_path: Path to the local file.
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).
        """
        from .copy._types import FileType
        if isinstance(mode, FileType):
            mode = mode.filemode
        self._check_open()
        path = _normalize_path(path)
        local_path = os.fspath(local_path)
        self._removes.discard(path)
        detected_mode = _mode_from_disk(local_path)
        if mode is None:
            mode = detected_mode
        blob_oid = self._repo.create_blob_fromdisk(local_path)
        self._writes[path] = (blob_oid, mode) if mode != GIT_FILEMODE_BLOB else blob_oid

    def write_text(self, path: str | os.PathLike[str], text: str, *, encoding: str = "utf-8", mode: FileType | int | None = None) -> None:
        """Stage a text write (convenience wrapper around :meth:`write`).

        Args:
            path: Destination path in the repo.
            text: String content (encoded with *encoding*).
            encoding: Text encoding (default ``"utf-8"``).
            mode: File mode override (e.g. ``FileType.EXECUTABLE``).
        """
        self.write(path, text.encode(encoding), mode=mode)

    def write_symlink(self, path: str | os.PathLike[str], target: str) -> None:
        """Stage a symbolic link entry.

        Args:
            path: Symlink path in the repo.
            target: The symlink target string.
        """
        self._check_open()
        path = _normalize_path(path)
        self._removes.discard(path)
        blob_oid = self._repo.create_blob(target.encode())
        self._writes[path] = (blob_oid, GIT_FILEMODE_LINK)

    def remove(self, path: str | os.PathLike[str]) -> None:
        """Stage a file removal.

        Args:
            path: Path to remove from the repo.

        Raises:
            FileNotFoundError: If *path* does not exist in the repo
                or pending writes.
            IsADirectoryError: If *path* is a directory.
        """
        self._check_open()
        path = _normalize_path(path)
        pending_write = path in self._writes
        repo = self._fs._store._repo
        exists_in_base = exists_at_path(repo, self._fs._tree_oid, path)
        if not pending_write and not exists_in_base:
            raise FileNotFoundError(path)
        # Check for directory in the base tree — even if there's a pending
        # write, we must not add a directory path to _removes.
        if exists_in_base:
            obj = _walk_to(repo, self._fs._tree_oid, path)
            if obj.type_num == GIT_OBJECT_TREE:
                raise IsADirectoryError(path)
        self._writes.pop(path, None)
        if exists_in_base:
            self._removes.add(path)

    def writer(self, path: str | os.PathLike[str], mode: str = "wb"):
        """Return a writable file-like that stages to the batch on close.

        ``"wb"`` accepts bytes; ``"w"`` accepts strings (UTF-8 encoded).

        Example::

            with fs.batch() as b:
                with b.writer("log.txt", "w") as f:
                    f.write("line 1\\n")
                    f.write("line 2\\n")

        Args:
            path: Destination path in the repo.
            mode: ``"wb"`` (binary, default) or ``"w"`` (text).
        """
        self._check_open()
        if mode == "wb":
            from ._fileobj import BatchWritableFile
            return BatchWritableFile(self, path)
        elif mode == "w":
            from ._fileobj import BatchWritableFile
            return BatchWritableFile(self, path, encoding="utf-8")
        else:
            raise ValueError(f"writer() mode must be 'wb' or 'w', got {mode!r}")

    def commit(self) -> FS:
        """Explicitly commit the batch, like ``__exit__`` with no exception.

        After calling this the batch is closed and no further writes are
        allowed.  Returns the resulting ``FS``.
        """
        self._check_open()

        if not self._writes and not self._removes:
            self.fs = self._fs
            self._closed = True
            return self.fs

        self.fs = self._fs._commit_changes(self._writes, self._removes, self._message, self._operation, parents=self._parents)
        self._closed = True
        return self.fs

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is not None:
            self._closed = True
            return False

        if self._closed:
            # Already committed via commit()
            return False

        if not self._writes and not self._removes:
            self.fs = self._fs
            self._closed = True
            return False

        # Let _commit_changes build changes and generate message
        self.fs = self._fs._commit_changes(self._writes, self._removes, self._message, self._operation, parents=self._parents)
        self._closed = True
        return False
