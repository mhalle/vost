"""The sync command."""

from __future__ import annotations

import os

import click

from ..copy._io import _copy_blob_to_batch
from ..exceptions import StaleSnapshotError
from ..tree import _entry_at_path
from ._helpers import (
    main,
    _parse_ref_path,
    _resolve_ref_path,
    _require_writable_ref,
    _check_ref_conflicts,
    _repo_option,
    _branch_option,
    _message_option,
    _dry_run_option,
    _checksum_option,
    _ignore_errors_option,
    _no_create_option,
    _require_repo,
    _status,
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
@_dry_run_option
@click.option("--exclude", multiple=True,
              help="Exclude files matching pattern (gitignore syntax, repeatable).")
@click.option("--exclude-from", "exclude_from", type=click.Path(exists=True),
              help="Read exclude patterns from file.")
@click.option("--gitignore", "use_gitignore", is_flag=True, default=False,
              help="Read .gitignore files from source tree (disk→repo only).")
@_ignore_errors_option
@_checksum_option
@_no_create_option
@_tag_option
@click.option("--watch", "watch", is_flag=True, default=False,
              help="Watch for changes and sync continuously (disk→repo only).")
@click.option("--debounce", type=int, default=2000,
              help="Debounce delay in ms for --watch (default: 2000).")
@click.option("--parent", "parent_refs", multiple=True,
              help="Additional parent ref (branch/tag/hash). Repeatable.")
@click.pass_context
def sync(ctx, args, branch, ref, at_path, match_pattern, before, back, message, dry_run, exclude, exclude_from, use_gitignore, ignore_errors, checksum, no_create, tag, force_tag, watch, debounce, parent_refs):
    """Make one path identical to another (like rsync --delete).

    Requires --repo or VOST_REPO environment variable.

    With one argument, syncs a local directory to the repo root:

        vost --repo path/to/repo.git sync ./dir

    \b
    With two arguments, direction is determined by the ':' prefix:
        vost sync ./local :repo_path   (disk → repo)
        vost sync :repo_path ./local   (repo → disk)
        vost sync session:/ :          (repo → repo, cross-branch)
    """
    from ..copy import ExcludeFilter

    if len(args) == 1:
        # 1-arg form: sync local dir to repo root
        rp = _parse_ref_path(args[0])
        if rp.is_repo:
            raise click.ClickException(
                "Single-argument sync must be a local path, not a repo path"
            )
        local_path = args[0]
        repo_dest = ""
        direction = "to_repo"
    elif len(args) == 2:
        src_raw, dest_raw = args
        src_rp = _parse_ref_path(src_raw)
        dest_rp = _parse_ref_path(dest_raw)

        if src_rp.is_repo and dest_rp.is_repo:
            direction = "repo_to_repo"
        elif not src_rp.is_repo and not dest_rp.is_repo:
            raise click.ClickException(
                "Neither argument is a repo path — prefix repo paths with ':'"
            )
        elif not src_rp.is_repo:
            # disk → repo
            local_path = src_raw
            repo_dest = dest_rp.path.rstrip("/")
            direction = "to_repo"
        else:
            # repo → disk
            repo_dest = src_rp.path.rstrip("/")
            local_path = dest_raw
            direction = "from_repo"
    else:
        raise click.ClickException("sync requires 1 or 2 arguments")

    has_snapshot_filters = ref or at_path or match_pattern or before or back
    if has_snapshot_filters and direction == "to_repo":
        raise click.ClickException(
            "--ref/--path/--match/--before only apply when reading from repo"
        )
    if tag and direction in ("from_repo", "repo_to_repo"):
        raise click.ClickException(
            "--tag only applies when writing to repo (disk → repo)"
        )
    if (exclude or exclude_from) and direction != "to_repo":
        raise click.ClickException(
            "--exclude/--exclude-from only apply when syncing from disk to repo"
        )
    if use_gitignore and direction != "to_repo":
        raise click.ClickException(
            "--gitignore only applies when syncing from disk to repo"
        )

    # Check for conflicts between explicit ref:path and flags
    if direction == "from_repo":
        _check_ref_conflicts([src_rp], ref=ref, branch=branch, back=back,
                             before=before, at_path=at_path, match_pattern=match_pattern)
    elif direction == "repo_to_repo":
        _check_ref_conflicts([src_rp, dest_rp], ref=ref, branch=branch, back=back,
                             before=before, at_path=at_path, match_pattern=match_pattern)

    # Build exclude filter (disk→repo only)
    excl = None
    if exclude or exclude_from or use_gitignore:
        excl = ExcludeFilter(patterns=exclude, exclude_from=exclude_from,
                             gitignore=use_gitignore)

    # --watch validation
    if watch:
        if dry_run:
            raise click.ClickException("--watch and --dry-run are incompatible")
        if direction != "to_repo":
            raise click.ClickException("--watch only supports disk → repo")
        if debounce < 100:
            raise click.ClickException("--debounce must be at least 100 ms")

    repo_path = _require_repo(ctx)

    if direction == "repo_to_repo":
        # ---- Repo → repo sync ----
        store = _open_store(repo_path)
        branch = branch or _current_branch(store)
        parents = [_get_fs(store, None, r) for r in parent_refs] if parent_refs else None

        # Resolve source
        if src_rp.ref or src_rp.back:
            src_fs = _resolve_ref_path(store, src_rp, ref, branch,
                                       at_path=at_path, match_pattern=match_pattern,
                                       before=before, back=back)
        else:
            src_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                                 match_pattern=match_pattern, before=before, back=back)

        # Resolve dest (must be writable branch)
        dest_fs, dest_branch = _require_writable_ref(store, dest_rp, branch)

        src_repo_path = src_rp.path.rstrip("/")
        dest_repo_path = dest_rp.path.rstrip("/")

        _sync_repo_to_repo(ctx, store, src_fs, dest_fs, src_repo_path,
                           dest_repo_path, dry_run, message, ignore_errors,
                           parents=parents)
        return

    if direction == "to_repo" and not dry_run and not no_create:
        store = _open_or_create_store(repo_path, branch or "main")
        branch = branch or _current_branch(store)
    else:
        store = _open_store(repo_path)
        branch = branch or _current_branch(store)

    parents = [_get_fs(store, None, r) for r in parent_refs] if parent_refs else None

    if watch:
        from ._watch import watch_and_sync
        watch_and_sync(store, branch, local_path, repo_dest,
                       debounce=debounce, message=message,
                       ignore_errors=ignore_errors, checksum=checksum,
                       exclude=excl)
        return

    if direction == "from_repo" and (src_rp.ref or src_rp.back):
        fs = _resolve_ref_path(store, src_rp, ref, branch,
                               at_path=at_path, match_pattern=match_pattern,
                               before=before, back=back)
    else:
        fs = _resolve_fs(store, branch, ref, at_path=at_path,
                         match_pattern=match_pattern, before=before, back=back)

    try:
        if direction == "to_repo":
            if dry_run:
                _dry_fs = fs.sync_in(local_path, repo_dest,
                                     dry_run=True,
                                     checksum=checksum,
                                     exclude=excl)
                changes = _dry_fs.changes
                if changes:
                    for w in changes.warnings:
                        click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                    for action in changes.actions():
                        prefix = {"add": "+", "update": "~", "delete": "-"}[action.action]
                        if repo_dest and action.path:
                            click.echo(f"{prefix} :{repo_dest}/{action.path}")
                        else:
                            click.echo(f"{prefix} :{repo_dest or ''}{action.path}")
            else:
                _new_fs = fs.sync_in(
                    local_path, repo_dest,
                    message=message, ignore_errors=ignore_errors,
                    checksum=checksum, exclude=excl,
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
                _status(ctx, f"Synced -> :{repo_dest or '/'}")
                if changes and changes.errors:
                    ctx.exit(1)
        else:
            if dry_run:
                _dry_fs = fs.sync_out(repo_dest, local_path,
                                      dry_run=True,
                                      checksum=checksum)
                changes = _dry_fs.changes
                if changes:
                    for w in changes.warnings:
                        click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                    for action in changes.actions():
                        prefix = {"add": "+", "update": "~", "delete": "-"}[action.action]
                        click.echo(f"{prefix} {os.path.join(local_path, action.path)}")
            else:
                _result_fs = fs.sync_out(
                    repo_dest, local_path,
                    ignore_errors=ignore_errors,
                    checksum=checksum,
                )
                changes = _result_fs.changes
                if changes:
                    for w in changes.warnings:
                        click.echo(f"WARNING: {w.path}: {w.error}", err=True)
                    for e in changes.errors:
                        click.echo(f"ERROR: {e.path}: {e.error}", err=True)
                _status(ctx, f"Synced -> {local_path}")
                if changes and changes.errors:
                    ctx.exit(1)
    except (FileNotFoundError, NotADirectoryError) as exc:
        raise click.ClickException(str(exc))
    except StaleSnapshotError:
        raise click.ClickException("Branch modified concurrently — retry")


def _sync_repo_to_repo(ctx, store, src_fs, dest_fs, src_repo_path, dest_repo_path,
                         dry_run, message, ignore_errors, *, parents=None):
    """Sync one repo path to another (like rsync --delete between refs)."""
    from ..copy._resolve import _walk_repo

    # Walk source and dest trees
    src_files = _walk_repo(src_fs, src_repo_path)
    dest_files = _walk_repo(dest_fs, dest_repo_path)

    src_keys = set(src_files.keys())
    dest_keys = set(dest_files.keys())

    to_add = src_keys - dest_keys
    to_delete = dest_keys - src_keys
    to_check = src_keys & dest_keys
    to_update = {k for k in to_check if src_files[k] != dest_files[k]}

    if not to_add and not to_delete and not to_update:
        return

    try:
        if dry_run:
            for p in sorted(to_delete):
                full = f"{dest_repo_path}/{p}" if dest_repo_path else p
                click.echo(f"- :{full}")
            for p in sorted(to_add):
                full = f"{dest_repo_path}/{p}" if dest_repo_path else p
                click.echo(f"+ :{full}")
            for p in sorted(to_update):
                full = f"{dest_repo_path}/{p}" if dest_repo_path else p
                click.echo(f"~ :{full}")
        else:
            with dest_fs.batch(message=message, operation="sync", parents=parents) as b:
                # Delete files not in source
                for p in sorted(to_delete):
                    full = f"{dest_repo_path}/{p}" if dest_repo_path else p
                    try:
                        b.remove(full)
                    except (FileNotFoundError, IsADirectoryError):
                        pass

                # Add/update files from source
                for p in sorted(to_add | to_update):
                    src_full = f"{src_repo_path}/{p}" if src_repo_path else p
                    dest_full = f"{dest_repo_path}/{p}" if dest_repo_path else p
                    _copy_blob_to_batch(b, src_fs, src_full, dest_full)

            _status(ctx, f"Synced -> :{dest_repo_path or '/'}")
    except (FileNotFoundError, NotADirectoryError) as exc:
        raise click.ClickException(str(exc))
    except StaleSnapshotError:
        raise click.ClickException("Branch modified concurrently — retry")
