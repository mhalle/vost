"""Basic commands: init, destroy, gc, ls, cat, rm, mv, write, log, diff, cmp, undo, redo, reflog."""

from __future__ import annotations

import io
import json
import os
import sys
from pathlib import Path as _Path

import click

from ..copy._resolve import _walk_repo
from ..copy._types import FileType
from ..exceptions import StaleSnapshotError
from ..repo import GitStore
from ..tree import (
    WalkEntry,
    _entry_at_path,
    _normalize_path,
    list_entries_at_path,
)
from ._helpers import (
    main,
    RefPath,
    _parse_ref_path,
    _resolve_ref_path,
    _require_writable_ref,
    _resolve_same_branch,
    _check_ref_conflicts,
    _expand_sources_repo,
    _repo_option,
    _branch_option,
    _message_option,
    _dry_run_option,
    _format_option,
    _require_repo,
    _status,
    _strip_colon,
    _normalize_repo_path,
    _open_store,
    _open_or_create_store,
    _current_branch,
    _get_branch_fs,
    _get_fs,
    _normalize_at_path,
    _parse_before,
    _resolve_fs,
    _log_entry_dict,
    _no_create_option,
    _no_glob_option,
    _snapshot_options,
    _tag_option,
    _apply_tag,
)


# ---------------------------------------------------------------------------
# init
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.option("--branch", "-b", default="main", help="Initial branch name (default: main).")
@click.option("-f", "--force", is_flag=True, help="Destroy existing repo and recreate.")
@click.pass_context
def init(ctx, branch, force):
    """Create a new bare git repository."""
    repo_path = _require_repo(ctx)
    if force and os.path.exists(repo_path):
        import shutil
        shutil.rmtree(repo_path)
    elif os.path.exists(repo_path):
        raise click.ClickException(f"Repository already exists: {repo_path}")
    GitStore.open(repo_path, branch=branch)
    _status(ctx, f"Initialized {repo_path}")


# ---------------------------------------------------------------------------
# destroy
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.option("-f", "--force", is_flag=True, help="Required to destroy a non-empty repo.")
@click.pass_context
def destroy(ctx, force):
    """Remove a bare git repository.

    Requires -f if the repo contains any branches or tags.
    """
    repo_path = _require_repo(ctx)
    try:
        store = GitStore.open(repo_path, create=False)
    except FileNotFoundError:
        raise click.ClickException(f"Repository not found: {repo_path}")

    if not force:
        has_data = len(store.tags) > 0 or any(
            fs.ls() for fs in store.branches.values()
        )
        if has_data:
            raise click.ClickException(
                "Repository is not empty. Use -f to destroy."
            )

    import shutil
    shutil.rmtree(repo_path)
    _status(ctx, f"Destroyed {repo_path}")


