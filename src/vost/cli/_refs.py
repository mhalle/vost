"""Branch and tag subcommands."""

from __future__ import annotations

import click

from ._helpers import (
    branch,
    tag,
    _repo_option,
    _branch_option,
    _require_repo,
    _status,
    _open_store,
    _current_branch,
    _apply_snapshot_filters,
    _resolve_fs,
    _resolve_ref,
    _snapshot_options,
)
import vost.cli._helpers as _helpers


# ---------------------------------------------------------------------------
# branch subcommands
# ---------------------------------------------------------------------------

@branch.command("list")
@_repo_option
@click.pass_context
def branch_list(ctx):
    """List all branches."""
    store = _open_store(_require_repo(ctx))
    for name in sorted(store.branches):
        click.echo(name)


@branch.command("set")
@_repo_option
@click.argument("name")
@_branch_option
@click.option("-f", "--force", is_flag=True, default=False,
              help="Overwrite if branch already exists.")
@click.option("--empty", is_flag=True, default=False,
              help="Create an empty root branch (no parent commit).")
@click.option("--squash", is_flag=True, default=False,
              help="Squash to a single commit (no history).")
@click.option("--append", is_flag=True, default=False,
              help="Append source tree as a new commit on the branch tip. "
                   "Combine with --squash to collapse source history.")
@_snapshot_options
@click.pass_context
def branch_set(ctx, name, branch, force, empty, squash, append, ref, at_path, match_pattern, before, back):
    """Create or update branch NAME.

    By default forks from the current branch. Use --empty for a new root branch.
    Use --force to overwrite an existing branch.
    """
    store = _open_store(_require_repo(ctx))

    if append:
        if empty:
            raise click.ClickException("--append cannot be combined with --empty")
        if name not in store.branches:
            raise click.ClickException(f"--append requires existing branch: {name}")
        branch = branch or _current_branch(store)
        source_fs = _resolve_fs(store, branch, ref=ref, at_path=at_path,
                                match_pattern=match_pattern, before=before, back=back)
        tip = store.branches[name]
        appended = source_fs.squash(parent=tip)
        from ..fs import FS
        new_fs = FS(store, appended._commit_oid, ref_name=name)
        try:
            store.branches[name] = new_fs
        except ValueError as e:
            raise click.ClickException(str(e))
    elif name in store.branches and not force:
        raise click.ClickException(f"Branch already exists: {name}")
    elif empty:
        if ref or at_path or match_pattern or before or back:
            raise click.ClickException(
                "--empty cannot be combined with --ref/--path/--match/--before/--back")
        repo = store._repo
        sig = store._signature
        tree_oid = repo.TreeBuilder().write()
        repo.create_commit(
            f"refs/heads/{name}", sig, sig,
            f"Initialize {name}", tree_oid, [],
        )
    else:
        branch = branch or _current_branch(store)
        source_fs = _resolve_fs(store, branch, ref=ref, at_path=at_path,
                                match_pattern=match_pattern, before=before, back=back)
        if squash:
            squashed = source_fs.squash()
            from ..fs import FS
            new_fs = FS(store, squashed._commit_oid, ref_name=name)
        else:
            from ..fs import FS
            new_fs = FS(store, source_fs._commit_oid, ref_name=name)
        try:
            store.branches[name] = new_fs
        except ValueError as e:
            raise click.ClickException(str(e))

    _status(ctx, f"Set branch {name}")


@branch.command("exists")
@_repo_option
@click.argument("name")
@click.pass_context
def branch_exists(ctx, name):
    """Check if branch NAME exists (exit 0 if yes, exit 1 if no)."""
    store = _open_store(_require_repo(ctx))
    if name not in store.branches:
        raise SystemExit(1)


@branch.command("delete")
@_repo_option
@click.argument("name")
@click.pass_context
def branch_delete(ctx, name):
    """Delete branch NAME."""
    store = _open_store(_require_repo(ctx))
    try:
        del store.branches[name]
    except KeyError:
        raise click.ClickException(f"Branch not found: {name}")
    _status(ctx, f"Deleted branch {name}")


