"""GitStore: repository and ref management."""

from __future__ import annotations

import os
import time as _time
from collections.abc import Callable, Iterator, MutableMapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from dulwich.objects import Blob as _DBlob
from dulwich.objects import Commit as _DCommit
from dulwich.objects import Tag as _DTag
from dulwich.objects import Tree as _DTree
from dulwich.repo import Repo as _DRepo

from .exceptions import StaleSnapshotError
from .mirror import RefChange, MirrorDiff
from .notes import NoteDict
from .tree import BlobOid, GitError, TreeBuilder

if TYPE_CHECKING:
    from .fs import FS


# ---------------------------------------------------------------------------
# Signature
# ---------------------------------------------------------------------------

@dataclass
class Signature:
    """Author/committer identity used for commits.

    Attributes:
        name: Author name (e.g. ``"vost"``).
        email: Author email (e.g. ``"vost@localhost"``).
    """

    name: str
    email: str
    _identity: bytes = field(init=False, repr=False, compare=False)

    def __post_init__(self):
        self._identity = f"{self.name} <{self.email}>".encode()


# ---------------------------------------------------------------------------
# References
# ---------------------------------------------------------------------------

class _Reference:
    """Wraps a dulwich ref."""

    def __init__(self, refs_container, ref_name: bytes, repo):
        self._refs = refs_container
        self._name = ref_name

    def resolve(self) -> _Reference:
        return self

    @property
    def target(self) -> bytes:
        return self._refs[self._name]

    def set_target(self, oid: bytes, message: bytes | None = None, committer: bytes | None = None):
        try:
            old_sha = self._refs[self._name]
        except KeyError:
            old_sha = None
        if message is None:
            message = b"update ref"
        if committer is None:
            committer = b"vost <vost@localhost>"
        ok = self._refs.set_if_equals(
            self._name, old_sha, oid,
            committer=committer, message=message,
        )
        if not ok:
            raise StaleSnapshotError(f"CAS failed updating {self._name!r}")


class _References:
    """Wraps dulwich refs to match repo.references API."""

    def __init__(self, dulwich_repo: _DRepo):
        self._dulwich_repo = dulwich_repo
        self._refs = dulwich_repo.refs

    def __getitem__(self, name: str) -> _Reference:
        ref_bytes = name.encode() if isinstance(name, str) else name
        if ref_bytes not in self._refs:
            raise KeyError(name)
        return _Reference(self._refs, ref_bytes, self._dulwich_repo)

    def __contains__(self, name: str) -> bool:
        ref_bytes = name.encode() if isinstance(name, str) else name
        return ref_bytes in self._refs

    def __iter__(self):
        for ref_bytes in self._refs.allkeys():
            yield ref_bytes.decode()

    def create(self, name: str, oid: bytes, message: bytes | None = None, committer: bytes | None = None):
        ref_bytes = name.encode() if isinstance(name, str) else name
        if message is None:
            message = b"create ref"
        if committer is None:
            committer = b"vost <vost@localhost>"
        ok = self._refs.set_if_equals(
            ref_bytes, None, oid,
            committer=committer, message=message,
        )
        if not ok:
            raise StaleSnapshotError(f"CAS failed creating {ref_bytes!r}")

    def delete(self, name: str):
        ref_bytes = name.encode() if isinstance(name, str) else name
        del self._refs[ref_bytes]


# ---------------------------------------------------------------------------
# Repository
# ---------------------------------------------------------------------------