# ---------------------------------------------------------------------------
# gc
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.pass_context
def gc(ctx):
    """Run garbage collection on the repository.

    Removes unreachable objects (orphaned blobs, etc.) and repacks
    the object store.  Requires git to be installed.
    """
    import shutil
    import subprocess

    repo_path = _require_repo(ctx)
    if not os.path.exists(repo_path):
        raise click.ClickException(f"Repository not found: {repo_path}")

    git = shutil.which("git")
    if git is None:
        raise click.ClickException(
            "git is not installed or not on PATH — gc requires git"
        )

    result = subprocess.run(
        [git, "gc"],
        cwd=repo_path,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        msg = result.stderr.strip() or result.stdout.strip() or "unknown error"
        raise click.ClickException(f"git gc failed: {msg}")

    _status(ctx, f"gc: {repo_path}")


# ---------------------------------------------------------------------------
# ls
# ---------------------------------------------------------------------------

def _read_link_target(object_store, oid) -> str:
    """Read symlink target from the blob."""
    return object_store[oid].data.decode()


def _ls_entry_dict(name, we, size, object_store):
    """Build a JSON-ready dict for a single ls -l entry."""
    if we is not None and we.file_type == FileType.LINK:
        target = _read_link_target(object_store, we.oid)
        return {"name": name, "hash": we.oid.decode(), "size": size, "type": "link", "target": target}
    if we is not None and we.file_type != FileType.TREE:
        return {"name": name, "hash": we.oid.decode(), "size": size, "type": str(we.file_type)}
    if we is not None:
        return {"name": name, "hash": we.oid.decode(), "type": "tree"}
    return {"name": name, "type": "tree"}


def _format_ls_output(results, long, fmt, object_store, *, full_hash=False):
    """Format and emit ls results.

    *results* is ``dict[str, WalkEntry | None]`` when *long* is True,
    else ``dict[str, None]``.
    """
    sorted_names = sorted(results)
    hash_len = 40 if full_hash else 7

    if fmt == "text" and not long:
        for name in sorted_names:
            click.echo(name)

    elif fmt == "text" and long:
        from .._objsize import ObjectSizer
        rows = []
        with ObjectSizer(object_store) as sizer:
            for name in sorted_names:
                we = results[name]
                if we is not None and we.file_type == FileType.LINK:
                    h = we.oid.decode()[:hash_len]
                    size = sizer.size(we.oid)
                    target = _read_link_target(object_store, we.oid)
                    rows.append((h, str(size), f"{name} -> {target}"))
                elif we is not None and we.file_type != FileType.TREE:
                    h = we.oid.decode()[:hash_len]
                    size = sizer.size(we.oid)
                    rows.append((h, str(size), name))
                else:
                    h = we.oid.decode()[:hash_len] if we is not None else ""
                    rows.append((h, "", name))
        width = max((len(s) for _, s, _ in rows if s), default=0)
        for hash_str, size_str, display in rows:
            click.echo(f"{hash_str}  {size_str:>{width}}  {display}")

    elif fmt == "json" and not long:
        click.echo(json.dumps(sorted_names))

    elif fmt == "json" and long:
        entries = []
        from .._objsize import ObjectSizer
        with ObjectSizer(object_store) as sizer:
            for name in sorted_names:
                we = results[name]
                if we is not None and we.file_type != FileType.TREE:
                    size = sizer.size(we.oid)
                else:
                    size = None
                entries.append(_ls_entry_dict(name, we, size, object_store))
        click.echo(json.dumps(entries))

    elif fmt == "jsonl" and not long:
        for name in sorted_names:
            click.echo(json.dumps(name))

    elif fmt == "jsonl" and long:
        from .._objsize import ObjectSizer
        with ObjectSizer(object_store) as sizer:
            for name in sorted_names:
                we = results[name]
                if we is not None and we.file_type != FileType.TREE:
                    size = sizer.size(we.oid)
                else:
                    size = None
                click.echo(json.dumps(
                    _ls_entry_dict(name, we, size, object_store)
                ))


@main.command()
@_repo_option
@click.argument("paths", nargs=-1)
@_branch_option
@click.option("-R", "--recursive", is_flag=True, help="List all files recursively with full paths.")
@click.option("-l", "--long", "long_", is_flag=True, help="Show file sizes, types, and hashes.")
@click.option("--full-hash", "full_hash", is_flag=True, default=False,
              help="Show full 40-character object hashes (default: 7-char short hash).")
@_format_option
@_no_glob_option
@_snapshot_options
@click.pass_context
def ls(ctx, paths, branch, recursive, long_, full_hash, fmt, no_glob, ref, at_path, match_pattern, before, back):
    """List files/directories at PATH(s) (or root).

    Accepts multiple paths and glob patterns.  Results are coalesced and
    deduplicated.  Quote glob patterns to prevent shell expansion.

    \b
    Examples:
        vost ls                         # root listing
        vost ls :src                    # subdirectory
        vost ls '*.txt' '*.py'          # multiple globs
        vost ls :src :docs              # multiple directories
        vost ls -R                      # all files recursively
        vost ls -R :src :docs           # recursive under multiple dirs
        vost ls -l                      # long listing with sizes and hashes
        vost ls --format json           # JSON output
    """
    store = _open_store(_require_repo(ctx))

    # Parse paths and check for conflicts with flags
    if paths:
        parsed = [_parse_ref_path(p) for p in paths]
        _check_ref_conflicts(parsed, ref=ref, branch=branch, back=back,
                             before=before, at_path=at_path, match_pattern=match_pattern)
    else:
        parsed = []

    branch = branch or _current_branch(store)
    default_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                             match_pattern=match_pattern, before=before, back=back)

    # No args → list root (single implicit path)
    if not paths:
        paths = (None,)

    # results: name → WalkEntry | None
    results: dict[str, WalkEntry | None] = {}

    for i, path in enumerate(paths):
        # Resolve per-path ref
        if path is not None:
            rp = parsed[i]
            if rp.is_repo and (rp.ref or rp.back):
                fs = _resolve_ref_path(store, rp, ref, branch,
                                       at_path=at_path, match_pattern=match_pattern,
                                       before=before, back=back)
            else:
                fs = default_fs
            repo_path = rp.path if rp.is_repo else path
        else:
            fs = default_fs
            repo_path = None

        has_glob = not no_glob and repo_path is not None and ("*" in repo_path or "?" in repo_path)

        if has_glob:
            pattern = repo_path
            matches = fs.iglob(pattern)
            if recursive:
                for m in matches:
                    if fs.is_dir(m):
                        for dp, _, fnames in fs.walk(m):
                            for fe in fnames:
                                name = f"{dp}/{fe.name}" if dp else fe.name
                                results.setdefault(name, fe if long_ else None)
                    else:
                        if long_ and m not in results:
                            entry = _entry_at_path(store._repo, fs._tree_oid, m)
                            if entry:
                                oid, fm = entry
                                results[m] = WalkEntry(m.rsplit("/", 1)[-1], oid, fm)
                            else:
                                results[m] = None
                        else:
                            results.setdefault(m, None)
            else:
                for m in matches:
                    if long_ and m not in results:
                        entry = _entry_at_path(store._repo, fs._tree_oid, m)
                        if entry:
                            oid, fm = entry
                            results[m] = WalkEntry(m.rsplit("/", 1)[-1], oid, fm)
                        else:
                            results[m] = None
                    else:
                        results.setdefault(m, None)

        elif recursive:
            rp_norm = None
            if repo_path:
                rp_norm = _normalize_repo_path(repo_path)
            try:
                for dp, _, fnames in fs.walk(rp_norm if rp_norm else None):
                    for fe in fnames:
                        name = f"{dp}/{fe.name}" if dp else fe.name
                        results.setdefault(name, fe if long_ else None)
            except FileNotFoundError:
                raise click.ClickException(f"Path not found: {rp_norm}")
            except NotADirectoryError:
                if long_ and rp_norm not in results:
                    entry = _entry_at_path(store._repo, fs._tree_oid, rp_norm)
                    if entry:
                        oid, fm = entry
                        results[rp_norm] = WalkEntry(rp_norm.rsplit("/", 1)[-1], oid, fm)
                    else:
                        results[rp_norm] = None
                else:
                    results.setdefault(rp_norm, None)

        else:
            rp_norm = None
            if repo_path:
                rp_norm = _normalize_repo_path(repo_path)
            try:
                if long_:
                    entries = list_entries_at_path(store._repo, fs._tree_oid, rp_norm)
                    for we in entries:
                        if we.file_type == FileType.TREE:
                            results.setdefault(we.name + "/", we)
                        else:
                            results.setdefault(we.name, we)
                else:
                    for name in fs.ls(rp_norm if rp_norm else None):
                        results.setdefault(name, None)
            except FileNotFoundError:
                raise click.ClickException(f"Path not found: {rp_norm}")
            except NotADirectoryError:
                if long_ and rp_norm not in results:
                    entry = _entry_at_path(store._repo, fs._tree_oid, rp_norm)
                    if entry:
                        oid, fm = entry
                        results[rp_norm] = WalkEntry(rp_norm.rsplit("/", 1)[-1], oid, fm)
                    else:
                        results[rp_norm] = None
                else:
                    results.setdefault(rp_norm, None)

    _format_ls_output(results, long_, fmt, store._repo.object_store, full_hash=full_hash)


