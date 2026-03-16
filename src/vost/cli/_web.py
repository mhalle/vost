"""serve — HTTP file server for repo contents."""

from __future__ import annotations

import html
import json
import mimetypes
from urllib.parse import quote

import click

from ._helpers import (
    main,
    _repo_option,
    _branch_option,
    _snapshot_options,
    _require_repo,
    _open_store,
    _current_branch,
    _resolve_fs,
)


# Register extensions that Python's mimetypes module doesn't know.
mimetypes.add_type("application/geo+json", ".geojson")

# MIME overrides for types that browsers download instead of displaying.
_MIME_OVERRIDES = {
    "application/json": "text/plain; charset=utf-8",
    "application/geo+json": "text/plain; charset=utf-8",
    "application/xml": "text/xml; charset=utf-8",
    "application/yaml": "text/plain; charset=utf-8",
    "application/x-yaml": "text/plain; charset=utf-8",
}


def _guess_mime(path):
    """Return a browser-friendly MIME type for *path*."""
    mime, _ = mimetypes.guess_type(path)
    if mime is None:
        return "application/octet-stream"
    return _MIME_OVERRIDES.get(mime, mime)


def _href(*segments: str) -> str:
    """Build an HTML-safe href from path segments.

    Each segment is percent-encoded (preserving ``/`` within segments),
    then the whole value is HTML-attribute-escaped.
    """
    parts = [quote(s, safe="/") for s in segments if s]
    raw = "/".join(parts)
    if not raw.startswith("/"):
        raw = "/" + raw
    return html.escape(raw, quote=True)


# ---------------------------------------------------------------------------
# WSGI middlewares
# ---------------------------------------------------------------------------

def _cors_middleware(app):
    """WSGI middleware that adds permissive CORS headers."""
    _CORS_HEADERS = [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS"),
        ("Access-Control-Allow-Headers", "Accept, If-None-Match"),
        ("Access-Control-Expose-Headers", "ETag, Content-Length"),
    ]

    def wrapped(environ, start_response):
        if environ.get("REQUEST_METHOD") == "OPTIONS":
            start_response("204 No Content", _CORS_HEADERS)
            return [b""]

        def cors_start_response(status, headers):
            return start_response(status, headers + _CORS_HEADERS)

        return app(environ, cors_start_response)

    return wrapped


def _no_cache_middleware(app):
    """WSGI middleware that adds Cache-Control: no-store."""

    def wrapped(environ, start_response):
        def nocache_start_response(status, headers):
            return start_response(status, headers + [("Cache-Control", "no-store")])

        return app(environ, nocache_start_response)

    return wrapped


def _base_path_middleware(app, prefix):
    """WSGI middleware that strips *prefix* from PATH_INFO."""

    def wrapped(environ, start_response):
        path = environ.get("PATH_INFO", "/")
        if not path.startswith(prefix):
            body = b"Not found"
            start_response("404 Not Found", [
                ("Content-Type", "text/plain"),
                ("Content-Length", str(len(body))),
            ])
            return [body]
        environ = dict(environ, PATH_INFO=path[len(prefix):] or "/")
        return app(environ, start_response)

    return wrapped


# ---------------------------------------------------------------------------
# WSGI app
# ---------------------------------------------------------------------------

_DEFAULT_MAX_FILE_SIZE = 250 * 1024 * 1024  # 250 MB


def _make_app(store, *, fs=None, resolver=None, ref_label=None,
              cors=False, no_cache=False, base_path="",
              max_file_size=_DEFAULT_MAX_FILE_SIZE):
    """Return a WSGI application serving *store* contents over HTTP.

    Single-ref mode (one snapshot per request):
      *resolver* — callable returning an FS, called on every request (live).
      *fs* — fixed snapshot (convenience shorthand for tests).

    If neither is given, multi-ref mode: first URL segment selects the
    branch or tag.

    *ref_label* is the display name shown in JSON responses for single-ref
    mode (e.g. the branch name).
    """
    single_ref = fs is not None or resolver is not None

    def _get_fs():
        if resolver is not None:
            return resolver()
        return fs

    def app(environ, start_response):
        path_info = environ.get("PATH_INFO", "/")
        path = path_info.strip("/")
        accept = environ.get("HTTP_ACCEPT", "")
        want_json = "application/json" in accept

        if single_ref:
            # --- Single-ref mode ---
            current_fs = _get_fs()
            return _serve_path(environ, start_response, current_fs,
                               ref_label or "", base_path, path, want_json,
                               max_file_size)
        else:
            # --- Multi-ref mode ---
            if not path:
                return _serve_ref_listing(start_response, store, base_path,
                                          want_json)

            # First segment is the ref
            parts = path.split("/", 1)
            ref_name = parts[0]
            rest = parts[1] if len(parts) > 1 else ""

            # Resolve ref: branches first, then tags
            if ref_name not in store.branches and ref_name not in store.tags:
                return _send_404(start_response, f"Unknown ref: {ref_name}")

            resolved = _resolve_fs_for_ref(store, ref_name)
            return _serve_path(environ, start_response, resolved, ref_name,
                               f"{base_path}/{ref_name}", rest, want_json,
                               max_file_size)

    result = app
    if base_path:
        result = _base_path_middleware(result, base_path)
    if cors:
        result = _cors_middleware(result)
    if no_cache:
        result = _no_cache_middleware(result)
    return result


