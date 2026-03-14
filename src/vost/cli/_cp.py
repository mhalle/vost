"""The cp command."""

from __future__ import annotations

import os
from pathlib import Path

import click

from ..copy._io import _copy_blob_to_batch
from ..copy._types import FileType
from ..exceptions import StaleSnapshotError
from ..tree import GIT_FILEMODE_BLOB_EXECUTABLE, _entry_at_path
from ._helpers import (
    main,
    _parse_ref_path,
    _resolve_ref_path,
    _require_writable_ref,
    _check_ref_conflicts,
    _expand_sources_disk,
    _expand_sources_repo,
    _repo_option,
    _branch_option,
    _message_option,
    _dry_run_option,
    _checksum_option,
    _ignore_errors_option,
    _no_glob_option,
    _no_create_option,
    _require_repo,
    _status,
    _normalize_repo_path,
    _open_store,
    _open_or_create_store,
    _current_branch,
    _get_fs,
    _resolve_fs,
    _snapshot_options,
    _tag_option,
    _apply_tag,
)


@main.command()
@_repo_option
@click.argument("args", nargs=-1, required=True)
@_branch_option
@_snapshot_options
@_message_option
@click.option("--type", "file_type", type=click.Choice(["blob", "executable"]),
              default=None, help="File type (default: auto-detect from disk).")
@click.option("--mode", "deprecated_mode", type=click.Choice(["644", "755"]),
              default=None, hidden=True)
@click.option("--follow-symlinks", is_flag=True, default=False,
              help="Follow symlinks instead of preserving them (disk→repo only).")
@_dry_run_option
@click.option("--ignore-existing", is_flag=True, default=False,
              help="Skip files that already exist at the destination.")
@click.option("--delete", is_flag=True, default=False,
              help="Delete destination files not present in source (like rsync --delete).")
@click.option("--exclude", multiple=True,
              help="Exclude files matching pattern (gitignore syntax, repeatable).")
@click.option("--exclude-from", "exclude_from", type=click.Path(exists=True),
              help="Read exclude patterns from file.")
@_ignore_errors_option
@_checksum_option
@_no_glob_option
@_no_create_option
@_tag_option
@click.option("--parent", "parent_refs", multiple=True,
              help="Additional parent ref (branch/tag/hash). Repeatable.")