# ---------------------------------------------------------------------------
# cat
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("paths", nargs=-1, required=True)
@_branch_option
@_snapshot_options
@click.pass_context
def cat(ctx, paths, branch, ref, at_path, match_pattern, before, back):
    """Concatenate file contents to stdout."""
    store = _open_store(_require_repo(ctx))

    # Parse paths and check for conflicts with flags
    parsed = [_parse_ref_path(p) for p in paths]
    _check_ref_conflicts(parsed, ref=ref, branch=branch, back=back,
                         before=before, at_path=at_path, match_pattern=match_pattern)

    branch = branch or _current_branch(store)
    default_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                             match_pattern=match_pattern, before=before, back=back)

    for i, path in enumerate(paths):
        rp = parsed[i]
        if rp.is_repo and (rp.ref or rp.back):
            fs = _resolve_ref_path(store, rp, ref, branch,
                                   at_path=at_path, match_pattern=match_pattern,
                                   before=before, back=back)
        else:
            fs = default_fs
        repo_path = _normalize_repo_path(rp.path if rp.is_repo else path)
        try:
            data = fs.read(repo_path)
        except FileNotFoundError:
            raise click.ClickException(f"File not found: {repo_path}")
        except IsADirectoryError:
            raise click.ClickException(f"{repo_path} is a directory, not a file")
        sys.stdout.buffer.write(data)


# ---------------------------------------------------------------------------
# hash
# ---------------------------------------------------------------------------