class _Repository:
    """Thin wrapper around dulwich Repo."""

    def __init__(self, path_or_repo):
        if isinstance(path_or_repo, str):
            self._drepo = _DRepo(path_or_repo)
        elif isinstance(path_or_repo, _DRepo):
            self._drepo = path_or_repo
        else:
            self._drepo = path_or_repo

    @property
    def path(self) -> str:
        p = self._drepo.path
        if os.path.isdir(p) and not p.endswith("/"):
            p += "/"
        return p

    def __getitem__(self, oid: bytes):
        return self._drepo.object_store[oid]

    def get(self, ref_str: str):
        """Lookup by full or short hex hash."""
        ref_bytes = ref_str.encode() if isinstance(ref_str, str) else ref_str
        if len(ref_bytes) == 40:
            try:
                return self[ref_bytes]
            except KeyError:
                return None
        matches: list[bytes] = []
        for sha in self._drepo.object_store:
            if sha.startswith(ref_bytes):
                matches.append(sha)
                if len(matches) > 1:
                    raise ValueError(
                        f"Ambiguous short hash {ref_str!r}: matches "
                        f"{matches[0].decode()[:12]} and {matches[1].decode()[:12]}"
                    )
        if matches:
            return self[matches[0]]
        return None

    def create_blob(self, data: bytes) -> BlobOid:
        blob = _DBlob.from_string(data)
        self._drepo.object_store.add_object(blob)
        return BlobOid(blob.id)

    def create_blob_fromdisk(self, path: str) -> BlobOid:
        with open(path, "rb") as f:
            data = f.read()
        return self.create_blob(data)

    def create_commit(
        self,
        ref_name,
        author: Signature,
        committer: Signature,
        message: str,
        tree_oid: bytes,
        parent_oids: list[bytes],
    ) -> bytes:
        c = _DCommit()
        c.tree = tree_oid
        c.parents = list(parent_oids)
        c.author = author._identity
        c.committer = committer._identity
        now = int(_time.time())
        c.author_time = c.commit_time = now
        c.author_timezone = c.commit_timezone = 0
        msg = message.encode() if isinstance(message, str) else message
        if not msg.endswith(b"\n"):
            msg += b"\n"
        c.message = msg
        c.encoding = b"UTF-8"
        self._drepo.object_store.add_object(c)

        if ref_name is not None:
            ref_bytes = ref_name.encode() if isinstance(ref_name, str) else ref_name
            self._drepo.refs[ref_bytes] = c.id

        return c.id

    def create_tag(
        self,
        name: str,
        target_oid: bytes,
        target_type: int,
        tagger: Signature,
        message: str,
    ) -> bytes:
        _type_map = {1: _DCommit, 2: _DTree, 3: _DBlob, 4: _DTag}
        type_class = _type_map.get(target_type, _DCommit)
        tag = _DTag()
        tag.name = name.encode()
        tag.object = (type_class, target_oid)
        tag.tagger = tagger._identity
        tag.tag_time = int(_time.time())
        tag.tag_timezone = 0
        msg = message.encode() if isinstance(message, str) else message
        if not msg.endswith(b"\n"):
            msg += b"\n"
        tag.message = msg
        self._drepo.object_store.add_object(tag)
        ref_bytes = f"refs/tags/{name}".encode()
        self._drepo.refs[ref_bytes] = tag.id
        return tag.id

    def TreeBuilder(self, tree=None) -> TreeBuilder:
        return TreeBuilder(self._drepo, tree)

    @property
    def references(self) -> _References:
        return _References(self._drepo)

    def get_head_branch(self) -> str | None:
        symrefs = self._drepo.refs.get_symrefs()
        target = symrefs.get(b"HEAD")
        if target is None:
            return None
        prefix = b"refs/heads/"
        if target.startswith(prefix):
            name = target[len(prefix):].decode()
            if target in self._drepo.refs:
                return name
        return None

    def set_head_branch(self, name: str):
        self._drepo.refs.set_symbolic_ref(b"HEAD", f"refs/heads/{name}".encode())

    @property
    def object_store(self):
        return self._drepo.object_store


def init_repository(path: str, bare: bool = True) -> _Repository:
    """Create a new bare git repository."""
    repo = _DRepo.init_bare(path, mkdir=True)
    return _Repository(repo)


@dataclass
class ReflogEntry:
    """A single reflog entry recording a branch movement.

    Attributes:
        old_sha: Previous 40-char hex commit SHA.
        new_sha: New 40-char hex commit SHA.
        committer: Identity string of the committer.
        timestamp: POSIX epoch seconds of the entry.
        message: Reflog message (e.g. ``"commit: + file.txt"``).
    """
    old_sha: str
    new_sha: str
    committer: str
    timestamp: float
    message: str


def _validate_ref_name(name: str) -> None:
    """Reject ref names that don't conform to Git's rules."""
    from dulwich.refs import check_ref_format

    # Colon check (vost-specific: colons conflict with ref:path syntax)
    if ":" in name:
        raise ValueError(f"Invalid ref name {name!r}: contains colon")
    ref_bytes = f"refs/heads/{name}".encode()
    if not check_ref_format(ref_bytes):
        raise ValueError(f"Invalid ref name: {name!r}")