@click.pass_context
def cp(ctx, args, branch, ref, at_path, match_pattern, before, back, message, file_type, deprecated_mode, follow_symlinks, dry_run, ignore_existing, delete, ignore_errors, checksum, no_glob, no_create, tag, force_tag, exclude, exclude_from, parent_refs):
    """Copy files and directories between disk and repo, or between repo refs.

    Requires --repo or VOST_REPO environment variable.

    The last argument is the destination; all preceding arguments are sources.
    Directories are copied recursively with their name preserved.
    A trailing '/' on a source means "contents of" (like rsync).
    Glob patterns (* and ?) are expanded; they do not match leading dots.

    \b
    Prefix repo-side paths with ':' (current branch) or 'ref:' (explicit ref).
    Examples:
        vost cp file.txt :               # disk → repo
        vost cp :file.txt ./             # repo → disk
        vost cp '*.jpg' :images/         # disk → repo with glob
        vost cp session:/ :              # repo → repo (cross-branch)
        vost cp main~1:a.txt :backup/    # from 1 commit back on main
    """
    from ..copy import ExcludeFilter
    from ..copy._resolve import _resolve_repo_sources, _enum_repo_to_repo

    if len(args) < 2:
        raise click.ClickException("cp requires at least two arguments (SRC... DEST)")

    raw_sources = args[:-1]
    raw_dest = args[-1]

    # Parse all args through _parse_ref_path
    parsed_sources = [_parse_ref_path(s) for s in raw_sources]
    parsed_dest = _parse_ref_path(raw_dest)

    any_src_repo = any(rp.is_repo for rp in parsed_sources)
    all_src_repo = all(rp.is_repo for rp in parsed_sources)
    all_src_local = all(not rp.is_repo for rp in parsed_sources)
    dest_is_repo = parsed_dest.is_repo

    # Determine direction
    if all_src_local and not dest_is_repo:
        raise click.ClickException(
            "Neither sources nor DEST is a repo path — prefix repo paths with ':'"
        )
    if all_src_local and dest_is_repo:
        direction = "disk_to_repo"
    elif all_src_repo and not dest_is_repo:
        direction = "repo_to_disk"
    elif any_src_repo and not all_src_repo and not dest_is_repo:
        raise click.ClickException(
            "All source paths must be the same type (all repo or all local)"
        )
    elif all_src_repo and dest_is_repo:
        direction = "repo_to_repo"
    elif any_src_repo and not all_src_repo and dest_is_repo:
        raise click.ClickException(
            "Mixed local and repo sources are not supported — use separate cp commands"
        )
    else:
        raise click.ClickException(
            "Neither sources nor DEST is a repo path — prefix repo paths with ':'"
        )

    # Check for conflicts between explicit ref:path and flags
    _check_ref_conflicts(parsed_sources + [parsed_dest],
                         ref=ref, branch=branch, back=back,
                         before=before, at_path=at_path, match_pattern=match_pattern)

    has_snapshot_filters = ref or at_path or match_pattern or before or back
    if has_snapshot_filters and direction == "disk_to_repo":
        raise click.ClickException(
            "--ref/--path/--match/--before only apply when reading from repo"
        )
    if tag and direction != "disk_to_repo":
        raise click.ClickException(
            "--tag only applies when writing to repo (disk -> repo)"
        )
    if (exclude or exclude_from) and direction != "disk_to_repo":
        raise click.ClickException(
            "--exclude/--exclude-from only apply when copying from disk to repo"
        )

    # Build exclude filter (disk→repo only)
    excl = None
    if exclude or exclude_from:
        excl = ExcludeFilter(patterns=exclude, exclude_from=exclude_from)

    repo_path = _require_repo(ctx)

    # Resolve advisory parent refs (deferred until store is open)
    _parent_refs = parent_refs

    if file_type:
        filemode = FileType(file_type).filemode
    elif deprecated_mode:
        filemode = FileType.EXECUTABLE.filemode if deprecated_mode == "755" else FileType.BLOB.filemode
    else:
        filemode = None

    # Detect single-plain-file case (no glob, no trailing slash, no directory,
    # no /./  pivot).  In this case, like standard `cp`, the dest is the exact
    # path, not a parent directory.
    single_file_src = (
        len(raw_sources) == 1
        and "*" not in raw_sources[0] and "?" not in raw_sources[0]
        and not raw_sources[0].endswith("/")
        and "/./" not in raw_sources[0]
    )

    if direction == "disk_to_repo":
        # ---- Disk → repo ----
        if not dry_run and not no_create:
            store = _open_or_create_store(repo_path, branch or "main")
            branch = branch or _current_branch(store)
        else:
            store = _open_store(repo_path)
            branch = branch or _current_branch(store)

        # Resolve dest ref
        if parsed_dest.ref:
            dest_fs, branch = _require_writable_ref(store, parsed_dest, branch)
        else:
            dest_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                                  match_pattern=match_pattern, before=before, back=back)
        fs = dest_fs
        parents = [_get_fs(store, None, r) for r in _parent_refs] if _parent_refs else None

        dest_path = parsed_dest.path
        if dest_path:
            dest_path = dest_path.rstrip("/")
            if dest_path:
                dest_path = _normalize_repo_path(dest_path)

        src_raw = raw_sources[0]
        is_single_file = single_file_src and os.path.isfile(src_raw)

        if is_single_file:
            if delete:
                raise click.ClickException(
                    "Cannot use --delete with a single file source."
                )
            # Check exclude filter for single-file mode
            if excl is not None and excl.active and excl.is_excluded(os.path.basename(src_raw)):
                return
            # Single file: dest is the exact repo path, unless dest is an
            # existing directory — then place the file inside it.
            local = Path(src_raw)
            if dest_path and fs.is_dir(dest_path):
                repo_file = _normalize_repo_path(f"{dest_path}/{local.name}")
            elif dest_path:
                repo_file = dest_path
            else:
                repo_file = _normalize_repo_path(local.name)
            if ignore_existing and fs.exists(repo_file):
                return
            try:
                if dry_run:
                    click.echo(f"{local} -> :{repo_file}")
                else:
                    with fs.batch(message=message, operation="cp", parents=parents) as b:
                        if not follow_symlinks and local.is_symlink():
                            b.write_symlink(repo_file, os.readlink(local))
                        else:
                            b.write_from_file(repo_file, local, mode=filemode)
                    if tag:
                        _apply_tag(store, b.fs, tag, force_tag)
                    _status(ctx, f"Copied -> :{repo_file}")
            except (FileNotFoundError, OSError) as exc:
                if ignore_errors:
                    click.echo(f"ERROR: {local}: {exc}", err=True)
                    ctx.exit(1)
                else:
                    raise click.ClickException(str(exc))
            except StaleSnapshotError:
                raise click.ClickException("Branch modified concurrently — retry")
        else:
            source_paths = list(raw_sources)
            if not no_glob:
                source_paths = _expand_sources_disk(source_paths)
            try:
                if dry_run:
                    _dry_fs = fs.copy_in(
                        source_paths, dest_path,
                        dry_run=True,
                        follow_symlinks=follow_symlinks,
                        ignore_existing=ignore_existing,
                        delete=delete,
                        checksum=checksum,
                        exclude=excl,
                    )
                    changes = _dry_fs.changes
                    if changes:
                        for w in changes.warnings:
                            click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                        for action in changes.actions():
                            prefix = {"add": "+", "update": "~", "delete": "-"}[action.action]
                            if dest_path and action.path:
                                click.echo(f"{prefix} :{dest_path}/{action.path}")
                            else:
                                click.echo(f"{prefix} :{dest_path or ''}{action.path}")
                else:
                    _new_fs = fs.copy_in(
                        source_paths, dest_path,
                        follow_symlinks=follow_symlinks,
                        message=message, mode=filemode,
                        ignore_existing=ignore_existing,
                        delete=delete,
                        ignore_errors=ignore_errors,
                        checksum=checksum,
                        exclude=excl,
                        parents=parents,
                    )
                    changes = _new_fs.changes
                    if changes:
                        for w in changes.warnings:
                            click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                        for e in changes.errors:
                            click.echo(f"ERROR: {e.path}: {e.error}", err=True)
                    if tag:
                        _apply_tag(store, _new_fs, tag, force_tag)
                    _status(ctx, f"Copied -> :{dest_path or '/'}")
                    if changes and changes.errors:
                        ctx.exit(1)
            except (FileNotFoundError, NotADirectoryError) as exc:
                raise click.ClickException(str(exc))
            except StaleSnapshotError:
                raise click.ClickException("Branch modified concurrently — retry")

    elif direction == "repo_to_disk":
        # ---- Repo → disk ----
        store = _open_store(repo_path)
        branch = branch or _current_branch(store)
        default_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                                 match_pattern=match_pattern, before=before, back=back)

        dest_path = raw_dest

        # For per-source ref grouping: resolve each source's FS
        # For backward compat, when all sources use the default ref, use single FS
        all_default_ref = all(not rp.ref for rp in parsed_sources)

        if all_default_ref:
            fs = default_fs
            source_paths = [rp.path for rp in parsed_sources]

            src_raw = source_paths[0]
            is_single_repo_file = (
                single_file_src and src_raw
                and not fs.is_dir(_normalize_repo_path(src_raw))
            )

            if is_single_repo_file:
                _cp_single_repo_to_disk(ctx, fs, src_raw, dest_path, dry_run,
                                         delete, ignore_existing, ignore_errors)
            else:
                _cp_multi_repo_to_disk(ctx, fs, source_paths, dest_path, dry_run,
                                        delete, ignore_existing, ignore_errors, checksum,
                                        no_glob)
        else:
            # Per-source ref grouping
            for rp in parsed_sources:
                if rp.is_repo and (rp.ref or rp.back):
                    src_fs = _resolve_ref_path(store, rp, ref, branch,
                                               at_path=at_path, match_pattern=match_pattern,
                                               before=before, back=back)
                else:
                    src_fs = default_fs
                source_paths = [rp.path]

                src_raw = rp.path
                is_single = (
                    single_file_src and src_raw
                    and not src_fs.is_dir(_normalize_repo_path(src_raw))
                )
                if is_single:
                    _cp_single_repo_to_disk(ctx, src_fs, src_raw, dest_path, dry_run,
                                             delete, ignore_existing, ignore_errors)
                else:
                    _cp_multi_repo_to_disk(ctx, src_fs, source_paths, dest_path, dry_run,
                                            delete, ignore_existing, ignore_errors, checksum,
                                            no_glob)

    elif direction == "repo_to_repo":
        # ---- Repo → repo ----
        store = _open_store(repo_path)
        branch = branch or _current_branch(store)
        parents = [_get_fs(store, None, r) for r in _parent_refs] if _parent_refs else None

        # Resolve dest
        dest_fs, dest_branch = _require_writable_ref(store, parsed_dest, branch)

        dest_path = parsed_dest.path
        if dest_path:
            dest_path = dest_path.rstrip("/")
            if dest_path:
                dest_path = _normalize_repo_path(dest_path)

        # Resolve source FS(es) and enumerate files
        default_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                                 match_pattern=match_pattern, before=before, back=back)

        # Collect all (src_repo_path, dest_repo_path) pairs across source groups
        all_pairs: list[tuple] = []  # (src_fs, src_path, dest_path)
        for rp in parsed_sources:
            if rp.is_repo and (rp.ref or rp.back):
                src_fs = _resolve_ref_path(store, rp, ref, branch,
                                           at_path=at_path, match_pattern=match_pattern,
                                           before=before, back=back)
            else:
                src_fs = default_fs
            src_path = rp.path if rp.is_repo else rp.path
            src_paths = _expand_sources_repo(src_fs, [src_path]) if not no_glob else [src_path]
            resolved = _resolve_repo_sources(src_fs, src_paths)
            pairs = _enum_repo_to_repo(src_fs, resolved, dest_path or "")
            for sp, dp in pairs:
                all_pairs.append((src_fs, sp, dp))

        if not all_pairs and not delete:
            return

        # Precompute delete set and prefix for key-space normalization
        _pfx = (dest_path + "/") if dest_path else ""
        deleted_set: set[str] = set()
        if delete:
            from ..copy._resolve import _walk_repo
            dest_files = set(_walk_repo(dest_fs, dest_path or "").keys())
            src_dest_rels = {
                dp[len(_pfx):] if _pfx and dp.startswith(_pfx) else dp
                for _, _, dp in all_pairs
            }
            to_delete = sorted(dest_files - src_dest_rels)
            deleted_set = set(to_delete)

        try:
            if dry_run:
                if delete:
                    for rel in to_delete:
                        full = f"{dest_path}/{rel}" if dest_path else rel
                        click.echo(f"- :{full}")
                for src_fs, sp, dp in all_pairs:
                    entry = _entry_at_path(src_fs._store._repo, src_fs._tree_oid, sp)
                    if entry is None:
                        continue
                    dp_rel = dp[len(_pfx):] if _pfx and dp.startswith(_pfx) else dp
                    if ignore_existing and dest_fs.exists(dp) and dp_rel not in deleted_set:
                        continue
                    # Check if it's add or update
                    if dest_fs.exists(dp):
                        click.echo(f"~ :{dp}")
                    else:
                        click.echo(f"+ :{dp}")
            else:
                with dest_fs.batch(message=message, operation="cp", parents=parents) as b:
                    # Handle --delete
                    if delete:
                        for rel in to_delete:
                            full = f"{dest_path}/{rel}" if dest_path else rel
                            try:
                                b.remove(full)
                            except (FileNotFoundError, IsADirectoryError):
                                pass

                    for src_fs, sp, dp in all_pairs:
                        dp_rel = dp[len(_pfx):] if _pfx and dp.startswith(_pfx) else dp
                        if ignore_existing and dest_fs.exists(dp) and dp_rel not in deleted_set:
                            continue
                        _copy_blob_to_batch(b, src_fs, sp, dp, filemode=filemode)
                _status(ctx, f"Copied -> :{dest_path or '/'}")
        except (FileNotFoundError, NotADirectoryError) as exc:
            raise click.ClickException(str(exc))
        except StaleSnapshotError:
            raise click.ClickException("Branch modified concurrently — retry")