def _parse_bare_as_ref(raw: str) -> RefPath:
    """Parse a ref:path string, treating a bare string (no ``:``) as a ref."""
    rp = _parse_ref_path(raw)
    if rp.is_repo:
        return rp
    # No colon → treat as ref (branch/tag/hash), not a local path
    # Parse ~N suffix
    ref_part = raw
    back = 0
    tilde = ref_part.rfind("~")
    if tilde >= 0:
        suffix = ref_part[tilde + 1:]
        if suffix.isdigit() and int(suffix) > 0:
            back = int(suffix)
            ref_part = ref_part[:tilde]
        elif suffix.isdigit():
            raise click.ClickException(
                f"Invalid ancestor '~0' — use '{ref_part[:tilde]}:' instead"
            )
        else:
            raise click.ClickException(
                f"Invalid ancestor suffix '~{suffix}' — must be a positive integer"
            )
    return RefPath(ref=ref_part, back=back, path="")


@main.command("hash")
@_repo_option
@click.argument("target", required=False, default=None)
@_branch_option
@_snapshot_options
@click.pass_context
def hash_cmd(ctx, target, branch, ref, at_path, match_pattern, before, back):
    """Print the SHA hash of a commit, tree, or blob.

    \b
    TARGET is a ref, ref:path, or :path specification:
        vost hash                     →  current branch commit hash
        vost hash main                →  main branch commit hash
        vost hash v1.0                →  tag commit hash
        vost hash :config.json        →  blob hash on current branch
        vost hash main:src/           →  tree hash of directory
        vost hash ~3:                 →  commit hash 3 back
    """
    object_path = None
    if target is not None:
        rp = _parse_bare_as_ref(target)
        if rp.ref and ref:
            raise click.ClickException("Cannot specify both positional ref and --ref")
        if rp.ref and branch is not None:
            raise click.ClickException("Cannot use -b/--branch with explicit ref in target")
        if rp.back and back:
            raise click.ClickException("Cannot specify both positional ~N and --back")
        if rp.ref:
            ref = rp.ref
        if rp.back:
            back = rp.back
        if rp.path:
            object_path = _normalize_repo_path(rp.path)

    store = _open_store(_require_repo(ctx))
    branch = branch or _current_branch(store)
    fs = _resolve_fs(store, branch, ref, at_path=at_path,
                     match_pattern=match_pattern, before=before, back=back)

    if object_path is not None:
        try:
            st = fs.stat(object_path)
        except FileNotFoundError:
            raise click.ClickException(f"Path not found: {object_path}")
        click.echo(st.hash)
    else:
        click.echo(fs.commit_hash)


# ---------------------------------------------------------------------------
# rm
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("paths", nargs=-1, required=True)
@click.option("-R", "--recursive", is_flag=True, default=False,
              help="Remove directories recursively.")
@_dry_run_option
@_no_glob_option
@_branch_option
@_message_option
@_tag_option
@click.pass_context
def rm(ctx, paths, recursive, dry_run, no_glob, branch, message, tag, force_tag):
    """Remove files from the repo.

    Accepts multiple paths and glob patterns.  Quote glob patterns to
    prevent shell expansion.  Directories require -R.

    \b
    Examples:
        vost rm :file.txt
        vost rm ':*.txt'
        vost rm -R :dir
        vost rm -n :file.txt         # dry run
        vost rm :a.txt :b.txt        # multiple
    """
    store = _open_store(_require_repo(ctx))
    branch = branch or _current_branch(store)

    parsed = [_parse_ref_path(p) for p in paths]
    branch = _resolve_same_branch(store, parsed, branch, operation="remove")
    fs = _get_branch_fs(store, branch)

    patterns = [_normalize_repo_path(rp.path if rp.is_repo else p)
                for p, rp in zip(paths, parsed)]
    if not no_glob:
        patterns = _expand_sources_repo(fs, patterns)

    try:
        if dry_run:
            result_fs = fs.remove(patterns, recursive=recursive,
                                   dry_run=True)
            changes = result_fs.changes
            if changes:
                for action in changes.actions():
                    click.echo(f"- :{action.path}")
        else:
            new_fs = fs.remove(patterns, recursive=recursive,
                               message=message)
            if tag:
                _apply_tag(store, new_fs, tag, force_tag)
            changes = new_fs.changes
            n = len(changes.delete) if changes else 0
            _status(ctx, f"Removed {n} file(s)")
    except FileNotFoundError as exc:
        raise click.ClickException(str(exc))
    except IsADirectoryError as exc:
        raise click.ClickException(f"{exc} — use -R to remove recursively")
    except StaleSnapshotError:
        raise click.ClickException(
            "Branch modified concurrently — retry"
        )