class GitStore:
    """A versioned filesystem backed by a bare git repository.

    Open or create a store with :meth:`open`.  Access snapshots via
    :attr:`branches`, :attr:`tags`, and :attr:`notes`.
    """

    def __init__(self, repo: _Repository, author: str, email: str):
        self._repo = repo
        self._signature = Signature(author, email)
        self.branches = RefDict(self, "refs/heads/")
        self.tags = RefDict(self, "refs/tags/")
        self.notes = NoteDict(self)

    def __repr__(self) -> str:
        return f"GitStore({self._repo.path!r})"

    def fs(self, ref: str, *, back: int = 0) -> FS:
        """Get an FS snapshot for any ref (branch, tag, or commit hash).

        Resolution order: branches → tags → commit hash.
        Writable for branches, read-only for tags and commit hashes.

        Args:
            ref: Branch name, tag name, or commit hash (full or short).
            back: Walk back N ancestor commits (default 0).

        Returns:
            FS snapshot for the resolved ref.

        Raises:
            KeyError: If ref cannot be resolved.
        """
        from .fs import FS

        if ref in self.branches:
            result = self.branches[ref]
        elif ref in self.tags:
            result = self.tags[ref]
        else:
            obj = self._repo.get(ref)
            if obj is None:
                raise KeyError(f"ref not found: {ref!r}")
            if obj.type_num != 1:
                raise KeyError(f"not a commit: {ref!r}")
            result = FS(self, obj.id, writable=False)
        if back:
            result = result.back(back)
        return result

    @classmethod
    def open(
        cls,
        path: str | Path,
        *,
        create: bool = True,
        branch: str | None = "main",
        author: str = "vost",
        email: str = "vost@localhost",
    ) -> GitStore:
        """Open or create a bare git repository.

        Args:
            path: Path to the bare repository.
            create: If True (default), create the repo when it doesn't exist.
                    If False, raise FileNotFoundError when missing.
            branch: Initial branch name when creating (default "main").
                    None to create a bare repo with no branches.
            author: Default author name for commits.
            email: Default author email for commits.
        """
        path = Path(path)

        if path.exists():
            repo = _Repository(str(path))
            return cls(repo, author, email)

        if not create:
            raise FileNotFoundError(f"Repository not found: {path}")

        repo = init_repository(str(path), bare=True)
        store = cls(repo, author, email)

        if branch is not None:
            sig = store._signature
            tree_oid = repo.TreeBuilder().write()
            repo.create_commit(
                f"refs/heads/{branch}",
                sig,
                sig,
                f"Initialize {branch}",
                tree_oid,
                [],
            )
            repo.set_head_branch(branch)

        return store

    def backup(
        self,
        url: str,
        *,
        dry_run: bool = False,
        progress: Callable | None = None,
        refs: list[str] | dict[str, str] | None = None,
        format: str | None = None,
        squash: bool = False,
    ) -> MirrorDiff:
        """Push refs to *url*, or write a bundle file.

        Without *refs* this is a full mirror: remote-only refs are deleted.
        With *refs* only the specified refs are pushed (no deletes).
        If *url* ends with ``.bundle`` (or *format* is ``"bundle"``), a
        portable bundle file is written instead of pushing to a remote.

        *refs* may be a list of names (identity mapping) or a dict mapping
        source names to destination names for renaming on the remote side.

        When *squash* is ``True`` and writing a bundle, each ref gets a
        parentless commit with the same tree (stripping history).

        Args:
            url: Remote URL, local path, or ``.bundle`` file path.
            dry_run: Compute diff without pushing.
            progress: Optional progress callback.
            refs: Ref names to include (short or full), or a dict mapping
                source to destination names. ``None`` = all refs.
            format: ``"bundle"`` to force bundle format.
            squash: Strip history — each ref becomes a single parentless
                commit. Only supported for bundle output.

        Returns:
            A :class:`MirrorDiff` describing what changed (or would change).
        """
        from .mirror import backup
        return backup(self, url, dry_run=dry_run, progress=progress,
                       refs=refs, format=format, squash=squash)

    def restore(
        self,
        url: str,
        *,
        dry_run: bool = False,
        progress: Callable | None = None,
        refs: list[str] | dict[str, str] | None = None,
        format: str | None = None,
    ) -> MirrorDiff:
        """Fetch refs from *url*, or import a bundle file.

        Restore is **additive**: refs are added and updated but local-only
        refs are never deleted.  HEAD (the current branch pointer) is not
        restored — use ``store.branches.current = "name"`` afterwards if
        needed.

        *refs* may be a list of names (identity mapping) or a dict mapping
        source names to destination names for renaming locally.

        Args:
            url: Remote URL, local path, or ``.bundle`` file path.
            dry_run: Compute diff without fetching.
            progress: Optional progress callback.
            refs: Ref names to include (short or full), or a dict mapping
                source to destination names. ``None`` = all refs.
            format: ``"bundle"`` to force bundle format.

        Returns:
            A :class:`MirrorDiff` describing what changed (or would change).
        """
        from .mirror import restore
        return restore(self, url, dry_run=dry_run, progress=progress,
                        refs=refs, format=format)

    def bundle_export(
        self,
        path: str,
        *,
        refs: list[str] | dict[str, str] | None = None,
        squash: bool = False,
        progress: Callable | None = None,
    ) -> None:
        """Export refs to a bundle file.

        *refs* may be a list of names (identity mapping) or a dict mapping
        source names to destination names for renaming in the bundle.

        When *squash* is ``True`` each ref in the bundle gets a parentless
        commit whose tree matches the original tip, stripping all history.

        Args:
            path: Destination ``.bundle`` file path.
            refs: Ref names to include (short or full), or a dict mapping
                source to destination names. ``None`` = all refs.
            squash: Strip history — each ref becomes a single parentless
                commit.
            progress: Optional progress callback.
        """
        from .mirror import bundle_export
        bundle_export(self, path, refs=refs, squash=squash, progress=progress)

    def bundle_import(
        self,
        path: str,
        *,
        refs: list[str] | dict[str, str] | None = None,
        progress: Callable | None = None,
    ) -> None:
        """Import refs from a bundle file (additive — no deletes).

        *refs* may be a list of names (identity mapping) or a dict mapping
        source names to destination names for renaming on import.

        Args:
            path: Source ``.bundle`` file path.
            refs: Ref names to include (short or full), or a dict mapping
                source to destination names. ``None`` = all refs.
            progress: Optional progress callback.
        """
        from .mirror import bundle_import
        bundle_import(self, path, refs=refs, progress=progress)


