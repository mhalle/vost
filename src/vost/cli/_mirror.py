"""Backup and restore commands."""

from __future__ import annotations

import click

from ._helpers import (
    main,
    _repo_option,
    _dry_run_option,
    _no_create_option,
    _require_repo,
    _status,
    _open_store,
    _open_or_create_bare,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _print_diff(diff, direction: str) -> None:
    """Pretty-print a MirrorDiff to stdout."""
    verb = "push" if direction == "push" else "pull"
    if diff.in_sync:
        click.echo(f"Nothing to {verb} — already in sync.")
        return
    for c in sorted(diff.add, key=lambda c: c.ref):
        click.echo(f"  create  {c.ref}  {c.new_target[:7]}")
    for c in sorted(diff.update, key=lambda c: c.ref):
        click.echo(f"  update  {c.ref}  {c.old_target[:7]} -> {c.new_target[:7]}")
    for c in sorted(diff.delete, key=lambda c: c.ref):
        click.echo(f"  delete  {c.ref}  {c.old_target[:7]}")
    click.echo(f"{diff.total} ref(s) would be changed.")


def _parse_refs(ref_tuple):
    """Parse --ref values into a list or dict.

    If any value contains ``:``, all values are parsed as ``src:dst``
    and a dict is returned.  Otherwise a plain list is returned.
    ``None`` is returned when the tuple is empty.
    """
    if not ref_tuple:
        return None
    has_rename = any(":" in r for r in ref_tuple)
    if not has_rename:
        return list(ref_tuple)
    result = {}
    for r in ref_tuple:
        if ":" in r:
            src, _, dst = r.partition(":")
            if not src or not dst:
                raise click.BadParameter(
                    f"Invalid ref rename {r!r} — expected src:dst",
                    param_hint="'--ref'",
                )
            result[src] = dst
        else:
            result[r] = r
    return result


def _progress_cb(ctx):
    """Return a progress callback if verbose mode is on, else None."""
    if not ctx.obj.get("verbose"):
        return None
    def _on_progress(msg):
        text = msg.decode()
        text = text.replace("\r", "\r\033[K")
        click.echo(text, nl=False, err=True)
    return _on_progress


# ---------------------------------------------------------------------------
# backup
# ---------------------------------------------------------------------------

@main.command("backup")
@_repo_option
@click.argument("url")
@_dry_run_option
@click.option("--ref", multiple=True, help="Ref to include (repeatable). Use src:dst to rename. Omit for all refs.")
@click.option("--format", "fmt", type=click.Choice(["bundle"]), default=None,
              help="Force output format (auto-detected from .bundle extension).")
@click.option("--squash", is_flag=True, default=False,
              help="Strip history: each ref becomes a single parentless commit (bundle only).")
@click.pass_context
def backup_cmd(ctx, url, dry_run, ref, fmt, squash):
    """Push refs to a remote URL or write a bundle file.

    Without --ref this is a full mirror: remote-only refs are deleted.
    With --ref only the specified refs are pushed (no deletes).
    Use --ref src:dst to rename refs on the remote side.

    If URL ends with .bundle, a portable bundle file is written.
    Use --squash to strip history from bundle output.
    """
    from ..mirror import _is_bundle_path, resolve_credentials

    store = _open_store(_require_repo(ctx))
    refs = _parse_refs(ref)
    use_bundle = (fmt == "bundle") or _is_bundle_path(url)
    auth_url = url if use_bundle else resolve_credentials(url)
    diff = store.backup(auth_url, dry_run=dry_run, progress=_progress_cb(ctx),
                        refs=refs, format=fmt, squash=squash)
    if dry_run:
        _print_diff(diff, "push")
    else:
        _status(ctx, f"Backed up to {url}")


# ---------------------------------------------------------------------------
# restore
# ---------------------------------------------------------------------------

@main.command("restore")
@_repo_option
@click.argument("url")
@_dry_run_option
@_no_create_option
@click.option("--ref", multiple=True, help="Ref to include (repeatable). Use src:dst to rename. Omit for all refs.")
@click.option("--format", "fmt", type=click.Choice(["bundle"]), default=None,
              help="Force input format (auto-detected from .bundle extension).")
@click.pass_context
def restore_cmd(ctx, url, dry_run, no_create, ref, fmt):
    """Fetch refs from a remote URL or import a bundle file.

    Restore is additive: refs are added and updated but local-only
    refs are never deleted. Use --ref src:dst to rename refs locally.
    HEAD (the current branch) is not restored;
    use 'vost branch current -b NAME' afterwards if needed.
    """
    from ..mirror import _is_bundle_path, resolve_credentials

    repo_path = _require_repo(ctx)
    store = _open_store(repo_path) if no_create else _open_or_create_bare(repo_path)
    refs = _parse_refs(ref)
    use_bundle = (fmt == "bundle") or _is_bundle_path(url)
    auth_url = url if use_bundle else resolve_credentials(url)
    diff = store.restore(auth_url, dry_run=dry_run, progress=_progress_cb(ctx),
                         refs=refs, format=fmt)
    if dry_run:
        _print_diff(diff, "pull")
    else:
        _status(ctx, f"Restored from {url}")