# ---------------------------------------------------------------------------
# mv
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("args", nargs=-1, required=True)
@click.option("-R", "--recursive", is_flag=True, default=False,
              help="Move directories recursively.")
@_dry_run_option
@_no_glob_option
@_branch_option
@_message_option
@_tag_option
@click.pass_context
def mv(ctx, args, recursive, dry_run, no_glob, branch, message, tag, force_tag):
    """Move/rename files in the repo.

    All arguments are repo paths (colon prefix required). The last
    argument is the destination. Glob patterns and -R for directories.

    \b
    Examples:
        vost mv :old.txt :new.txt               # rename
        vost mv ':*.txt' :archive/               # move into dir
        vost mv -R :src :backup/src              # move directory
        vost mv :a.txt :b.txt :dest/             # multiple -> dir
        vost mv -n :old.txt :new.txt             # dry run
    """
    if len(args) < 2:
        raise click.ClickException("mv requires at least two arguments (SRC... DEST)")

    # Parse all args — all must be repo paths
    parsed = [_parse_ref_path(p) for p in args]
    for i, rp in enumerate(parsed):
        if not rp.is_repo:
            raise click.ClickException(
                f"All paths must be repo paths (colon prefix required): {args[i]}"
            )
        if rp.back:
            raise click.ClickException(
                "Cannot move to/from a historical commit (remove ~N)"
            )

    store = _open_store(_require_repo(ctx))
    branch = branch or _current_branch(store)
    branch = _resolve_same_branch(store, parsed, branch, operation="move")
    fs = _get_branch_fs(store, branch)

    source_patterns = [
        _normalize_repo_path(rp.path) if rp.path else ""
        for rp in parsed[:-1]
    ]
    if not no_glob:
        source_patterns = _expand_sources_repo(fs, source_patterns)
    dest_rp = parsed[-1]
    dest_path = dest_rp.path
    # Preserve trailing slash for directory semantics
    if dest_path:
        norm = _normalize_repo_path(dest_path.rstrip("/"))
        if dest_path.endswith("/"):
            dest_path = norm + "/" if norm else ""
        else:
            dest_path = norm
    else:
        dest_path = ""

    try:
        if dry_run:
            result_fs = fs.move(
                source_patterns, dest_path, recursive=recursive,
                dry_run=True,
            )
            changes = result_fs.changes
            if changes:
                for action in changes.actions():
                    prefix = {"add": "+", "delete": "-"}[action.action]
                    click.echo(f"{prefix} :{action.path}")
        else:
            new_fs = fs.move(
                source_patterns, dest_path,
                recursive=recursive, message=message,
            )
            if tag:
                _apply_tag(store, new_fs, tag, force_tag)
            changes = new_fs.changes
            n_add = len(changes.add) if changes else 0
            n_del = len(changes.delete) if changes else 0
            _status(ctx, f"Moved {n_del} -> {n_add} file(s)")
    except FileNotFoundError as exc:
        raise click.ClickException(str(exc))
    except IsADirectoryError as exc:
        raise click.ClickException(f"{exc} — use -R to move recursively")
    except ValueError as exc:
        raise click.ClickException(str(exc))
    except StaleSnapshotError:
        raise click.ClickException(
            "Branch modified concurrently — retry"
        )


# ---------------------------------------------------------------------------
# write
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("path")
@_branch_option
@_message_option
@_no_create_option
@_tag_option
@click.option("-p", "--passthrough", is_flag=True, default=False,
              help="Echo stdin to stdout (tee mode for pipelines).")
@click.option("--parent", "parent_refs", multiple=True,
              help="Additional parent ref (branch/tag/hash). Repeatable.")
