"""Mirror (backup/restore) operations for vost.

Ref-level mirroring: push all local refs to a remote (backup) or fetch
all remote refs to local (restore).  Extracted from repo.py and cli.py.
"""

from __future__ import annotations

import os
import time as _time
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from dulwich.client import get_transport_and_path as _get_transport_and_path
from dulwich.errors import NotGitRepository
from dulwich.porcelain import ls_remote as _ls_remote
from dulwich.protocol import ZERO_SHA as _ZERO_SHA
from dulwich.repo import Repo as _DRepo

if TYPE_CHECKING:
    from .repo import GitStore


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

@dataclass
class RefChange:
    """A single ref change in a :class:`MirrorDiff`.

    Attributes:
        ref: Full ref name (e.g. ``"refs/heads/main"``).
        old_target: Previous 40-char hex SHA, or ``None`` for creates.
        new_target: New 40-char hex SHA, or ``None`` for deletes.
    """
    ref: str
    old_target: str | None = None
    new_target: str | None = None


@dataclass
class MirrorDiff:
    """Result of a :meth:`~vost.GitStore.backup` or :meth:`~vost.GitStore.restore` operation.

    Attributes:
        add: Refs to create.
        update: Refs to update.
        delete: Refs to delete.
    """
    add: list[RefChange] = field(default_factory=list)
    update: list[RefChange] = field(default_factory=list)
    delete: list[RefChange] = field(default_factory=list)

    @property
    def in_sync(self) -> bool:
        """``True`` if there are no changes."""
        return not self.add and not self.update and not self.delete

    @property
    def total(self) -> int:
        """Total number of ref changes."""
        return len(self.add) + len(self.update) + len(self.delete)


# ---------------------------------------------------------------------------
# Bundle detection and ref name resolution
# ---------------------------------------------------------------------------

def _is_bundle_path(path: str) -> bool:
    """Return True if *path* has a ``.bundle`` extension."""
    return path.lower().endswith(".bundle")


def _resolve_ref_names(names, available_refs):
    """Resolve short ref names to full byte ref paths.

    Tries ``refs/heads/``, ``refs/tags/``, ``refs/notes/`` prefixes against
    *available_refs*.  Full paths (starting with ``refs/``) pass through
    unchanged.  If no match is found the name is assumed to be a branch.

    Args:
        names: Iterable of ref names (str or bytes).
        available_refs: Set/collection of bytes ref names to match against.

    Returns:
        Set of bytes ref names.
    """
    result = set()
    for name in names:
        name_b = name.encode() if isinstance(name, str) else name
        if name_b.startswith(b"refs/"):
            result.add(name_b)
            continue
        for prefix in (b"refs/heads/", b"refs/tags/", b"refs/notes/"):
            candidate = prefix + name_b
            if candidate in available_refs:
                result.add(candidate)
                break
        else:
            # Default: assume branch
            result.add(b"refs/heads/" + name_b)
    return result


def _resolve_one_ref_name(name, available_refs):
    """Resolve a single short ref name to a full byte ref path.

    Like ``_resolve_ref_names`` but for one name.  Returns bytes.
    """
    name_b = name.encode() if isinstance(name, str) else name
    if name_b.startswith(b"refs/"):
        return name_b
    for prefix in (b"refs/heads/", b"refs/tags/", b"refs/notes/"):
        candidate = prefix + name_b
        if candidate in available_refs:
            return candidate
    return b"refs/heads/" + name_b


def _normalize_refs(refs):
    """Convert refs to a dict mapping source -> dest.

    - ``None`` -> ``None`` (all refs, identity)
    - ``list`` -> ``{name: name for name in list}``
    - ``dict`` -> as-is

    Returns:
        ``None`` or ``dict[str, str]``.
    """
    if refs is None:
        return None
    if isinstance(refs, dict):
        return refs
    return {name: name for name in refs}


def _resolve_ref_map(ref_map, available_refs):
    """Resolve a src->dst ref map to full byte ref paths on both sides.

    *ref_map* is ``dict[str, str]`` from ``_normalize_refs``.
    *available_refs* is the set of bytes refs to resolve source names against.

    Returns:
        ``dict[bytes, bytes]`` mapping resolved-source to resolved-dest.
    """
    result = {}
    for src, dst in ref_map.items():
        src_full = _resolve_one_ref_name(src, available_refs)
        # For the dest, infer the same prefix as the resolved source
        dst_b = dst.encode() if isinstance(dst, str) else dst
        if not dst_b.startswith(b"refs/"):
            # Use the prefix from the resolved source
            for prefix in (b"refs/heads/", b"refs/tags/", b"refs/notes/"):
                if src_full.startswith(prefix):
                    dst_b = prefix + dst_b
                    break
            else:
                dst_b = b"refs/heads/" + dst_b
        result[src_full] = dst_b
    return result


