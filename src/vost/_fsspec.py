"""fsspec filesystem adapter for vost."""

from __future__ import annotations

from io import BytesIO

from fsspec.spec import AbstractFileSystem


class VostFileSystem(AbstractFileSystem):
    """An fsspec filesystem backed by a vost (git) repository.

    Parameters:
        repo: Path to the bare git repository.
        ref: Branch name, tag name, or commit hash.  Defaults to the
            repository's current (HEAD) branch.
        back: Number of ancestor commits to walk back (time-travel).
        readonly: If True, block all write operations even on branches.
    """

    protocol = "vost"

    def __init__(self, repo, ref=None, back=0, readonly=False, **kwargs):
        super().__init__(**kwargs)
        from .repo import GitStore

        self._store = GitStore.open(repo, create=False)
        self._readonly = bool(readonly)

        # Resolve ref -> FS snapshot
        if ref is None:
            fs = self._store.branches.current
            if fs is None:
                raise ValueError("no current branch and no ref specified")
        elif ref in self._store.branches:
            fs = self._store.branches[ref]
        elif ref in self._store.tags:
            fs = self._store.tags[ref]
        else:
            # Treat as commit hash
            from .fs import FS

            obj = self._store._repo.get(ref)
            if obj is None or obj.type_num != 1:
                raise ValueError(f"ref not found: {ref!r}")
            fs = FS(self._store, obj.id, writable=False)

        if back:
            fs = fs.back(back)

        self._fs = fs

    def _vpath(self, path):
        """Strip leading / and normalize for vost (empty string = root)."""
        return self._strip_protocol(path).lstrip("/")

    def info(self, path, **kwargs):
        vp = self._vpath(path)
        if not vp:
            st = self._fs.stat()
        else:
            st = self._fs.stat(vp)
        return {
            "name": path.rstrip("/") if vp else "/",
            "size": st.size,
            "type": "directory" if st.file_type.value == "tree" else "file",
            "mode": st.mode,
            "sha": st.hash,
            "mtime": st.mtime,
        }

    def ls(self, path, detail=False, **kwargs):
        vp = self._vpath(path)
        if detail:
            entries = self._fs.listdir(vp if vp else None)
            result = []
            for e in entries:
                from .copy._types import FileType

                name = f"{vp}/{e.name}" if vp else e.name
                ftype = FileType.from_filemode(e.mode)
                is_dir = ftype.value == "tree"
                result.append(
                    {
                        "name": f"/{name}",
                        "size": self._fs.size(name) if not is_dir else 0,
                        "type": "directory" if is_dir else "file",
                        "mode": e.mode,
                    }
                )
            return result
        else:
            names = self._fs.ls(vp if vp else None)
            prefix = f"/{vp}/" if vp else "/"
            return [f"{prefix}{n}" for n in names]

    def _require_writable(self):
        if self._readonly:
            raise PermissionError("filesystem is read-only (readonly=True)")
        if not self._fs.writable:
            raise PermissionError("snapshot is read-only (tag or detached)")

    def _open(self, path, mode="rb", **kwargs):
        vp = self._vpath(path)
        if "r" in mode:
            return BytesIO(self._fs.read(vp))
        elif "w" in mode:
            self._require_writable()
            return _VostWriteFile(self, vp)
        else:
            raise ValueError(f"unsupported mode: {mode}")

    def cat_file(self, path, start=None, end=None, **kwargs):
        vp = self._vpath(path)
        offset = start or 0
        size = (end - offset) if end is not None else None
        return self._fs.read(vp, offset=offset, size=size)

    def pipe_file(self, path, value, **kwargs):
        self._require_writable()
        vp = self._vpath(path)
        self._fs = self._fs.write(vp, value)

    def rm(self, path, recursive=False, maxdepth=None):
        self._require_writable()
        vp = self._vpath(path)
        self._fs = self._fs.remove(vp, recursive=recursive)

    def mkdir(self, path, create_parents=True, **kwargs):
        pass  # git has no empty directories

    def mkdirs(self, path, exist_ok=False):
        pass  # git has no empty directories


class _VostWriteFile(BytesIO):
    """Buffered write file that commits on close."""

    def __init__(self, vfs, path):
        super().__init__()
        self._vfs = vfs
        self._path = path

    def close(self):
        if not self.closed:
            self._vfs._fs = self._vfs._fs.write(self._path, self.getvalue())
        super().close()