@click.pass_context
def write(ctx, path, branch, message, no_create, tag, force_tag, passthrough, parent_refs):
    """Write stdin to a file in the repo."""
    from ..fs import retry_write

    # Parse ref:path — explicit ref overrides -b
    rp = _parse_ref_path(path)
    if rp.is_repo and rp.ref:
        branch = rp.ref
    if rp.is_repo and rp.back:
        raise click.ClickException("Cannot write to a historical commit (remove ~N)")

    # Stage 1: open store, resolve branch name (no FS fetch yet)
    repo_path = _require_repo(ctx)
    if no_create:
        store = _open_store(repo_path)
        branch = branch or _current_branch(store)
    else:
        store = _open_or_create_store(repo_path, branch=branch or "main")
        branch = branch or _current_branch(store)

    repo_path_norm = _normalize_repo_path(rp.path if rp.is_repo else _strip_colon(path))

    # Stage 2: read stdin (may take arbitrarily long — no stale FS held)
    if passthrough:
        buf = io.BytesIO()
        stdout = sys.stdout.buffer
        stdin = sys.stdin.buffer
        _read = getattr(stdin, 'read1', stdin.read)
        while True:
            chunk = _read(8192)
            if not chunk:
                break
            stdout.write(chunk)
            stdout.flush()
            buf.write(chunk)
        data = buf.getvalue()
    else:
        data = sys.stdin.buffer.read()

    # Resolve advisory parent refs
    parents = [_get_fs(store, None, r) for r in parent_refs] if parent_refs else None

    # Stage 3: commit (fetches fresh FS internally, retries on stale)
    try:
        new_fs = retry_write(store, branch, repo_path_norm, data, message=message, parents=parents)
    except StaleSnapshotError:
        raise click.ClickException(
            "Branch modified concurrently — retry"
        )
    except KeyError:
        raise click.ClickException(f"Branch not found: {branch}")
    if tag:
        _apply_tag(store, new_fs, tag, force_tag)
    _status(ctx, f"Wrote :{repo_path_norm}")


# ---------------------------------------------------------------------------
# log
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("target", required=False, default=None)
@click.option("--at", "deprecated_at", default=None, hidden=True)
@_branch_option
@_snapshot_options
@_format_option
@click.pass_context
def log(ctx, target, at_path, deprecated_at, match_pattern, before, branch, ref, back, fmt):
    """Show commit log, optionally filtered by path and/or message pattern.

    \b
    An optional TARGET argument supports ref and ref:path syntax:
        vost log main                →  --ref main
        vost log main:config.json    →  --ref main --path config.json
        vost log main~3:             →  --ref main --back 3
        vost log ~3:config.json      →  --back 3 --path config.json
    """
    at_path = at_path or deprecated_at

    # Parse optional positional target
    if target is not None:
        rp = _parse_bare_as_ref(target)
        if rp.ref and ref:
            raise click.ClickException("Cannot specify both positional ref and --ref")
        if rp.ref and branch is not None:
            raise click.ClickException("Cannot use -b/--branch with explicit ref in target")
        if rp.back and back:
            raise click.ClickException("Cannot specify both positional ~N and --back")
        if rp.path and at_path:
            raise click.ClickException("Cannot specify both positional path and --path")
        if rp.ref:
            ref = rp.ref
        if rp.back:
            back = rp.back
        if rp.path:
            at_path = rp.path

    store = _open_store(_require_repo(ctx))
    branch = branch or _current_branch(store)
    fs = _get_fs(store, branch, ref)
    if back:
        try:
            fs = fs.back(back)
        except ValueError as e:
            raise click.ClickException(str(e))

    before = _parse_before(before)
    at_path = _normalize_at_path(at_path)
    entries = list(fs.log(path=at_path, match=match_pattern, before=before))

    if fmt == "json":
        click.echo(json.dumps([_log_entry_dict(e) for e in entries], indent=2))
    elif fmt == "jsonl":
        for entry in entries:
            click.echo(json.dumps(_log_entry_dict(entry)))
    else:
        for entry in entries:
            click.echo(f"{entry.commit_hash[:7]}  {entry.time.isoformat()}  {entry.message}")


# ---------------------------------------------------------------------------
# diff
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.argument("baseline", required=False, default=None)
@_branch_option
@_snapshot_options
@click.option("--reverse", is_flag=True, help="Swap comparison direction.")
@click.pass_context
def diff(ctx, baseline, branch, ref, at_path, match_pattern, before, back, reverse):
    """Show files that differ between HEAD and another snapshot.

    \b
    An optional BASELINE argument supports ref and ref:path syntax:
        vost diff dev                →  --ref dev
        vost diff ~3:                →  --back 3
        vost diff dev:               →  --ref dev
        vost diff main~2:            →  --ref main --back 2
    """
    # Parse optional positional baseline
    if baseline is not None:
        rp = _parse_bare_as_ref(baseline)
        if rp.ref and ref:
            raise click.ClickException("Cannot specify both positional ref and --ref")
        if rp.ref and branch is not None:
            raise click.ClickException("Cannot use -b/--branch with explicit ref in baseline")
        if rp.back and back:
            raise click.ClickException("Cannot specify both positional ~N and --back")
        if rp.path and at_path:
            raise click.ClickException("Cannot specify both positional path and --path")
        if rp.ref:
            ref = rp.ref
        if rp.back:
            back = rp.back
        if rp.path:
            at_path = rp.path

    store = _open_store(_require_repo(ctx))
    branch = branch or _current_branch(store)
    head_fs = _get_fs(store, branch, None)
    other_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                           match_pattern=match_pattern, before=before, back=back)
    if head_fs.commit_hash == other_fs.commit_hash:
        return
    new_files = _walk_repo(head_fs, "")
    old_files = _walk_repo(other_fs, "")
    if reverse:
        new_files, old_files = old_files, new_files
    for p in sorted(set(new_files) - set(old_files)):
        click.echo(f"A  {p}")
    for p in sorted(set(new_files) & set(old_files)):
        if new_files[p] != old_files[p]:
            click.echo(f"M  {p}")
    for p in sorted(set(old_files) - set(new_files)):
        click.echo(f"D  {p}")