class RefDict(MutableMapping):
    """Dict-like access to branches or tags.

    ``store.branches`` and ``store.tags`` are both ``RefDict`` instances.
    Supports ``[]``, ``del``, ``in``, ``len``, and iteration.
    """

    def __init__(self, store: GitStore, prefix: str):
        self._store = store
        self._prefix = prefix  # "refs/heads/" or "refs/tags/"

    @property
    def _is_tags(self) -> bool:
        return self._prefix == "refs/tags/"

    def __repr__(self) -> str:
        kind = "tags" if self._is_tags else "branches"
        return f"RefDict({kind!r}, len={len(self)})"

    def _ref_name(self, name: str) -> str:
        return f"{self._prefix}{name}"

    def __getitem__(self, name: str) -> FS:
        from .fs import FS

        repo = self._store._repo
        ref_name = self._ref_name(name)
        try:
            ref = repo.references[ref_name]
        except KeyError:
            raise KeyError(name)
        oid = ref.resolve().target
        if self._is_tags:
            obj = repo[oid]
            for _ in range(50):
                if obj.type_num == 1:  # GIT_OBJECT_COMMIT
                    break
                if not isinstance(obj, _DTag):
                    raise ValueError(f"Tag {name!r} does not point to a commit")
                obj = repo[obj.object[1]]
            else:
                raise ValueError(f"Tag {name!r} does not point to a commit")
            if obj.type_num != 1:
                raise ValueError(f"Tag {name!r} does not point to a commit")
            return FS(self._store, obj.id, ref_name=name, writable=False)
        else:
            return FS(self._store, oid, ref_name=name)

    def __setitem__(self, name: str, fs: FS):
        from ._lock import repo_lock
        from .fs import FS

        _validate_ref_name(name)
        if not isinstance(fs, FS):
            raise TypeError(f"Expected FS, got {type(fs).__name__}")
        try:
            same = os.path.samefile(fs._store._repo.path, self._store._repo.path)
        except OSError:
            same = False
        if not same:
            raise ValueError("FS belongs to a different repository")

        repo = self._store._repo
        ref_name = self._ref_name(name)

        committer = self._store._signature._identity
        with repo_lock(repo.path):
            if ref_name in repo.references:
                if self._is_tags:
                    raise KeyError(f"Tag {name!r} already exists")
                # Get commit message for reflog
                commit = repo[fs._commit_oid]
                msg_str = commit.message.decode().splitlines()[0] if commit.message else ""
                msg = f"branch: set to {msg_str}".encode()
                repo.references[ref_name].set_target(fs._commit_oid, message=msg, committer=committer)
            else:
                commit = repo[fs._commit_oid]
                msg_str = commit.message.decode().splitlines()[0] if commit.message else ""
                msg = f"branch: Created from {msg_str}".encode()
                repo.references.create(ref_name, fs._commit_oid, message=msg, committer=committer)

    def __delitem__(self, name: str):
        from ._lock import repo_lock

        repo = self._store._repo
        ref_name = self._ref_name(name)

        with repo_lock(repo.path):
            try:
                repo.references[ref_name]
            except KeyError:
                raise KeyError(name)
            repo.references.delete(ref_name)

    def __contains__(self, name: str) -> bool:
        ref_name = self._ref_name(name)
        return ref_name in self._store._repo.references

    def __iter__(self) -> Iterator[str]:
        prefix_len = len(self._prefix)
        for ref_name in self._store._repo.references:
            if ref_name.startswith(self._prefix):
                yield ref_name[prefix_len:]

    def __len__(self) -> int:
        return sum(1 for _ in self)

    def set(self, name: str, fs: FS) -> FS:
        """Set branch to FS snapshot and return writable FS bound to it.

        This is a convenience method that combines setting and getting:

            fs_new = repo.branches.set('feature', fs)

        Is equivalent to:

            repo.branches['feature'] = fs
            fs_new = repo.branches['feature']

        Args:
            name: Branch name
            fs: FS snapshot to set (can be read-only)

        Returns:
            New writable FS bound to the branch

        Example:
            >>> fs_wow = repo.branches.set('wow', fs_main)
            >>> fs_wow.ref_name  # 'wow' (not 'main')
        """
        self[name] = fs
        return self[name]

    @property
    def current_name(self) -> str | None:
        """The repository's current (HEAD) branch name, or ``None`` if HEAD is dangling.

        Only valid for branches; raises ``ValueError`` for tags.
        Cheap — does not construct an FS object.
        """
        if self._is_tags:
            raise ValueError("Tags do not have a current branch")
        return self._store._repo.get_head_branch()

    @property
    def current(self) -> FS | None:
        """The FS for the repository's current (HEAD) branch, or ``None`` if HEAD is dangling.

        Only valid for branches; raises ``ValueError`` for tags.
        """
        if self._is_tags:
            raise ValueError("Tags do not have a current branch")
        name = self._store._repo.get_head_branch()
        if name is None:
            return None
        return self[name]

    @current.setter
    def current(self, name: str) -> None:
        if self._is_tags:
            raise ValueError("Tags do not have a current branch")
        if name not in self:
            raise KeyError(f"Branch not found: {name!r}")
        self._store._repo.set_head_branch(name)

    def reflog(self, name: str) -> list[ReflogEntry]:
        """Read reflog entries for a branch.

        Args:
            name: Branch name (e.g., "main")

        Returns:
            List of :class:`ReflogEntry` objects.

        Raises:
            KeyError: If branch doesn't exist
            FileNotFoundError: If no reflog exists

        Example:
            >>> entries = repo.branches.reflog("main")
            >>> for e in entries:
            ...     print(f"{e.message}: {e.new_sha[:7]}")
        """
        if self._is_tags:
            raise ValueError("Tags do not have reflog")

        # Verify branch exists
        ref_name = self._ref_name(name)
        if ref_name not in self._store._repo.references:
            raise KeyError(name)

        ref_bytes = ref_name.encode() if isinstance(ref_name, str) else ref_name
        entries = list(self._store._repo._drepo.read_reflog(ref_bytes))
        if not entries:
            raise FileNotFoundError(f"No reflog found for branch {name!r}")
        return [
            ReflogEntry(
                old_sha=e.old_sha.decode(),
                new_sha=e.new_sha.decode(),
                committer=e.committer.decode(),
                timestamp=e.timestamp,
                message=e.message.decode(),
            )
            for e in entries
        ]