def _cp_single_repo_to_disk(ctx, fs, src_raw, dest_path, dry_run,
                              delete, ignore_existing, ignore_errors):
    """Handle single repo file → disk copy."""
    if delete:
        raise click.ClickException(
            "Cannot use --delete with a single file source."
        )
    src_path = _normalize_repo_path(src_raw)
    if not fs.exists(src_path):
        raise click.ClickException(f"File not found in repo: {src_path}")
    local_dest = Path(dest_path)
    if local_dest.is_dir():
        out = local_dest / Path(src_path).name
    else:
        out = local_dest
    if ignore_existing and out.exists():
        return
    if dry_run:
        click.echo(f":{src_path} -> {out}")
    else:
        try:
            out.parent.mkdir(parents=True, exist_ok=True)
            if out.exists() or out.is_symlink():
                out.unlink()
            entry = _entry_at_path(fs._store._repo, fs._tree_oid, src_path)
            if entry and FileType.from_filemode(entry[1]) == FileType.LINK:
                target = fs.readlink(src_path)
                out.symlink_to(target)
            else:
                out.write_bytes(fs.read(src_path))
                if entry and entry[1] == GIT_FILEMODE_BLOB_EXECUTABLE:
                    os.chmod(out, 0o755)
            cts = fs._store._repo[fs._commit_oid].commit_time
            os.utime(out, (cts, cts), follow_symlinks=False)
        except OSError as exc:
            if ignore_errors:
                click.echo(f"ERROR: {out}: {exc}", err=True)
                ctx.exit(1)
            else:
                raise click.ClickException(f"Cannot write {out}: {exc}")
        else:
            _status(ctx, f"Copied :{src_path} -> {out}")