# ---------------------------------------------------------------------------
# cmp
# ---------------------------------------------------------------------------

def _get_blob_hash(store, rp, fs):
    """Return 40-char hex blob hash for a RefPath target."""
    if rp.is_repo:
        path = _normalize_repo_path(rp.path)
        try:
            return fs.object_hash(path)
        except FileNotFoundError:
            raise click.ClickException(f"File not found: {path}")
        except IsADirectoryError:
            raise click.ClickException(f"Is a directory: {path}")
    else:
        local = _Path(rp.path)
        if not local.exists():
            raise click.ClickException(f"File not found: {rp.path}")
        if local.is_dir():
            raise click.ClickException(f"Is a directory: {rp.path}")
        from ..copy._io import _local_file_oid_abs
        return _local_file_oid_abs(local).decode("ascii")


@main.command()
@_repo_option
@click.argument("file1", required=True)
@click.argument("file2", required=True)
@_branch_option
@_snapshot_options
@click.pass_context
def cmp(ctx, file1, file2, branch, ref, at_path, match_pattern, before, back):
    """Compare two files by content hash.

    Compares the git blob SHA of two files. Files can be repo paths
    (with : prefix or ref:path syntax), local disk paths, or a mix.

    \b
    Exit codes:
        0  files are identical
        1  files differ

    \b
    Examples:
        vost cmp :file1.txt :file2.txt           # two repo files
        vost cmp main:f.txt dev:f.txt             # cross-branch
        vost cmp main~3:f.txt main:f.txt          # ancestor
        vost cmp :data.bin /tmp/data.bin           # repo vs disk
        vost cmp /tmp/a.txt /tmp/b.txt             # two disk files
    """
    rp1 = _parse_ref_path(file1)
    rp2 = _parse_ref_path(file2)

    # Collect repo-typed args and check for conflicts with flags
    repo_rps = [rp for rp in (rp1, rp2) if rp.is_repo]
    if repo_rps:
        _check_ref_conflicts(repo_rps, ref=ref, branch=branch, back=back,
                             before=before, at_path=at_path, match_pattern=match_pattern)

    # Only open store if at least one arg is a repo path
    store = None
    default_fs = None
    if repo_rps:
        store = _open_store(_require_repo(ctx))
        branch = branch or _current_branch(store)
        default_fs = _resolve_fs(store, branch, ref, at_path=at_path,
                                 match_pattern=match_pattern, before=before, back=back)

    # Resolve FS for each arg
    def _resolve_arg_fs(rp):
        if not rp.is_repo:
            return None  # local file, no FS needed
        if rp.ref or rp.back:
            return _resolve_ref_path(store, rp, ref, branch,
                                     at_path=at_path, match_pattern=match_pattern,
                                     before=before, back=back)
        return default_fs

    fs1 = _resolve_arg_fs(rp1)
    fs2 = _resolve_arg_fs(rp2)

    hash1 = _get_blob_hash(store, rp1, fs1)
    hash2 = _get_blob_hash(store, rp2, fs2)

    if ctx.obj.get("verbose"):
        click.echo(f"{hash1}  {file1}", err=True)
        click.echo(f"{hash2}  {file2}", err=True)

    ctx.exit(0 if hash1 == hash2 else 1)