# ---------------------------------------------------------------------------
# Transport helpers (operate on raw dulwich Repo)
# ---------------------------------------------------------------------------

def _diff_refs(drepo: _DRepo, url: str, direction: str) -> dict:
    """Compare local and remote refs.

    *direction* is ``"push"`` (local->remote) or ``"pull"`` (remote->local).
    Returns ``{"create": [...], "update": [...], "delete": [...],
    "src": {ref: sha}, "dest": {ref: sha}}`` with bytes keys.
    """
    # Auto-create remote for push if it's a local path that doesn't exist
    is_local = not any(url.startswith(proto) for proto in ["http://", "https://", "git://", "ssh://"])
    if is_local and not url.startswith("file://"):
        # Detect scp-style URLs: user@host:path or host:path
        # Exclude Windows drive letters (single letter before colon).
        if "@" in url and ":" in url.split("@", 1)[1]:
            raise ValueError(
                f"scp-style URL not supported: {url!r} — use ssh:// format instead"
            )
        colon_idx = url.find(":")
        # A colon after >1 chars with no path separator before it
        # looks like host:path.  Treat both / and \ as separators
        # to avoid rejecting Windows paths (e.g. \\?\C:\repo).
        prefix = url[:colon_idx]
        if colon_idx > 1 and "/" not in prefix and "\\" not in prefix:
            raise ValueError(
                f"scp-style URL not supported: {url!r} — use ssh:// format instead"
            )
    if is_local and direction == "push":
        local_path = url[7:] if url.startswith("file://") else url
        if not os.path.exists(local_path):
            _DRepo.init_bare(local_path, mkdir=True)

    try:
        remote_result = _ls_remote(url)
        refs_dict = remote_result.refs if hasattr(remote_result, "refs") else remote_result
        remote_refs = {
            ref: sha
            for ref, sha in refs_dict.items()
            if ref != b"HEAD" and not ref.endswith(b"^{}")
        }
    except NotGitRepository:
        # Remote doesn't exist - treat as empty for push, fail for pull
        if direction == "push":
            remote_refs = {}
        else:
            raise

    local_refs = {
        ref: sha
        for ref, sha in drepo.get_refs().items()
        if ref != b"HEAD"
    }

    if direction == "push":
        src, dest = local_refs, remote_refs
    else:
        src, dest = remote_refs, local_refs

    create, update, delete = [], [], []
    for ref, sha in src.items():
        if ref not in dest:
            create.append(ref)
        elif dest[ref] != sha:
            update.append(ref)
    for ref in dest:
        if ref not in src:
            delete.append(ref)

    return {"create": create, "update": update, "delete": delete,
            "src": src, "dest": dest}


def _mirror_push(drepo: _DRepo, url: str, *, progress=None):
    """Push all local refs to *url*, mirroring (force + delete stale)."""
    client, path = _get_transport_and_path(url)
    local_refs = {
        ref: sha
        for ref, sha in drepo.get_refs().items()
        if ref != b"HEAD"
    }

    def update_refs(remote_refs):
        new_refs = {}
        for ref, sha in local_refs.items():
            new_refs[ref] = sha
        for ref in remote_refs:
            if ref not in local_refs and ref != b"HEAD":
                new_refs[ref] = _ZERO_SHA
        return new_refs

    def gen_pack(have, want, *, ofs_delta=False, progress=progress):
        return drepo.object_store.generate_pack_data(
            have, want, ofs_delta=ofs_delta, progress=progress,
        )

    return client.send_pack(path, update_refs, gen_pack, progress=progress)


def _mirror_fetch(drepo: _DRepo, url: str, *, progress=None):
    """Fetch all remote refs from *url*, mirroring (force + delete stale)."""
    client, path = _get_transport_and_path(url)
    result = client.fetch(path, drepo, progress=progress)

    remote_refs = {
        ref: sha
        for ref, sha in result.refs.items()
        if ref != b"HEAD" and not ref.endswith(b"^{}")
    }

    # Set all remote refs locally
    for ref, sha in remote_refs.items():
        drepo.refs[ref] = sha

    # Delete local refs not on remote
    for ref in list(drepo.refs.allkeys()):
        if ref != b"HEAD" and ref not in remote_refs:
            drepo.refs.remove_if_equals(ref, drepo.refs[ref])

    return result