def _resolve_fs_for_ref(store, ref_name):
    """Resolve a ref name to an FS, trying branches then tags."""
    if ref_name in store.branches:
        return store.branches[ref_name]
    return store.tags[ref_name]


def _serve_ref_listing(start_response, store, base_path, want_json):
    """Serve the list of branches and tags."""
    branches = sorted(store.branches)
    tags = sorted(store.tags)

    if want_json:
        body = json.dumps({"branches": branches, "tags": tags}).encode()
        start_response("200 OK", [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
        ])
        return [body]

    # HTML listing
    lines = ["<html><body>", "<h1>Branches</h1>", "<ul>"]
    for b in branches:
        lines.append(f'<li><a href="{_href(base_path, b)}/">{html.escape(b)}</a></li>')
    lines.append("</ul>")
    lines.append("<h1>Tags</h1>")
    lines.append("<ul>")
    for t in tags:
        lines.append(f'<li><a href="{_href(base_path, t)}/">{html.escape(t)}</a></li>')
    lines.append("</ul>")
    lines.append("</body></html>")
    body = "\n".join(lines).encode()
    start_response("200 OK", [
        ("Content-Type", "text/html; charset=utf-8"),
        ("Content-Length", str(len(body))),
    ])
    return [body]


def _serve_path(environ, start_response, fs, ref_label, link_prefix, path, want_json,
                max_file_size=_DEFAULT_MAX_FILE_SIZE):
    """Serve a file or directory listing within a resolved FS."""
    etag = f'"{fs.commit_hash}"'

    # 304 Not Modified if client ETag matches
    if_none_match = environ.get("HTTP_IF_NONE_MATCH")
    if if_none_match and if_none_match == etag:
        start_response("304 Not Modified", [("ETag", etag)])
        return [b""]

    if not path:
        return _serve_dir(start_response, fs, ref_label, link_prefix, "", want_json, etag)

    if not fs.exists(path):
        return _send_404(start_response, f"Not found: {path}")

    if fs.is_dir(path):
        return _serve_dir(start_response, fs, ref_label, link_prefix, path, want_json, etag)

    return _serve_file(start_response, fs, ref_label, path, want_json, etag,
                       max_file_size)


def _send_413(start_response, path, size, limit):
    """Send a 413 Payload Too Large response."""
    msg = f"File too large: {path} ({size} bytes, limit {limit} bytes)"
    body = msg.encode()
    start_response("413 Payload Too Large", [
        ("Content-Type", "text/plain"),
        ("Content-Length", str(len(body))),
    ])
    return [body]


def _serve_file(start_response, fs, ref_label, path, want_json, etag,
                max_file_size=_DEFAULT_MAX_FILE_SIZE):
    """Serve file contents or JSON metadata."""
    # Check size before reading
    if max_file_size > 0:
        try:
            file_size = fs.size(path)
            if file_size > max_file_size:
                return _send_413(start_response, path, file_size, max_file_size)
        except Exception:
            pass  # fall through to read

    data = fs.read(path)

    if want_json:
        body = json.dumps({
            "path": path,
            "ref": ref_label,
            "size": len(data),
            "type": "file",
        }).encode()
        start_response("200 OK", [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
            ("ETag", etag),
            ("Cache-Control", "no-cache"),
        ])
        return [body]

    mime = _guess_mime(path)

    start_response("200 OK", [
        ("Content-Type", mime),
        ("Content-Length", str(len(data))),
        ("ETag", etag),
        ("Cache-Control", "no-cache"),
    ])
    return [data]


def _serve_dir(start_response, fs, ref_label, link_prefix, path, want_json, etag):
    """Serve directory listing as JSON or HTML."""
    entries = fs.ls(path if path else None)

    if want_json:
        body = json.dumps({
            "path": path,
            "ref": ref_label,
            "entries": sorted(entries),
            "type": "directory",
        }).encode()
        start_response("200 OK", [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
            ("ETag", etag),
            ("Cache-Control", "no-cache"),
        ])
        return [body]

    # HTML listing
    display_path = path or "/"
    lines = ["<html><body>", f"<h1>{html.escape(display_path)}</h1>", "<ul>"]
    for entry in sorted(entries):
        href = _href(link_prefix, path, entry) if path else _href(link_prefix, entry)
        lines.append(f'<li><a href="{href}">{html.escape(entry)}</a></li>')
    lines.append("</ul>")
    lines.append("</body></html>")
    body = "\n".join(lines).encode()
    start_response("200 OK", [
        ("Content-Type", "text/html; charset=utf-8"),
        ("Content-Length", str(len(body))),
        ("ETag", etag),
        ("Cache-Control", "no-cache"),
    ])
    return [body]


