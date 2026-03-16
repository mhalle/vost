"""Copy/sync/remove/move operations (internal implementations)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import TYPE_CHECKING

from ..tree import _entry_at_path, _normalize_path, _mode_from_disk, GIT_FILEMODE_LINK
from ._types import ChangeError, ChangeReport, FileEntry, FileType, _finalize_changes
from ._resolve import (
    _walk_local_paths,
    _walk_repo,
    _resolve_disk_sources,
    _resolve_repo_sources,
    _enum_disk_to_repo,
    _enum_repo_to_disk,
    _enum_repo_to_repo,
)
from ._io import (
    _local_file_oid,
    _local_file_oid_abs,
    _copy_blob_to_batch,
    _write_files_to_repo,
    _write_files_to_disk,
    _filter_tree_conflicts,
    _prune_empty_dirs,
)

if TYPE_CHECKING:
    from .._exclude import ExcludeFilter
    from ..fs import FS


# ---------------------------------------------------------------------------
# Helper: Convert path lists to FileEntry lists
# ---------------------------------------------------------------------------

def _make_entries_from_disk(rel_paths: list[str], rel_to_abs: dict[str, str], follow_symlinks: bool = False) -> list[FileEntry]:
    """Convert relative paths to FileEntry objects by checking filesystem.

    Args:
        rel_paths: List of relative paths (relative to dest)
        rel_to_abs: Mapping from relative paths to absolute local paths
        follow_symlinks: Whether symlinks are being followed
    """
    entries = []
    for rel in rel_paths:
        full_path = rel_to_abs.get(rel, rel)  # fallback to rel if not in map
        if os.path.islink(full_path) and not follow_symlinks:
            entries.append(FileEntry(rel, FileType.LINK, src=full_path))
        else:
            try:
                mode = _mode_from_disk(full_path)
                entries.append(FileEntry.from_mode(rel, mode, src=full_path))
            except OSError:
                # If we can't read it, assume blob
                entries.append(FileEntry(rel, FileType.BLOB, src=full_path))
    return entries


def _make_entries_from_repo(fs: FS, rel_paths: list[str], base_path: str) -> list[FileEntry]:
    """Convert relative paths to FileEntry objects by checking repo."""
    entries = []
    for rel in rel_paths:
        full_path = f"{base_path}/{rel}" if base_path else rel
        entry = _entry_at_path(fs._store._repo, fs._tree_oid, full_path)
        if entry:
            entries.append(FileEntry.from_mode(rel, entry[1], src=full_path))
        else:
            # Shouldn't happen, but handle gracefully
            entries.append(FileEntry(rel, FileType.BLOB, src=full_path))
    return entries


def _make_entries_from_repo_dict(fs: FS, rel_paths: list[str], rel_to_repo: dict[str, str]) -> list[FileEntry]:
    """Convert relative paths to FileEntry objects by checking repo.

    Args:
        rel_paths: List of relative paths (relative to dest)
        rel_to_repo: Mapping from relative paths to repo paths
    """
    entries = []
    for rel in rel_paths:
        repo_path = rel_to_repo.get(rel, rel)
        entry = _entry_at_path(fs._store._repo, fs._tree_oid, repo_path)
        if entry:
            entries.append(FileEntry.from_mode(rel, entry[1], src=repo_path))
        else:
            # Shouldn't happen, but handle gracefully
            entries.append(FileEntry(rel, FileType.BLOB, src=repo_path))
    return entries


# ---------------------------------------------------------------------------
# Common executor
# ---------------------------------------------------------------------------

def _apply_plan(
    fs: FS,
    write_pairs: list[tuple[str, str]],
    delete_paths: list[str],
    changes: ChangeReport,
    *,
    message: str | None = None,
    operation: str | None = None,
    follow_symlinks: bool = False,
    mode: FileType | int | None = None,
    ignore_errors: bool = False,
    parents: list[FS] | None = None,
) -> FS:
    """Execute a batch of writes and deletes, attach *changes* to the result.

    Shared by ``_copy_in`` and ``_remove``.
    """
    if not write_pairs and not delete_paths:
        if ignore_errors and changes.errors:
            raise RuntimeError(f"All files failed to copy: {changes.errors}")
        fs._changes = _finalize_changes(changes)
        return fs

    with fs.batch(message=message, operation=operation, parents=parents) as b:
        if write_pairs:
            _write_files_to_repo(b, write_pairs, follow_symlinks=follow_symlinks,
                                 mode=mode, ignore_errors=ignore_errors,
                                 errors=changes.errors)
        for path in delete_paths:
            try:
                b.remove(path)
            except OSError as exc:
                if not ignore_errors:
                    raise
                changes.errors.append(ChangeError(path=path, error=str(exc)))
    result_fs = b.fs

    if ignore_errors and changes.errors and result_fs.commit_hash == fs.commit_hash:
        raise RuntimeError(f"All files failed to copy: {changes.errors}")

    result_fs._changes = _finalize_changes(changes)
    return result_fs


# ---------------------------------------------------------------------------
# Copy: disk → repo
# ---------------------------------------------------------------------------

def _copy_in(
    fs: FS,
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
    operation: str = "cp",
    parents: list[FS] | None = None,
) -> FS:
    """Copy local files/dirs/globs into the repo. Returns ``FS``.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.

    With ``delete=True``, files under *dest* that are not covered by
    *sources* are removed (rsync ``--delete`` semantics).

    The changes are available via ``fs.changes``.
    """
    if isinstance(sources, str):
        sources = [sources]
    if dry_run:
        return _copy_in_dry(fs, sources, dest,
                            follow_symlinks=follow_symlinks,
                            ignore_existing=ignore_existing, delete=delete,
                            checksum=checksum, exclude=exclude,
                            ignore_errors=ignore_errors)

    changes = ChangeReport()

    if ignore_errors:
        resolved: list[tuple[str, str, str]] = []
        for src in sources:
            try:
                resolved.extend(_resolve_disk_sources([src]))
            except (FileNotFoundError, NotADirectoryError) as exc:
                changes.errors.append(ChangeError(path=src, error=str(exc)))
        if not resolved:
            if changes.errors:
                raise RuntimeError(f"All files failed to copy: {changes.errors}")
            fs._changes = _finalize_changes(changes)
            return fs
    else:
        resolved = _resolve_disk_sources(sources)

    pairs = _enum_disk_to_repo(resolved, dest, follow_symlinks=follow_symlinks,
                               exclude=exclude)

    if delete:
        # Hash-based comparison: build plan then execute
        # Build {repo_rel: local_abs} from enumerated pairs
        pair_map: dict[str, str] = {}
        for local_path, repo_path in pairs:
            # repo_rel is the path relative to dest
            if dest and repo_path.startswith(dest + "/"):
                rel = repo_path[len(dest) + 1:]
            else:
                rel = repo_path
            if rel in pair_map:
                changes.warnings.append(ChangeError(
                    path=local_path,
                    error=f"Overlapping destination '{rel}': skipping (kept earlier source)",
                ))
            else:
                pair_map[rel] = local_path

        repo_files = _walk_repo(fs, dest)
        local_rels = set(pair_map.keys())
        repo_rels = set(repo_files.keys())

        add_rels = sorted(local_rels - repo_rels)
        delete_rels = sorted(repo_rels - local_rels)
        # rsync behavior: when --exclude is combined with --delete,
        # excluded files in the destination are PRESERVED (not deleted).
        if exclude is not None and exclude.active:
            delete_rels = [r for r in delete_rels
                           if not exclude.is_excluded(r)]
        both = sorted(local_rels & repo_rels)

        if not checksum:
            commit_ts = fs._store._repo[fs._commit_oid].commit_time

        update_rels: list[str] = []
        for rel in both:
            try:
                repo_oid, repo_mode = repo_files[rel]
                local_path = Path(pair_map[rel])

                if not checksum:
                    try:
                        if not follow_symlinks and local_path.is_symlink():
                            pass  # fall through to hash
                        elif int(local_path.stat().st_mtime) <= commit_ts:
                            continue  # assume unchanged
                    except OSError:
                        pass  # fall through to hash on stat failure

                if _local_file_oid_abs(local_path, follow_symlinks=follow_symlinks) != repo_oid:
                    update_rels.append(rel)
                elif repo_mode != GIT_FILEMODE_LINK and _mode_from_disk(pair_map[rel]) != repo_mode:
                    update_rels.append(rel)
            except OSError as exc:
                if not ignore_errors:
                    raise
                changes.errors.append(ChangeError(path=pair_map[rel], error=str(exc)))
                update_rels.append(rel)  # treat as needing update

        if ignore_existing:
            update_rels = []

        write_rels = add_rels + update_rels
        if not write_rels and not delete_rels:
            if ignore_errors and changes.errors:
                raise RuntimeError(
                    f"All files failed to copy: {changes.errors}"
                )
            fs._changes = _finalize_changes(changes)
            return fs

        write_pairs = []
        for rel in write_rels:
            repo_path = f"{dest}/{rel}" if dest else rel
            write_pairs.append((pair_map[rel], repo_path))

        write_path_set = set(write_rels)
        safe_deletes = _filter_tree_conflicts(write_path_set, delete_rels)
        delete_full = [f"{dest}/{rel}" if dest else rel for rel in safe_deletes]

        changes.add = _make_entries_from_disk(add_rels, pair_map, follow_symlinks)
        changes.update = _make_entries_from_disk(update_rels, pair_map, follow_symlinks)
        changes.delete = _make_entries_from_repo(fs, safe_deletes, dest)

        return _apply_plan(fs, write_pairs, delete_full, changes,
                           message=message, operation=operation,
                           follow_symlinks=follow_symlinks, mode=mode,
                           ignore_errors=ignore_errors, parents=parents)
    else:
        # Non-delete mode: classify written pairs as add vs update
        if ignore_existing:
            pairs = [(l, r) for l, r in pairs if not fs.exists(r)]

        if not pairs:
            if ignore_errors and changes.errors:
                raise RuntimeError(
                    f"All files failed to copy: {changes.errors}"
                )
            fs._changes = _finalize_changes(changes)
            return fs

        # Classify before writing and build rel -> local_path mapping
        pair_map: dict[str, str] = {}
        add_rels: list[str] = []
        update_rels: list[str] = []
        for local_path, repo_path in pairs:
            if dest and repo_path.startswith(dest + "/"):
                rel = repo_path[len(dest) + 1:]
            else:
                rel = repo_path
            pair_map[rel] = local_path
            if fs.exists(repo_path):
                update_rels.append(rel)
            else:
                add_rels.append(rel)

        changes.add = _make_entries_from_disk(add_rels, pair_map, follow_symlinks)
        changes.update = _make_entries_from_disk(update_rels, pair_map, follow_symlinks)

        return _apply_plan(fs, pairs, [], changes,
                           message=message, operation=operation,
                           follow_symlinks=follow_symlinks, mode=mode,
                           ignore_errors=ignore_errors, parents=parents)


def _copy_in_dry(
    fs: FS,
    sources: list[str],
    dest: str,
    *,
    follow_symlinks: bool = False,
    ignore_existing: bool = False,
    delete: bool = False,
    checksum: bool = True,
    exclude: ExcludeFilter | None = None,
    ignore_errors: bool = False,
) -> FS:
    """Dry-run for _copy_in. Returns input *fs* with ``.changes`` set."""
    changes = ChangeReport()
    if ignore_errors:
        resolved: list[tuple[str, str, str]] = []
        for src in sources:
            try:
                resolved.extend(_resolve_disk_sources([src]))
            except (FileNotFoundError, NotADirectoryError) as exc:
                changes.errors.append(ChangeError(path=src, error=str(exc)))
        if not resolved:
            fs._changes = _finalize_changes(changes)
            return fs
    else:
        resolved = _resolve_disk_sources(sources)
    pairs = _enum_disk_to_repo(resolved, dest, follow_symlinks=follow_symlinks,
                               exclude=exclude)

    if delete:
        prior_errors = changes.errors[:]
        changes = ChangeReport()
        changes.errors = prior_errors
        pair_map: dict[str, str] = {}
        for local_path, repo_path in pairs:
            if dest and repo_path.startswith(dest + "/"):
                rel = repo_path[len(dest) + 1:]
            else:
                rel = repo_path
            if rel in pair_map:
                changes.warnings.append(ChangeError(
                    path=local_path,
                    error=f"Overlapping destination '{rel}': skipping (kept earlier source)",
                ))
            else:
                pair_map[rel] = local_path

        repo_files = _walk_repo(fs, dest)
        local_rels = set(pair_map.keys())
        repo_rels = set(repo_files.keys())

        add = sorted(local_rels - repo_rels)
        delete_list = sorted(repo_rels - local_rels)
        # rsync behavior: when --exclude is combined with --delete,
        # excluded files in the destination are PRESERVED (not deleted).
        if exclude is not None and exclude.active:
            delete_list = [r for r in delete_list
                           if not exclude.is_excluded(r)]
        both = sorted(local_rels & repo_rels)

        if not checksum:
            commit_ts = fs._store._repo[fs._commit_oid].commit_time

        update: list[str] = []
        for rel in both:
            repo_oid, repo_mode = repo_files[rel]
            local_path = Path(pair_map[rel])

            if not checksum:
                try:
                    if not follow_symlinks and local_path.is_symlink():
                        pass  # fall through to hash
                    elif int(local_path.stat().st_mtime) <= commit_ts:
                        continue  # assume unchanged
                except OSError:
                    pass  # fall through to hash on stat failure

            if _local_file_oid_abs(local_path, follow_symlinks=follow_symlinks) != repo_oid:
                update.append(rel)
            elif repo_mode != GIT_FILEMODE_LINK and _mode_from_disk(pair_map[rel]) != repo_mode:
                update.append(rel)

        if ignore_existing:
            update = []

        # Convert to FileEntry lists
        changes.add = _make_entries_from_disk(add, pair_map, follow_symlinks)
        changes.update = _make_entries_from_disk(update, pair_map, follow_symlinks)
        changes.delete = _make_entries_from_repo(fs, delete_list, dest)
        fs._changes = _finalize_changes(changes)
        return fs
    else:
        # Non-delete mode: classify by existence only
        add: list[str] = []
        update: list[str] = []
        pair_map: dict[str, str] = {}
        for local_path, repo_path in pairs:
            if dest and repo_path.startswith(dest + "/"):
                rel = repo_path[len(dest) + 1:]
            else:
                rel = repo_path
            pair_map[rel] = local_path
            if fs.exists(repo_path):
                update.append(rel)
            else:
                add.append(rel)

        if ignore_existing:
            update = []

        # Convert to FileEntry lists
        add_entries = _make_entries_from_disk(sorted(add), pair_map, follow_symlinks)
        update_entries = _make_entries_from_disk(sorted(update), pair_map, follow_symlinks)
        result = ChangeReport(add=add_entries, update=update_entries)
        result.errors = changes.errors
        fs._changes = _finalize_changes(result)
        return fs


# ---------------------------------------------------------------------------
# Copy: repo → disk
# ---------------------------------------------------------------------------

def _copy_out(
    fs: FS,
    sources: str | list[str],
    dest: str,
    *,
    dry_run: bool = False,
    ignore_existing: bool = False,
    delete: bool = False,
    ignore_errors: bool = False,
    checksum: bool = True,
) -> FS:
    """Copy repo files/dirs/globs to local disk. Returns ``FS`` with ``.changes`` set.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.

    With ``delete=True``, local files under *dest* that are not covered
    by *sources* are removed (rsync ``--delete`` semantics).
    """
    if isinstance(sources, str):
        sources = [sources]
    if dry_run:
        return _copy_out_dry(fs, sources, dest,
                             ignore_existing=ignore_existing, delete=delete,
                             checksum=checksum, ignore_errors=ignore_errors)

    import shutil

    changes = ChangeReport()

    if ignore_errors:
        resolved: list[tuple[str, str, str]] = []
        for src in sources:
            try:
                resolved.extend(_resolve_repo_sources(fs, [src]))
            except (FileNotFoundError, NotADirectoryError) as exc:
                changes.errors.append(ChangeError(path=src, error=str(exc)))
        if not resolved:
            if changes.errors:
                raise RuntimeError(f"All files failed to copy: {changes.errors}")
            fs._changes = _finalize_changes(changes)
            return fs
    else:
        resolved = _resolve_repo_sources(fs, sources)

    pairs = _enum_repo_to_disk(fs, resolved, dest)

    if delete:
        base = Path(dest)
        base.mkdir(parents=True, exist_ok=True)

        # Build {local_rel: repo_path} from enumerated pairs
        pair_map: dict[str, str] = {}
        for repo_path, local_path in pairs:
            rel = os.path.relpath(local_path, dest).replace(os.sep, "/")
            if rel in pair_map:
                changes.warnings.append(ChangeError(
                    path=repo_path,
                    error=f"Overlapping destination '{rel}': skipping (kept earlier source)",
                ))
            else:
                pair_map[rel] = repo_path

        # B2 fix: build repo_files from pair_map (deduplicated) not pairs
        repo_files = {}
        for rel, rp in pair_map.items():
            entry = _entry_at_path(fs._store._repo, fs._tree_oid, rp)
            if entry is not None:
                repo_files[rel] = (entry[0], entry[1])

        local_paths = _walk_local_paths(dest)
        source_rels = set(pair_map.keys())

        add_rels = sorted(source_rels - local_paths)
        delete_rels = sorted(local_paths - source_rels)
        both = sorted(source_rels & local_paths)

        if not checksum:
            commit_ts = fs._store._repo[fs._commit_oid].commit_time

        update_rels: list[str] = []
        for rel in both:
            try:
                if rel in repo_files:
                    repo_oid, repo_mode = repo_files[rel]
                    local_path = base / rel

                    if not checksum:
                        try:
                            if local_path.is_symlink():
                                pass  # fall through to hash
                            elif int(local_path.stat().st_mtime) <= commit_ts:
                                continue  # assume unchanged
                        except OSError:
                            pass  # fall through to hash on stat failure

                    if _local_file_oid(base, rel) != repo_oid:
                        update_rels.append(rel)
                    elif repo_mode != GIT_FILEMODE_LINK and _mode_from_disk(str(base / rel)) != repo_mode:
                        update_rels.append(rel)
            except OSError as exc:
                if not ignore_errors:
                    raise
                changes.errors.append(ChangeError(path=str(base / rel), error=str(exc)))
                update_rels.append(rel)  # treat as needing update

        if ignore_existing:
            update_rels = []

        n_errs_pre = len(changes.errors)
        # Process deletes first
        for rel in delete_rels:
            out = base / rel
            try:
                if out.exists() or out.is_symlink():
                    out.unlink()
            except OSError as exc:
                if not ignore_errors:
                    raise
                changes.errors.append(ChangeError(path=str(out), error=str(exc)))

        # Clear blocking paths
        for rel in add_rels + update_rels:
            out = base / rel
            if out.is_dir() and not out.is_symlink():
                shutil.rmtree(out)
            for parent in out.parents:
                if parent == base:
                    break
                if parent.exists() and not parent.is_dir():
                    parent.unlink()
                    break
                if parent.is_symlink():
                    parent.unlink()
                    break

        write_pairs = []
        for rel in add_rels + update_rels:
            write_pairs.append((pair_map[rel], str(base / rel)))

        cts = fs._store._repo[fs._commit_oid].commit_time
        _write_files_to_disk(fs, write_pairs, base=base,
                             ignore_errors=ignore_errors,
                             errors=changes.errors, commit_ts=cts)
        _prune_empty_dirs(base)

        # Convert to FileEntry lists
        # For add/update from repo, get modes from repo
        repo_rel_to_path = {rel: pair_map[rel] for rel in add_rels + update_rels}
        changes.add = _make_entries_from_repo_dict(fs, add_rels, repo_rel_to_path)
        changes.update = _make_entries_from_repo_dict(fs, update_rels, repo_rel_to_path)
        # For deletes from disk, we don't have type info anymore (already deleted)
        # Just mark as blobs
        changes.delete = [FileEntry(rel, FileType.BLOB) for rel in delete_rels]

        n_op_errors = len(changes.errors) - n_errs_pre
        n_total_ops = len(write_pairs) + len(delete_rels)
        if ignore_errors and changes.errors and n_total_ops > 0 and n_op_errors >= n_total_ops:
            raise RuntimeError(
                f"All files failed to copy: {changes.errors}"
            )
        fs._changes = _finalize_changes(changes)
        return fs
    else:
        if ignore_existing:
            pairs = [(r, l) for r, l in pairs if not (Path(l).exists() or Path(l).is_symlink())]

        if not pairs:
            if ignore_errors and changes.errors:
                raise RuntimeError(
                    f"All files failed to copy: {changes.errors}"
                )
            fs._changes = _finalize_changes(changes)
            return fs

        # Classify as add vs update and build mapping
        repo_rel_to_path: dict[str, str] = {}
        add_rels: list[str] = []
        update_rels: list[str] = []
        for repo_path, local_path in pairs:
            rel = os.path.relpath(local_path, dest).replace(os.sep, "/")
            repo_rel_to_path[rel] = repo_path
            try:
                p = Path(local_path)
                exists = p.exists() or p.is_symlink()
            except OSError:
                exists = False
            if exists:
                update_rels.append(rel)
            else:
                add_rels.append(rel)

        n_errs_pre = len(changes.errors)
        cts = fs._store._repo[fs._commit_oid].commit_time
        _write_files_to_disk(fs, pairs, base=Path(dest),
                             ignore_errors=ignore_errors,
                             errors=changes.errors, commit_ts=cts)

        # Convert to FileEntry lists (get modes from repo)
        changes.add = _make_entries_from_repo_dict(fs, add_rels, repo_rel_to_path)
        changes.update = _make_entries_from_repo_dict(fs, update_rels, repo_rel_to_path)

        if ignore_errors and changes.errors and len(changes.errors) - n_errs_pre >= len(pairs):
            raise RuntimeError(
                f"All files failed to copy: {changes.errors}"
            )

    fs._changes = _finalize_changes(changes)
    return fs


def _copy_out_dry(
    fs: FS,
    sources: list[str],
    dest: str,
    *,
    ignore_existing: bool = False,
    delete: bool = False,
    checksum: bool = True,
    ignore_errors: bool = False,
) -> FS:
    """Dry-run for _copy_out. Returns input *fs* with ``.changes`` set."""
    prior_errors: list[ChangeError] = []
    if ignore_errors:
        resolved: list[tuple[str, str, str]] = []
        for src in sources:
            try:
                resolved.extend(_resolve_repo_sources(fs, [src]))
            except (FileNotFoundError, NotADirectoryError) as exc:
                prior_errors.append(ChangeError(path=src, error=str(exc)))
        if not resolved:
            changes = ChangeReport()
            changes.errors = prior_errors
            fs._changes = _finalize_changes(changes)
            return fs
    else:
        resolved = _resolve_repo_sources(fs, sources)
    pairs = _enum_repo_to_disk(fs, resolved, dest)

    if delete:
        base = Path(dest)
        changes = ChangeReport()
        changes.errors = prior_errors

        pair_map: dict[str, str] = {}
        for repo_path, local_path in pairs:
            rel = os.path.relpath(local_path, dest).replace(os.sep, "/")
            if rel in pair_map:
                changes.warnings.append(ChangeError(
                    path=repo_path,
                    error=f"Overlapping destination '{rel}': skipping (kept earlier source)",
                ))
            else:
                pair_map[rel] = repo_path

        repo_files: dict[str, tuple[bytes, int]] = {}
        for rel, rp in pair_map.items():
            entry = _entry_at_path(fs._store._repo, fs._tree_oid, rp)
            if entry is not None:
                repo_files[rel] = (entry[0], entry[1])

        local_paths = _walk_local_paths(dest) if base.exists() else set()
        source_rels = set(pair_map.keys())

        add = sorted(source_rels - local_paths)
        delete_list = sorted(local_paths - source_rels)
        both = sorted(source_rels & local_paths)

        if not checksum:
            commit_ts = fs._store._repo[fs._commit_oid].commit_time

        update: list[str] = []
        for rel in both:
            if rel in repo_files:
                repo_oid, repo_mode = repo_files[rel]
                local_path = base / rel

                if not checksum:
                    try:
                        if local_path.is_symlink():
                            pass  # fall through to hash
                        elif int(local_path.stat().st_mtime) <= commit_ts:
                            continue  # assume unchanged
                    except OSError:
                        pass  # fall through to hash on stat failure

                if _local_file_oid(base, rel) != repo_oid:
                    update.append(rel)
                elif repo_mode != GIT_FILEMODE_LINK and _mode_from_disk(str(base / rel)) != repo_mode:
                    update.append(rel)

        if ignore_existing:
            update = []

        # Convert to FileEntry lists
        changes.add = _make_entries_from_repo_dict(fs, add, pair_map)
        changes.update = _make_entries_from_repo_dict(fs, update, pair_map)
        # For deletes, we don't have type info (files on disk), just mark as blobs
        changes.delete = [FileEntry(rel, FileType.BLOB) for rel in delete_list]
        fs._changes = _finalize_changes(changes)
        return fs
    else:
        # Non-delete mode: classify by existence only
        add: list[str] = []
        update: list[str] = []
        repo_rel_to_path: dict[str, str] = {}
        for repo_path, local_path in pairs:
            rel = os.path.relpath(local_path, dest).replace(os.sep, "/")
            repo_rel_to_path[rel] = repo_path
            p = Path(local_path)
            if p.exists() or p.is_symlink():
                update.append(rel)
            else:
                add.append(rel)

        if ignore_existing:
            update = []

        # Convert to FileEntry lists
        add_entries = _make_entries_from_repo_dict(fs, sorted(add), repo_rel_to_path)
        update_entries = _make_entries_from_repo_dict(fs, sorted(update), repo_rel_to_path)
        result = ChangeReport(add=add_entries, update=update_entries)
        result.errors = prior_errors
        fs._changes = _finalize_changes(result)
        return fs


# ---------------------------------------------------------------------------
# Remove
# ---------------------------------------------------------------------------

def _collect_remove_paths(
    fs: FS,
    sources: list[str],
    *,
    recursive: bool = False,
) -> list[str]:
    """Resolve *sources* against the repo and return full paths to delete.

    Raises ``FileNotFoundError`` when a source matches nothing and
    ``IsADirectoryError`` when a directory is matched without *recursive*.
    """
    resolved = _resolve_repo_sources(fs, sources)
    delete_paths: list[str] = []
    for repo_path, mode, _prefix in resolved:
        if mode == "file":
            delete_paths.append(repo_path)
        elif mode in ("dir", "contents"):
            if not recursive:
                raise IsADirectoryError(
                    f"{repo_path} is a directory (use recursive=True)"
                )
            walk_root = repo_path or None
            for dirpath, _dirs, files in fs.walk(walk_root):
                for fe in files:
                    full = f"{dirpath}/{fe.name}" if dirpath else fe.name
                    delete_paths.append(full)
    return sorted(set(delete_paths))


def _remove(
    fs: FS,
    sources: str | list[str],
    *,
    dry_run: bool = False,
    recursive: bool = False,
    message: str | None = None,
    parents: list[FS] | None = None,
) -> FS:
    """Remove files matching *sources* from the repo. Returns ``FS``.

    *sources* may be a single string or a list of strings.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.
    """
    if isinstance(sources, (str, os.PathLike)):
        sources = [str(sources)]

    delete_paths = _collect_remove_paths(fs, sources, recursive=recursive)
    if not delete_paths:
        raise FileNotFoundError(f"No matches for sources: {sources}")

    changes = ChangeReport()
    rel_to_repo = {p: p for p in delete_paths}
    changes.delete = _make_entries_from_repo_dict(fs, delete_paths, rel_to_repo)

    if dry_run:
        fs._changes = _finalize_changes(changes)
        return fs

    return _apply_plan(fs, [], delete_paths, changes,
                       message=message, operation="rm", parents=parents)


# ---------------------------------------------------------------------------
# Sync (convenience wrappers with delete=True)
# ---------------------------------------------------------------------------

def _ensure_trailing_slash(path: str) -> str:
    """Ensure *path* ends with ``/`` (contents mode for copy)."""
    return path if path.endswith("/") else path + "/"


def _sync_in(
    fs: FS, local_path: str, repo_path: str, *,
    dry_run: bool = False,
    message: str | None = None,
    ignore_errors: bool = False,
    checksum: bool = True,
    exclude: ExcludeFilter | None = None,
    parents: list[FS] | None = None,
) -> FS:
    """Make *repo_path* identical to *local_path*. Returns ``FS``.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.
    """
    if dry_run:
        try:
            return _copy_in(
                fs, [_ensure_trailing_slash(local_path)], repo_path,
                dry_run=True, delete=True,
                checksum=checksum, exclude=exclude,
                ignore_errors=ignore_errors,
            )
        except (FileNotFoundError, NotADirectoryError):
            # Nonexistent local path → everything in repo is a delete
            dest = _normalize_path(repo_path) if repo_path else ""
            repo_files = _walk_repo(fs, dest)
            if not repo_files and dest and fs.exists(dest) and not fs.is_dir(dest):
                entry = _entry_at_path(fs._store._repo, fs._tree_oid, dest)
                file_entry = FileEntry.from_mode(dest, entry[1]) if entry else FileEntry(dest, FileType.BLOB)
                fs._changes = ChangeReport(delete=[file_entry])
                return fs
            delete_list = sorted(repo_files.keys())
            delete_entries = _make_entries_from_repo(fs, delete_list, dest)
            fs._changes = _finalize_changes(ChangeReport(delete=delete_entries))
            return fs

    try:
        return _copy_in(
            fs, [_ensure_trailing_slash(local_path)], repo_path,
            message=message, delete=True, ignore_errors=ignore_errors,
            checksum=checksum, exclude=exclude, operation="sync",
            parents=parents,
        )
    except (FileNotFoundError, NotADirectoryError):
        # Nonexistent local path → treat as empty source (delete everything)
        new_fs, delete_rels, is_file = _sync_delete_all_in_repo(fs, repo_path, message=message, parents=parents)
        if not delete_rels:
            new_fs._changes = None
            return new_fs
        # Convert string list to FileEntry list
        # For file deletes, rels are full paths (base=""); for dirs, relative to repo_path
        base = "" if is_file else repo_path
        changes = ChangeReport(delete=_make_entries_from_repo(fs, delete_rels, base))
        new_fs._changes = changes
        return new_fs


def _sync_delete_all_in_repo(
    fs: FS, repo_path: str, *, message: str | None = None,
    parents: list[FS] | None = None,
) -> tuple[FS, list[str], bool]:
    """Delete all files under *repo_path* (used when sync source is empty).

    Returns ``(new_fs, deleted_rels, is_file)`` where *deleted_rels* are
    relative to *repo_path* for directories, or full paths for single files.
    *is_file* is ``True`` when *repo_path* was a single file.
    """
    dest = _normalize_path(repo_path) if repo_path else ""
    repo_files = _walk_repo(fs, dest)
    if not repo_files:
        # _walk_repo returns {} for files (not dirs) — check if dest is a file
        if dest and fs.exists(dest) and not fs.is_dir(dest):
            with fs.batch(message=message, operation="sync", parents=parents) as b:
                b.remove(dest)
            return b.fs, [dest], True
        return fs, [], False
    with fs.batch(message=message, parents=parents) as b:
        for rel in sorted(repo_files):
            full = f"{dest}/{rel}" if dest else rel
            b.remove(full)
    return b.fs, sorted(repo_files.keys()), False


def _sync_out(
    fs: FS, repo_path: str, local_path: str, *,
    dry_run: bool = False,
    ignore_errors: bool = False,
    checksum: bool = True,
) -> FS:
    """Make *local_path* identical to *repo_path*. Returns ``FS`` with ``.changes`` set.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.
    """
    if dry_run:
        try:
            sources = [_ensure_trailing_slash(repo_path)] if repo_path else [""]
            return _copy_out(fs, sources, local_path, dry_run=True,
                             delete=True, checksum=checksum,
                             ignore_errors=ignore_errors)
        except (FileNotFoundError, NotADirectoryError):
            # Nonexistent repo path → everything local is a delete
            local_paths = sorted(_walk_local_paths(local_path))
            # For local files being deleted, we don't have type info, mark as blobs
            delete_entries = [FileEntry(p, FileType.BLOB) for p in local_paths]
            fs._changes = _finalize_changes(ChangeReport(delete=delete_entries))
            return fs

    try:
        sources = [_ensure_trailing_slash(repo_path)] if repo_path else [""]
        return _copy_out(fs, sources, local_path, delete=True,
                         ignore_errors=ignore_errors, checksum=checksum)
    except (FileNotFoundError, NotADirectoryError):
        # Nonexistent repo path → treat as empty source (delete everything local)
        delete_rels = _sync_delete_all_local(local_path)
        if not delete_rels:
            fs._changes = None
            return fs
        # For local file deletes, we don't have type info - just mark as blobs
        fs._changes = ChangeReport(delete=[FileEntry(rel, FileType.BLOB) for rel in delete_rels])
        return fs


def _sync_delete_all_local(local_path: str) -> list[str]:
    """Delete all files under *local_path* and prune empty dirs.

    Returns sorted list of deleted relative paths.
    """
    base = Path(local_path)
    base.mkdir(parents=True, exist_ok=True)
    deleted = sorted(_walk_local_paths(local_path))
    for rel in deleted:
        out = base / rel
        if out.exists() or out.is_symlink():
            out.unlink()
    _prune_empty_dirs(base)
    return deleted


# ---------------------------------------------------------------------------
# Move (repo-internal)
# ---------------------------------------------------------------------------

def _resolve_move(
    fs: FS,
    sources: list[str],
    dest: str,
    *,
    recursive: bool = False,
) -> tuple[list[tuple[str, str]], list[str]]:
    """Common resolution for move operations.

    Returns ``(pairs, delete_paths)`` where *pairs* is a list of
    ``(src_repo_path, dest_repo_path)`` and *delete_paths* is the
    list of source paths to remove.

    Implements POSIX mv semantics: when there is a single source file
    and *dest* is not an existing directory and does not end with ``/``,
    the dest is the exact target path (rename).  Otherwise files are
    placed inside *dest*.
    """
    resolved = _resolve_repo_sources(fs, sources)

    dest_norm = _normalize_path(dest.rstrip("/")) if dest.rstrip("/") else ""
    dest_exists_as_dir = dest_norm and fs.is_dir(dest_norm)

    # POSIX mv rename: single source, dest not ending with "/", dest not
    # an existing directory → rename (file or directory).
    is_rename = (
        len(resolved) == 1
        and resolved[0][1] in ("file", "dir")
        and not dest.endswith("/")
        and not dest_exists_as_dir
    )

    if is_rename and resolved[0][1] == "file":
        src_path = resolved[0][0]
        dest_path = dest_norm if dest_norm else src_path.rsplit("/", 1)[-1]
        pairs = [(src_path, dest_path)]
    elif is_rename and resolved[0][1] == "dir":
        # Directory rename: treat as "contents of src" → dest
        renamed = [(resolved[0][0], "contents", resolved[0][2])]
        pairs = _enum_repo_to_repo(fs, renamed, dest_norm)
    else:
        # Multi-source or dest is existing dir or ends with "/"
        enum_dest = dest_norm
        pairs = _enum_repo_to_repo(fs, resolved, enum_dest)

    if not pairs:
        raise FileNotFoundError(f"No matches for patterns: {sources}")

    # Validate: no src == dest
    for src, dst in pairs:
        if src == dst:
            raise ValueError(f"Source and destination are the same: {src}")

    # Collect all source paths for deletion
    delete_paths = _collect_remove_paths(fs, sources, recursive=recursive)

    return pairs, delete_paths


def _move(
    fs: FS,
    sources: str | list[str],
    dest: str,
    *,
    dry_run: bool = False,
    recursive: bool = False,
    message: str | None = None,
    parents: list[FS] | None = None,
) -> FS:
    """Move/rename files within the repo. Returns ``FS``.

    With ``dry_run=True``, no changes are written; the input *fs* is
    returned with ``.changes`` set.
    """
    if isinstance(sources, str):
        sources = [sources]
    pairs, delete_paths = _resolve_move(fs, sources, dest, recursive=recursive)

    # Build changes
    changes = ChangeReport()
    dest_rel_to_repo = {dp: dp for _, dp in pairs}
    changes.add = _make_entries_from_repo_dict(fs, [dp for _, dp in pairs], dest_rel_to_repo)
    src_rel_to_repo = {p: p for p in delete_paths}
    changes.delete = _make_entries_from_repo_dict(fs, delete_paths, src_rel_to_repo)

    if dry_run:
        fs._changes = _finalize_changes(changes)
        return fs

    # Execute: write dest files from source blob data, then remove sources
    with fs.batch(message=message, operation="mv", parents=parents) as b:
        for src, dst in pairs:
            _copy_blob_to_batch(b, fs, src, dst)
        for path in delete_paths:
            b.remove(path)

    result_fs = b.fs
    result_fs._changes = _finalize_changes(changes)
    return result_fs