def _targeted_push(drepo: _DRepo, url: str, ref_filter: set, *,
                   ref_map: dict | None = None, progress=None):
    """Push only refs in *ref_filter* to *url* (no deletes).

    If *ref_map* is given (``{src_bytes: dst_bytes}``), refs are renamed
    on the remote side.
    """
    client, path = _get_transport_and_path(url)
    local_refs = {
        ref: sha
        for ref, sha in drepo.get_refs().items()
        if ref != b"HEAD" and ref in ref_filter
    }

    def update_refs(remote_refs):
        # Preserve all existing remote refs, only add/update targeted ones
        new_refs = dict(remote_refs)
        for ref, sha in local_refs.items():
            dst = ref_map.get(ref, ref) if ref_map else ref
            new_refs[dst] = sha
        return new_refs

    def gen_pack(have, want, *, ofs_delta=False, progress=progress):
        return drepo.object_store.generate_pack_data(
            have, want, ofs_delta=ofs_delta, progress=progress,
        )

    return client.send_pack(path, update_refs, gen_pack, progress=progress)


def _additive_fetch(drepo: _DRepo, url: str, *, refs=None, ref_map=None,
                    progress=None):
    """Fetch refs from *url* additively (no deletes).

    If *refs* is given (list of str), only those refs are set locally.
    If *ref_map* is given (``{src_bytes: dst_bytes}``), refs are renamed
    when written locally.
    """
    client, path = _get_transport_and_path(url)
    result = client.fetch(path, drepo, progress=progress)

    remote_refs = {
        ref: sha
        for ref, sha in result.refs.items()
        if ref != b"HEAD" and not ref.endswith(b"^{}")
    }

    if ref_map is not None:
        remote_refs = {r: s for r, s in remote_refs.items() if r in ref_map}
    elif refs is not None:
        ref_set = _resolve_ref_names(refs, set(remote_refs.keys()))
        remote_refs = {r: s for r, s in remote_refs.items() if r in ref_set}

    for ref, sha in remote_refs.items():
        dst = ref_map.get(ref, ref) if ref_map else ref
        drepo.refs[dst] = sha

    return result


# ---------------------------------------------------------------------------
# Bundle helpers
# ---------------------------------------------------------------------------

def _create_squashed_commit(drepo, tree_oid, signature):
    """Create a parentless commit with the given tree.

    The commit is written to the object store but no ref is created.
    Returns the commit OID (bytes, 40-char hex).
    """
    from dulwich.objects import Commit as _DCommit

    c = _DCommit()
    c.tree = tree_oid
    c.parents = []
    c.author = c.committer = signature._identity
    now = int(_time.time())
    c.author_time = c.commit_time = now
    c.author_timezone = c.commit_timezone = 0
    c.message = b"squash\n"
    c.encoding = b"UTF-8"
    drepo.object_store.add_object(c)
    return c.id


