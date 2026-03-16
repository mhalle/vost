"""serve — HTTP file server for repo contents."""

from __future__ import annotations

import html
import json
import logging
import mimetypes
import sys
from datetime import datetime, timezone
from urllib.parse import quote

import click

_logger = logging.getLogger("vost.serve")

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

# Extensions that should be served as text/plain when mimetypes doesn't
# recognize them.  Covers source code, config, and data formats.
_TEXT_EXTENSIONS = frozenset((
    # Programming languages
    ".py", ".pyi", ".pyw", ".rs", ".go", ".c", ".h", ".cpp", ".hpp", ".cc",
    ".cxx", ".hxx", ".cs", ".java", ".kt", ".kts", ".scala", ".clj",
    ".cljs", ".erl", ".ex", ".exs", ".hs", ".ml", ".mli", ".fs", ".fsi",
    ".fsx", ".r", ".R", ".jl", ".lua", ".rb", ".pl", ".pm", ".php",
    ".swift", ".m", ".mm", ".v", ".sv", ".vhd", ".vhdl", ".zig", ".nim",
    ".d", ".ada", ".adb", ".ads", ".pas", ".pp",
    # Shell / scripting
    ".sh", ".bash", ".zsh", ".fish", ".csh", ".ksh", ".ps1", ".psm1",
    ".bat", ".cmd",
    # Web / markup
    ".ts", ".tsx", ".jsx", ".vue", ".svelte", ".astro",
    ".sass", ".scss", ".less", ".styl",
    ".pug", ".slim", ".haml", ".ejs", ".hbs", ".mustache",
    ".graphql", ".gql", ".proto",
    # Config / data
    ".toml", ".ini", ".cfg", ".conf", ".env", ".properties",
    ".editorconfig", ".gitignore", ".gitattributes", ".dockerignore",
    ".flake8", ".pylintrc", ".rubocop",
    ".nix", ".dhall", ".tf", ".hcl",
    ".cmake", ".mk", ".makefile",
    # Documentation / text
    ".rst", ".tex", ".bib", ".adoc", ".org", ".wiki",
    ".diff", ".patch",
    # Data formats
    ".jsonl", ".ndjson", ".jsonc", ".json5",
    ".sql", ".graphql",
    ".dot", ".gv",
    ".srt", ".vtt", ".ass",
))


def _guess_mime(path):
    """Return a browser-friendly MIME type for *path*."""
    mime, _ = mimetypes.guess_type(path)
    if mime is not None:
        return _MIME_OVERRIDES.get(mime, mime)
    # Check for known text extensions
    dot = path.rfind(".")
    if dot >= 0 and path[dot:].lower() in _TEXT_EXTENSIONS:
        return "text/plain; charset=utf-8"
    # Dotfiles without extension (Makefile, Dockerfile, etc.)
    basename = path.rsplit("/", 1)[-1]
    if basename in ("Makefile", "Dockerfile", "Vagrantfile", "Gemfile",
                    "Rakefile", "Procfile", "Brewfile", "Justfile",
                    "CMakeLists.txt", "OWNERS", "CODEOWNERS",
                    "LICENSE", "LICENCE", "AUTHORS", "CONTRIBUTORS"):
        return "text/plain; charset=utf-8"
    return "application/octet-stream"


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


class _AccessLogger:
    """Write CLF access-log lines to stderr and/or a file."""

    def __init__(self, quiet=False, log_file=None):
        self._quiet = quiet
        self._file = open(log_file, "a") if log_file else None

    def log(self, client_ip, method, path, status, size):
        now = datetime.now(timezone.utc).strftime("%d/%b/%Y:%H:%M:%S %z")
        line = f'{client_ip} - - [{now}] "{method} {path} HTTP/1.1" {status} {size}\n'
        if not self._quiet:
            sys.stderr.write(line)
        if self._file:
            self._file.write(line)
            self._file.flush()

    def close(self):
        if self._file:
            self._file.close()
            self._file = None