def _cp_multi_repo_to_disk(ctx, fs, source_paths, dest_path, dry_run,
                             delete, ignore_existing, ignore_errors, checksum,
                             no_glob):
    """Handle multi-file repo → disk copy."""
    if not no_glob:
        source_paths = _expand_sources_repo(fs, source_paths)
    try:
        if dry_run:
            result_fs = fs.copy_out(
                source_paths, dest_path,
                dry_run=True,
                ignore_existing=ignore_existing,
                delete=delete,
                checksum=checksum,
            )
            changes = result_fs.changes
            if changes:
                for w in changes.warnings:
                    click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                for action in changes.actions():
                    prefix = {"add": "+", "update": "~", "delete": "-"}[action.action]
                    click.echo(f"{prefix} {os.path.join(dest_path, action.path)}")
        else:
            result_fs = fs.copy_out(
                source_paths, dest_path,
                ignore_existing=ignore_existing,
                delete=delete,
                ignore_errors=ignore_errors,
                checksum=checksum,
            )
            changes = result_fs.changes
            if changes:
                for w in changes.warnings:
                    click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                for e in changes.errors:
                    click.echo(f"ERROR: {e.path}: {e.error}", err=True)
            _status(ctx, f"Copied -> {dest_path}")
            if changes and changes.errors:
                ctx.exit(1)
    except (FileNotFoundError, NotADirectoryError) as exc:
        raise click.ClickException(str(exc))
    except OSError as exc:
        raise click.ClickException(str(exc))