def bundle_export(store: GitStore, path: str, *, refs=None, squash: bool = False,
                  progress=None):
    """Create a bundle file from local refs.

    When *squash* is ``True`` each ref in the bundle gets a parentless
    commit whose tree matches the original tip, effectively stripping
    all history from the bundle.
    """
    from dulwich.bundle import create_bundle_from_repo, write_bundle

    drepo = store._repo._drepo
    all_local = {r for r in drepo.get_refs() if r != b"HEAD"}
    ref_map_normalized = _normalize_refs(refs)

    if ref_map_normalized is not None:
        ref_map = _resolve_ref_map(ref_map_normalized, all_local)
        bundle_refs = sorted(ref_map.keys())
    else:
        ref_map = None
        bundle_refs = sorted(all_local)

    # ------------------------------------------------------------------
    # Squash: replace each ref's commit with a parentless commit
    # sharing the same tree.  We create temporary refs so that
    # create_bundle_from_repo can look them up normally.
    # ------------------------------------------------------------------
    tmp_refs: list[bytes] = []
    if squash:
        squashed_bundle_refs = []
        for ref in bundle_refs:
            sha = drepo.refs[ref]
            obj = drepo.object_store[sha]
            # Peel tags to commit
            from dulwich.objects import Tag as _DTag
            while isinstance(obj, _DTag):
                obj = drepo.object_store[obj.object[1]]
            tree_oid = obj.tree
            sq_oid = _create_squashed_commit(drepo, tree_oid, store._signature)
            # Create a temporary ref
            tmp_name = b"refs/vost-squash-tmp/" + ref.split(b"/", 2)[-1]
            drepo.refs[tmp_name] = sq_oid
            tmp_refs.append(tmp_name)
            squashed_bundle_refs.append(tmp_name)
        actual_refs = squashed_bundle_refs
    else:
        actual_refs = bundle_refs

    try:
        bundle = create_bundle_from_repo(drepo, refs=actual_refs, progress=progress)

        if squash:
            # Remap temp ref names back to the real names (or renamed names)
            new_references = {}
            for tmp_name, orig_name in zip(tmp_refs, bundle_refs):
                dest_name = ref_map.get(orig_name, orig_name) if ref_map else orig_name
                new_references[dest_name] = bundle.references[tmp_name]
            bundle.references = new_references
        elif ref_map is not None:
            # Rename refs in the bundle header if a mapping is provided
            bundle.references = {
                ref_map.get(r, r): sha
                for r, sha in bundle.references.items()
            }

        with open(path, "wb") as f:
            write_bundle(f, bundle)
        bundle.close()
    finally:
        # Clean up temporary refs
        for tmp_name in tmp_refs:
            try:
                del drepo.refs[tmp_name]
            except (KeyError, Exception):
                pass


def bundle_import(store: GitStore, path: str, *, refs=None, progress=None):
    """Import refs from a bundle file (additive — no deletes)."""
    from dulwich.bundle import read_bundle

    drepo = store._repo._drepo
    ref_map_normalized = _normalize_refs(refs)

    with open(path, "rb") as f:
        bundle = read_bundle(f)
        # Work around dulwich bug: Bundle.store_objects() uses
        # iter_unpacked() which doesn't resolve ofs_delta objects,
        # silently dropping delta-compressed entries.  Import the
        # pack via add_thin_pack() which resolves deltas correctly.
        raw = bundle.pack_data._file
        raw.seek(0)
        drepo.object_store.add_thin_pack(raw.read, None)
        if ref_map_normalized is not None:
            ref_map = _resolve_ref_map(ref_map_normalized,
                                       set(bundle.references.keys()))
        for ref, sha in bundle.references.items():
            if ref_map_normalized is None:
                drepo.refs[ref] = sha
            elif ref in ref_map:
                drepo.refs[ref_map[ref]] = sha
        bundle.close()


def _diff_bundle_export(store: GitStore, path: str, *, refs=None) -> dict:
    """Compute diff for exporting a bundle (all refs are 'create')."""
    drepo = store._repo._drepo
    local_refs = {
        ref: sha for ref, sha in drepo.get_refs().items()
        if ref != b"HEAD"
    }
    ref_map_normalized = _normalize_refs(refs)
    if ref_map_normalized is not None:
        ref_map = _resolve_ref_map(ref_map_normalized, set(local_refs.keys()))
        # Filter to only matched sources, then rename to dest names
        filtered = {}
        for src, dst in ref_map.items():
            if src in local_refs:
                filtered[dst] = local_refs[src]
        local_refs = filtered

    return {
        "create": list(local_refs.keys()),
        "update": [],
        "delete": [],
        "src": local_refs,
        "dest": {},
    }


def _diff_bundle_import(store: GitStore, path: str, *, refs=None) -> dict:
    """Compute diff for importing a bundle (additive — no deletes)."""
    from dulwich.bundle import read_bundle

    drepo = store._repo._drepo
    with open(path, "rb") as f:
        bundle = read_bundle(f)
        bundle_refs = dict(bundle.references)
        bundle.close()

    ref_map_normalized = _normalize_refs(refs)
    if ref_map_normalized is not None:
        ref_map = _resolve_ref_map(ref_map_normalized,
                                   set(bundle_refs.keys()))
        # Remap: keep only matched sources, rename to dest
        remapped = {}
        for src, dst in ref_map.items():
            if src in bundle_refs:
                remapped[dst] = bundle_refs[src]
        bundle_refs = remapped

    local_refs = {r: s for r, s in drepo.get_refs().items() if r != b"HEAD"}

    create, update = [], []
    for ref, sha in bundle_refs.items():
        if ref not in local_refs:
            create.append(ref)
        elif local_refs[ref] != sha:
            update.append(ref)

    return {
        "create": create,
        "update": update,
        "delete": [],
        "src": bundle_refs,
        "dest": local_refs,
    }