# ---------------------------------------------------------------------------
# undo
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@_branch_option
@click.argument("steps", type=int, default=1, required=False)
@click.pass_context
def undo(ctx, branch, steps):
    """Move branch back N commits (default 1).

    Walks back through parent commits and updates the branch pointer.
    Creates a reflog entry so you can redo later.

    Examples:
        vost --repo data.git undo       # Back 1 commit
        vost --repo data.git undo 3     # Back 3 commits
        vost --repo data.git undo -b dev 2  # Undo 2 on 'dev' branch
    """
    repo_path = _require_repo(ctx)
    repo = _open_store(repo_path)

    try:
        branch = branch or _current_branch(repo)
        fs = repo.branches[branch]

        # Perform undo
        new_fs = fs.undo(steps)

        # Show what happened
        step_word = "step" if steps == 1 else "steps"
        _status(ctx, f"Undid {steps} {step_word} on '{branch}'")
        click.echo(f"Branch now at: {new_fs.commit_hash[:7]} - {new_fs.message}")

    except KeyError:
        raise click.ClickException(f"Branch not found: {branch}")
    except ValueError as e:
        raise click.ClickException(str(e))
    except PermissionError as e:
        raise click.ClickException(str(e))


# ---------------------------------------------------------------------------
# redo
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@_branch_option
@click.argument("steps", type=int, default=1, required=False)
@click.pass_context
def redo(ctx, branch, steps):
    """Move branch forward N steps in reflog (default 1).

    Uses the reflog to find where the branch was in the future and moves
    there. Can resurrect commits after undo or divergence.

    Examples:
        vost --repo data.git redo       # Forward 1 step
        vost --repo data.git redo 2     # Forward 2 steps
        vost --repo data.git redo -b dev  # Redo on 'dev' branch
    """
    repo_path = _require_repo(ctx)
    repo = _open_store(repo_path)

    try:
        branch = branch or _current_branch(repo)
        fs = repo.branches[branch]

        # Perform redo
        new_fs = fs.redo(steps)

        # Show what happened
        step_word = "step" if steps == 1 else "steps"
        _status(ctx, f"Redid {steps} {step_word} on '{branch}'")
        click.echo(f"Branch now at: {new_fs.commit_hash[:7]} - {new_fs.message}")

    except KeyError:
        raise click.ClickException(f"Branch not found: {branch}")
    except ValueError as e:
        raise click.ClickException(str(e))
    except PermissionError as e:
        raise click.ClickException(str(e))
    except FileNotFoundError as e:
        raise click.ClickException(str(e))


# ---------------------------------------------------------------------------
# reflog
# ---------------------------------------------------------------------------

def _reflog_entry_dict(entry) -> dict:
    from datetime import datetime as _dt
    return {
        "old_sha": entry.old_sha,
        "new_sha": entry.new_sha,
        "committer": entry.committer,
        "timestamp": entry.timestamp,
        "time": _dt.fromtimestamp(entry.timestamp).isoformat(),
        "message": entry.message,
    }


@main.command()
@_repo_option
@_branch_option
@click.option("-n", "--limit", type=int, help="Limit number of entries shown.")
@_format_option
@click.pass_context
def reflog(ctx, branch, limit, fmt):
    """Show reflog entries for a branch.

    The reflog shows chronological history of where the branch pointer
    has been, including undos and branch updates. This is different from
    'log' which shows the commit tree.

    Examples:
        vost --repo data.git reflog              # Show all entries (text)
        vost --repo data.git reflog -n 10        # Show last 10
        vost --repo data.git reflog -b dev       # Show for 'dev' branch
        vost --repo data.git reflog --format json   # JSON output
        vost --repo data.git reflog --format jsonl  # JSON Lines output
    """
    repo_path = _require_repo(ctx)
    repo = _open_store(repo_path)

    try:
        branch = branch or _current_branch(repo)
        entries = repo.branches.reflog(branch)

        # Apply limit if specified
        if limit:
            entries = entries[-limit:]

        # Handle empty reflog
        if not entries:
            if fmt == "json":
                click.echo("[]")
            elif fmt == "jsonl":
                pass  # No output for empty
            else:
                click.echo(f"No reflog entries for branch '{branch}'")
            return

        # Output in requested format
        if fmt == "json":
            click.echo(json.dumps([_reflog_entry_dict(e) for e in entries], indent=2))
        elif fmt == "jsonl":
            for entry in entries:
                click.echo(json.dumps(_reflog_entry_dict(entry)))
        else:
            # Text format (default)
            from datetime import datetime as _dt
            click.echo(f"Reflog for branch '{branch}' ({len(entries)} entries):\n")

            for i, entry in enumerate(entries):
                new = entry.new_sha[:7]
                msg = entry.message

                # Format timestamp
                ts = _dt.fromtimestamp(entry.timestamp)
                time_str = ts.strftime("%Y-%m-%d %H:%M:%S")

                click.echo(f"  [{i}] {new} ({time_str})")
                click.echo(f"      {msg}")
                click.echo()

    except KeyError:
        raise click.ClickException(f"Branch not found: {branch}")
    except FileNotFoundError as e:
        raise click.ClickException(str(e))