def _send_404(start_response, message="Not found"):
    """Send a 404 response."""
    body = message.encode()
    start_response("404 Not Found", [
        ("Content-Type", "text/plain"),
        ("Content-Length", str(len(body))),
    ])
    return [body]


# ---------------------------------------------------------------------------
# CLI command
# ---------------------------------------------------------------------------

@main.command()
@_repo_option
@click.option("--host", default="127.0.0.1", help="Bind address (default: 127.0.0.1).")
@click.option("--port", "-p", default=8000, type=int,
              help="Port to listen on (default: 8000, use 0 for OS-assigned).")
@_branch_option
@_snapshot_options
@click.option("--all", "all_refs", is_flag=True, default=False,
              help="Multi-ref mode: expose all branches and tags via /<ref>/<path>.")
@click.option("--cors", is_flag=True, default=False,
              help="Enable CORS headers (Access-Control-Allow-Origin: *).")
@click.option("--no-cache", "no_cache", is_flag=True, default=False,
              help="Send Cache-Control: no-store on every response.")
@click.option("--base-path", "base_path", default="",
              help="URL prefix to mount under (e.g. /data).")
@click.option("--open", "open_browser", is_flag=True, default=False,
              help="Open the URL in the default browser on start.")
@click.option("--quiet", "-q", is_flag=True, default=False,
              help="Suppress per-request log output.")
@click.option("--max-file-size", "max_file_size_mb", type=int, default=250,
              help="Maximum file size to serve in MB (default: 250, 0 = unlimited).")
@click.pass_context
def serve(ctx, host, port, branch, ref, at_path, match_pattern, before, back,
          all_refs, cors, no_cache, base_path, open_browser, quiet, max_file_size_mb):
    """Serve repository files over HTTP.

    By default, serves the current branch at /<path>.  Use --ref, --back,
    --before, etc. to pin a specific snapshot.  Use --all to expose every
    branch and tag via /<ref>/<path>.

    \b
    Examples:
        vost serve -r data.git
        vost serve -r data.git -b dev
        vost serve -r data.git --ref v1.0
        vost serve -r data.git --all --cors
        vost serve -r data.git --base-path /data -p 9000
        vost serve -r data.git --open --no-cache
    """
    from wsgiref.simple_server import make_server, WSGIRequestHandler

    store = _open_store(_require_repo(ctx))

    # Normalize base_path: strip trailing slash, ensure leading slash
    if base_path:
        base_path = "/" + base_path.strip("/")

    max_file_size = max_file_size_mb * 1024 * 1024 if max_file_size_mb > 0 else 0

    if all_refs:
        if ref or at_path or match_pattern or before or back:
            raise click.ClickException(
                "--all cannot be combined with --ref, --path, --match, --before, or --back"
            )
        app = _make_app(store, cors=cors, no_cache=no_cache, base_path=base_path,
                        max_file_size=max_file_size)
        mode = "multi-ref"
    else:
        branch = branch or _current_branch(store)
        ref_label = ref or branch

        def _resolve():
            return _resolve_fs(store, branch, ref,
                               at_path=at_path, match_pattern=match_pattern,
                               before=before, back=back)

        app = _make_app(store, resolver=_resolve, ref_label=ref_label,
                        cors=cors, no_cache=no_cache, base_path=base_path,
                        max_file_size=max_file_size)
        mode = f"branch {branch} (live)"
        if back:
            mode += f" ~{back}"

    if quiet:
        class _Handler(WSGIRequestHandler):
            def log_request(self, code="-", size="-"):
                pass
    else:
        class _Handler(WSGIRequestHandler):
            def log_request(self, code="-", size="-"):
                click.echo(
                    f"{self.client_address[0]} - {self.command} {self.path} {code}",
                    err=True,
                )

    server = make_server(host, port, app, handler_class=_Handler)
    url = f"http://{host}:{server.server_port}{base_path}/"
    click.echo(f"Serving {_require_repo(ctx)} ({mode}) at {url}", err=True)
    click.echo("Press Ctrl+C to stop.", err=True)

    if open_browser:
        import webbrowser
        webbrowser.open(url)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        click.echo("\nStopped.", err=True)
    finally:
        server.server_close()