# ---------------------------------------------------------------------------
# Core mirror functions
# ---------------------------------------------------------------------------

def _raw_diff_to_sync_diff(raw: dict) -> MirrorDiff:
    """Convert bytes-keyed diff dict to MirrorDiff."""
    src, dest = raw["src"], raw["dest"]

    def _sha(b):
        return b.decode() if isinstance(b, bytes) else str(b)

    add = [
        RefChange(ref=ref.decode(), new_target=_sha(src[ref]))
        for ref in raw["create"]
    ]
    update = [
        RefChange(ref=ref.decode(), old_target=_sha(dest[ref]), new_target=_sha(src[ref]))
        for ref in raw["update"]
    ]
    delete = [
        RefChange(ref=ref.decode(), old_target=_sha(dest[ref]))
        for ref in raw["delete"]
    ]
    return MirrorDiff(add=add, update=update, delete=delete)


def backup(
    store: GitStore,
    url: str,
    *,
    dry_run: bool = False,
    progress=None,
    refs: list[str] | dict[str, str] | None = None,
    format: str | None = None,
    squash: bool = False,
) -> MirrorDiff:
    """Push refs to *url* (or write a bundle file).

    Without ``--ref`` this is a full mirror: remote-only refs are deleted.
    With ``--ref`` only the specified refs are pushed (no deletes).

    *refs* may be a list of names (identity mapping) or a dict mapping
    source names to destination names for renaming on the remote side.

    When *squash* is ``True`` and writing a bundle, each ref gets a
    parentless commit with the same tree (stripping history).
    *squash* is only supported for bundle output.

    Returns a `MirrorDiff` describing what changed (or would change).
    """
    drepo = store._repo._drepo
    use_bundle = (format == "bundle") or _is_bundle_path(url)

    if squash and not use_bundle:
        raise ValueError("squash is only supported for bundle output")

    if use_bundle:
        raw = _diff_bundle_export(store, url, refs=refs)
        diff = _raw_diff_to_sync_diff(raw)
        if not dry_run:
            bundle_export(store, url, refs=refs, squash=squash, progress=progress)
        return diff

    if refs is not None:
        ref_map_normalized = _normalize_refs(refs)
        raw = _diff_refs(drepo, url, "push")
        ref_map = _resolve_ref_map(ref_map_normalized, set(raw["src"].keys()))
        has_rename = any(s != d for s, d in ref_map.items())
        ref_set = set(ref_map.keys())
        if has_rename:
            # For renaming, we need to compute the diff using dest names
            # against the remote, and filter sources
            filtered_src = {ref_map[r]: raw["src"][r] for r in raw["create"]
                            if r in ref_set}
            filtered_src.update({ref_map[r]: raw["src"][r] for r in raw["update"]
                                 if r in ref_set})
            # Also include refs in ref_set that are in src but not in create/update
            # (i.e. already in sync under old name — but dest name may be new)
            for src_ref in ref_set:
                dst_ref = ref_map[src_ref]
                if src_ref in raw["src"] and dst_ref not in filtered_src:
                    filtered_src[dst_ref] = raw["src"][src_ref]
            # Recompute create/update against remote using dest names
            create, update = [], []
            for dst_ref, sha in filtered_src.items():
                if dst_ref not in raw["dest"]:
                    create.append(dst_ref)
                elif raw["dest"][dst_ref] != sha:
                    update.append(dst_ref)
            renamed_raw = {
                "create": create, "update": update, "delete": [],
                "src": filtered_src, "dest": raw["dest"],
            }
            diff = _raw_diff_to_sync_diff(renamed_raw)
        else:
            raw["create"] = [r for r in raw["create"] if r in ref_set]
            raw["update"] = [r for r in raw["update"] if r in ref_set]
            raw["delete"] = []  # no deletes when using --ref
            diff = _raw_diff_to_sync_diff(raw)
        if not dry_run:
            _targeted_push(drepo, url, ref_set,
                           ref_map=ref_map if has_rename else None,
                           progress=progress)
        return diff

    raw = _diff_refs(drepo, url, "push")
    diff = _raw_diff_to_sync_diff(raw)
    if not dry_run:
        _mirror_push(drepo, url, progress=progress)
    return diff