def _logging_middleware(app, access_logger):
    """WSGI middleware that logs each request in CLF format."""

    def wrapped(environ, start_response):
        captured = {}

        def logging_start_response(status, headers):
            captured["status"] = status.split(" ", 1)[0]
            for k, v in headers:
                if k.lower() == "content-length":
                    captured["size"] = v
                    break
            return start_response(status, headers)

        try:
            result = app(environ, logging_start_response)
        except Exception:
            _logger.exception(
                "Internal server error on %s %s",
                environ.get("REQUEST_METHOD", "GET"),
                environ.get("PATH_INFO", "/"),
            )
            body = b"Internal server error"
            start_response("500 Internal Server Error", [
                ("Content-Type", "text/plain"),
                ("Content-Length", str(len(body))),
            ])
            captured["status"] = "500"
            captured["size"] = str(len(body))
            result = [body]

        access_logger.log(
            environ.get("REMOTE_ADDR", "-"),
            environ.get("REQUEST_METHOD", "GET"),
            environ.get("PATH_INFO", "/"),
            captured.get("status", "-"),
            captured.get("size", "-"),
        )
        return result

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
              max_file_size=_DEFAULT_MAX_FILE_SIZE,
              access_logger=None):
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
    if access_logger is not None:
        result = _logging_middleware(result, access_logger)
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

    # Classify entries as files or directories
    dir_entries = []
    for entry in sorted(entries):
        entry_path = f"{path}/{entry}" if path else entry
        is_dir = fs.is_dir(entry_path)
        dir_entries.append((entry, is_dir))

    if want_json:
        json_entries = []
        for entry, is_dir in dir_entries:
            json_entries.append(entry + "/" if is_dir else entry)
        body = json.dumps({
            "path": path,
            "ref": ref_label,
            "entries": json_entries,
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
    for entry, is_dir in dir_entries:
        suffix = "/" if is_dir else ""
        entry_href = _href(link_prefix, path, entry) if path else _href(link_prefix, entry)
        if is_dir:
            entry_href += "/"
        lines.append(f'<li><a href="{entry_href}">{html.escape(entry)}{suffix}</a></li>')
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
              help="Suppress access log on stderr (errors still logged; --log-file still writes).")
@click.option("--log-file", "log_file", type=click.Path(), default=None,
              help="Append access log to file (CLF format).")
@click.option("--max-file-size", "max_file_size_mb", type=int, default=250,
              help="Maximum file size to serve in MB (default: 250, 0 = unlimited).")
@click.pass_context
def serve(ctx, host, port, branch, ref, at_path, match_pattern, before, back,
          all_refs, cors, no_cache, base_path, open_browser, quiet, log_file,
          max_file_size_mb):
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

    logging.basicConfig(
        level=logging.INFO,
        format="%(levelname)s: %(message)s",
        stream=sys.stderr,
    )

    store = _open_store(_require_repo(ctx))

    # Normalize base_path: strip trailing slash, ensure leading slash
    if base_path:
        base_path = "/" + base_path.strip("/")

    max_file_size = max_file_size_mb * 1024 * 1024 if max_file_size_mb > 0 else 0

    access_logger = _AccessLogger(quiet=quiet, log_file=log_file)

    if all_refs:
        if ref or at_path or match_pattern or before or back:
            raise click.ClickException(
                "--all cannot be combined with --ref, --path, --match, --before, or --back"
            )
        app = _make_app(store, cors=cors, no_cache=no_cache, base_path=base_path,
                        max_file_size=max_file_size, access_logger=access_logger)
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
                        max_file_size=max_file_size, access_logger=access_logger)
        mode = f"branch {branch} (live)"
        if back:
            mode += f" ~{back}"

    # Suppress wsgiref's own per-request log — access logger handles it
    class _Handler(WSGIRequestHandler):
        def log_request(self, code="-", size="-"):
            pass

        def log_error(self, format, *args):
            _logger.error(format, *args)

    server = make_server(host, port, app, handler_class=_Handler)
    url = f"http://{host}:{server.server_port}{base_path}/"
    _logger.info("Serving %s (%s) at %s", _require_repo(ctx), mode, url)
    _logger.info("Press Ctrl+C to stop.")

    if open_browser:
        import webbrowser
        webbrowser.open(url)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        _logger.info("Stopped.")
    finally:
        access_logger.close()
        server.server_close()