@branch.command("hash")
@_repo_option
@click.argument("name")
@click.option("--back", type=int, default=0, help="Walk back N commits.")
@click.option("--before", "before", default=None,
              help="Use latest commit on or before this date (ISO 8601).")
@click.option("--match", "match_pattern", default=None,
              help="Use latest commit matching this message pattern (* and ?).")
@click.option("--path", "at_path", default=None,
              help="Use latest commit that changed this path.")
@click.pass_context
def branch_hash(ctx, name, at_path, match_pattern, before, back):
    """Print the commit hash of branch NAME."""
    store = _open_store(_require_repo(ctx))
    try:
        fs = store.branches[name]
    except KeyError:
        raise click.ClickException(f"Branch not found: {name}")
    fs = _apply_snapshot_filters(fs, at_path=at_path, match_pattern=match_pattern,
                                 before=before, back=back)
    click.echo(fs.commit_hash)


# ---------------------------------------------------------------------------
# tag subcommands
# ---------------------------------------------------------------------------

@tag.command("list")
@_repo_option
@click.pass_context
def tag_list(ctx):
    """List all tags."""
    store = _open_store(_require_repo(ctx))
    for name in sorted(store.tags):
        click.echo(name)


@tag.command("set")
@_repo_option
@click.argument("name")
@_branch_option
@click.option("-f", "--force", is_flag=True, default=False,
              help="Overwrite if tag already exists.")
@_snapshot_options
@click.pass_context
def tag_set(ctx, name, branch, force, ref, at_path, match_pattern, before, back):
    """Create or update tag NAME from an existing ref.

    Use --force to overwrite an existing tag.
    """
    store = _open_store(_require_repo(ctx))

    if name in store.tags and not force:
        raise click.ClickException(f"Tag already exists: {name}")

    branch = branch or _current_branch(store)
    source_fs = _resolve_fs(store, branch, ref=ref, at_path=at_path,
                            match_pattern=match_pattern, before=before, back=back)
    from ..fs import FS
    new_fs = FS(store, source_fs._commit_oid, writable=False)
    if name in store.tags:
        del store.tags[name]
    store.tags[name] = new_fs
    _status(ctx, f"Set tag {name}")


@tag.command("exists")
@_repo_option
@click.argument("name")
@click.pass_context
def tag_exists(ctx, name):
    """Check if tag NAME exists (exit 0 if yes, exit 1 if no)."""
    store = _open_store(_require_repo(ctx))
    if name not in store.tags:
        raise SystemExit(1)


@tag.command("delete")
@_repo_option
@click.argument("name")
@click.pass_context
def tag_delete(ctx, name):
    """Delete tag NAME."""
    store = _open_store(_require_repo(ctx))
    try:
        del store.tags[name]
    except KeyError:
        raise click.ClickException(f"Tag not found: {name}")
    _status(ctx, f"Deleted tag {name}")


@tag.command("hash")
@_repo_option
@click.argument("name")
@click.pass_context
def tag_hash(ctx, name):
    """Print the commit hash of tag NAME."""
    store = _open_store(_require_repo(ctx))
    try:
        fs = store.tags[name]
    except KeyError:
        raise click.ClickException(f"Tag not found: {name}")
    click.echo(fs.commit_hash)


@branch.command("current")
@_repo_option
@click.option("--branch", "-b", default=None,
              help="Set the current branch to this name.")
@click.pass_context
def branch_current(ctx, branch):
    """Show or set the repository's current branch.

    Without -b, prints the current branch (HEAD).
    With -b NAME, sets the current branch to NAME (must exist).
    """
    store = _open_store(_require_repo(ctx))
    if branch is None:
        name = store.branches.current_name
        if name is None:
            raise click.ClickException("HEAD does not point to an existing branch")
        click.echo(name)
    else:
        try:
            store.branches.current = branch
        except KeyError:
            raise click.ClickException(f"Branch not found: {branch}")
        _status(ctx, f"Current branch set to {branch}")


# Wire up the default subcommand references in _helpers
_helpers.branch_list = branch_list
_helpers.tag_list = tag_list