def restore(
    store: GitStore,
    url: str,
    *,
    dry_run: bool = False,
    progress=None,
    refs: list[str] | dict[str, str] | None = None,
    format: str | None = None,
) -> MirrorDiff:
    """Fetch refs from *url* (or import a bundle file).

    Restore is **additive**: it adds and updates refs but never deletes
    local-only refs.

    *refs* may be a list of names (identity mapping) or a dict mapping
    source names to destination names for renaming locally.

    Returns a `MirrorDiff` describing what changed (or would change).
    """
    drepo = store._repo._drepo
    use_bundle = (format == "bundle") or _is_bundle_path(url)

    if use_bundle:
        raw = _diff_bundle_import(store, url, refs=refs)
        diff = _raw_diff_to_sync_diff(raw)
        if not dry_run:
            bundle_import(store, url, refs=refs, progress=progress)
        return diff

    raw = _diff_refs(drepo, url, "pull")
    ref_map_normalized = _normalize_refs(refs)
    if ref_map_normalized is not None:
        ref_map = _resolve_ref_map(ref_map_normalized, set(raw["src"].keys()))
        has_rename = any(s != d for s, d in ref_map.items())
        ref_set = set(ref_map.keys())
        if has_rename:
            # Compute diff using dest names against local refs
            local_refs = raw["dest"]
            filtered_src = {}
            for src_ref in ref_set:
                if src_ref in raw["src"]:
                    dst_ref = ref_map[src_ref]
                    filtered_src[dst_ref] = raw["src"][src_ref]
            create, update = [], []
            for dst_ref, sha in filtered_src.items():
                if dst_ref not in local_refs:
                    create.append(dst_ref)
                elif local_refs[dst_ref] != sha:
                    update.append(dst_ref)
            renamed_raw = {
                "create": create, "update": update, "delete": [],
                "src": filtered_src, "dest": local_refs,
            }
            diff = _raw_diff_to_sync_diff(renamed_raw)
        else:
            raw["create"] = [r for r in raw["create"] if r in ref_set]
            raw["update"] = [r for r in raw["update"] if r in ref_set]
            raw["delete"] = []
            diff = _raw_diff_to_sync_diff(raw)
        if not dry_run:
            _additive_fetch(drepo, url,
                            ref_map=ref_map if has_rename else None,
                            refs=refs if not has_rename else None,
                            progress=progress)
    else:
        raw["delete"] = []  # additive: never delete
        diff = _raw_diff_to_sync_diff(raw)
        if not dry_run:
            _additive_fetch(drepo, url, refs=refs, progress=progress)
    return diff


# ---------------------------------------------------------------------------
# Credentials
# ---------------------------------------------------------------------------

def resolve_credentials(url: str) -> str:
    """Inject credentials into an HTTPS URL if available.

    Tries ``git credential fill`` first (works with any configured helper:
    osxkeychain, wincred, libsecret, ``gh auth setup-git``, etc.).  Falls
    back to ``gh auth token`` for GitHub hosts.  Non-HTTPS URLs and URLs
    that already contain credentials are returned unchanged.
    """
    if not url.startswith("https://"):
        return url

    from urllib.parse import urlparse, urlunparse

    parsed = urlparse(url)
    if parsed.username:
        return url  # already has credentials

    import subprocess

    # Try git credential fill
    try:
        stdin = f"protocol={parsed.scheme}\nhost={parsed.hostname}\n\n"
        proc = subprocess.run(
            ["git", "credential", "fill"],
            input=stdin, capture_output=True, text=True, timeout=5,
        )
        if proc.returncode == 0:
            creds = {}
            for line in proc.stdout.strip().splitlines():
                if "=" in line:
                    k, _, v = line.partition("=")
                    creds[k] = v
            username = creds.get("username")
            password = creds.get("password")
            if username and password:
                from urllib.parse import quote

                netloc = f"{quote(username, safe='')}:{quote(password, safe='')}@{parsed.hostname}"
                if parsed.port:
                    netloc += f":{parsed.port}"
                return urlunparse(parsed._replace(netloc=netloc))
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fallback: gh auth token (GitHub-specific)
    try:
        proc = subprocess.run(
            ["gh", "auth", "token", "--hostname", parsed.hostname],
            capture_output=True, text=True, timeout=5,
        )
        token = proc.stdout.strip()
        if proc.returncode == 0 and token:
            netloc = f"x-access-token:{token}@{parsed.hostname}"
            if parsed.port:
                netloc += f":{parsed.port}"
            return urlunparse(parsed._replace(netloc=netloc))
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    return url
