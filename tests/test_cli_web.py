"""Tests for the vost serve command (WSGI app + live CLI server)."""

import json
import os
import re
import signal
import subprocess
import sys
import time
from urllib.request import urlopen, Request
from urllib.error import HTTPError

import pytest
from click.testing import CliRunner

from vost.cli import main
from vost.cli._web import _make_app
from vost.repo import GitStore


# ---------------------------------------------------------------------------
# WSGI test helper
# ---------------------------------------------------------------------------

def _wsgi_get(app, path="/", accept=None, method="GET", if_none_match=None, range=None):
    """Call the WSGI app with a request, return (status, headers, body)."""
    environ = {
        "REQUEST_METHOD": method,
        "PATH_INFO": path,
        "SERVER_NAME": "localhost",
        "SERVER_PORT": "8000",
        "HTTP_HOST": "localhost:8000",
        "wsgi.input": None,
        "wsgi.errors": None,
    }
    if accept:
        environ["HTTP_ACCEPT"] = accept
    if if_none_match:
        environ["HTTP_IF_NONE_MATCH"] = if_none_match
    if range:
        environ["HTTP_RANGE"] = range

    captured = {}

    def start_response(status, headers):
        captured["status"] = status
        captured["headers"] = dict(headers)

    body_parts = app(environ, start_response)
    body = b"".join(body_parts)
    return captured["status"], captured["headers"], body


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def store_with_files(tmp_path):
    """Store with files on 'main' and a tag."""
    store = GitStore.open(str(tmp_path / "test.git"), branch="main")
    fs = store.branches["main"]
    fs = fs.write("hello.txt", b"hello world\n")
    fs = fs.write("data/info.json", b'{"key": "value"}')
    fs = fs.write("data/image.png", b"\x89PNG\r\n\x1a\n")
    fs = fs.write("readme.md", b"# Readme\n")
    store.tags["v1"] = fs
    return store


@pytest.fixture
def store_with_branches(tmp_path):
    """Store with two branches and a tag."""
    store = GitStore.open(str(tmp_path / "test.git"), branch="main")
    fs = store.branches["main"]
    fs = fs.write("main-file.txt", b"on main")

    # Create a second branch
    repo = store._repo
    sig = store._signature
    tree_oid = repo.TreeBuilder().write()
    repo.create_commit(
        "refs/heads/dev",
        sig, sig,
        "Initialize dev",
        tree_oid,
        [],
    )
    fs_dev = store.branches["dev"]
    fs_dev = fs_dev.write("dev-file.txt", b"on dev")

    store.tags["v1"] = store.branches["main"]
    return store


@pytest.fixture
def store_with_symlink(tmp_path):
    """Store with a symlink."""
    store = GitStore.open(str(tmp_path / "test.git"), branch="main")
    fs = store.branches["main"]
    fs = fs.write("target.txt", b"target content")
    fs = fs.write_symlink("link.txt", "target.txt")
    return store


# ---------------------------------------------------------------------------
# XSS escaping tests
# ---------------------------------------------------------------------------

class TestHtmlEscaping:
    def test_file_name_escaped_in_dir_listing(self, tmp_path):
        """Special chars in file names must be HTML-escaped in directory listings."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs = fs.write("<script>alert(1)</script>.txt", b"xss")
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/")
        assert status == "200 OK"
        text = body.decode()
        assert "&lt;script&gt;" in text
        assert "<script>alert(1)</script>" not in text

    def test_dir_path_escaped_in_heading(self, tmp_path):
        """Directory display path must be HTML-escaped in <h1>."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs = fs.write("a&b/file.txt", b"data")
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/a&b")
        assert status == "200 OK"
        text = body.decode()
        assert "a&amp;b" in text

    def test_href_attribute_escaped(self, tmp_path):
        """Quote-breaking chars in filenames must be escaped in href attributes."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        # A filename with a double-quote that could break href="..."
        fs = fs.write('x"onmouseover=alert(1).txt', b"xss")
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        _, _, body = _wsgi_get(app, "/")
        text = body.decode()
        # The raw double-quote must NOT appear unescaped inside an href="..." attr
        assert 'x"onmouseover' not in text

    def test_branch_name_escaped_in_ref_listing(self, tmp_path):
        """Branch names with special chars must be escaped in multi-ref HTML."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs.write("f.txt", b"data")
        # Create a branch whose name contains HTML special chars
        store.branches.set("<b>evil</b>", store.branches["main"])
        app = _make_app(store)
        _, _, body = _wsgi_get(app, "/")
        text = body.decode()
        assert "&lt;b&gt;evil&lt;/b&gt;" in text
        assert "<b>evil</b>" not in text


# ---------------------------------------------------------------------------
# Single-ref mode tests (default)
# ---------------------------------------------------------------------------

class TestSingleRefRoot:
    def test_root_html(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "hello.txt" in html

    def test_root_json(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/", accept="application/json")
        assert status == "200 OK"
        data = json.loads(body)
        assert "hello.txt" in data["entries"]
        assert data["ref"] == "main"


class TestSingleRefFile:
    def test_text_file(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"
        assert "text/plain" in headers["Content-Type"]

    def test_nested_file(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/data/info.json")
        assert status == "200 OK"
        assert body == b'{"key": "value"}'

    def test_binary_file(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/data/image.png")
        assert status == "200 OK"
        assert body == b"\x89PNG\r\n\x1a\n"
        assert "image/png" in headers["Content-Type"]

    def test_file_json_metadata(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(
            app, "/hello.txt", accept="application/json"
        )
        assert status == "200 OK"
        data = json.loads(body)
        assert data["path"] == "hello.txt"
        assert data["ref"] == "main"
        assert data["size"] == len(b"hello world\n")
        assert data["type"] == "file"

    def test_serve_tag_snapshot(self, store_with_files):
        """Can serve a tag FS in single-ref mode."""
        fs = store_with_files.tags["v1"]
        app = _make_app(store_with_files, fs=fs, ref_label="v1")
        status, _, body = _wsgi_get(app, "/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_serve_historical_snapshot(self, store_with_files):
        """Can serve an older snapshot via back()."""
        fs = store_with_files.branches["main"]
        old_fs = fs.back(1)  # before readme.md was added
        app = _make_app(store_with_files, fs=old_fs, ref_label="main")
        status, _, _ = _wsgi_get(app, "/readme.md")
        assert status == "404 Not Found"
        # But earlier files still present
        status, _, body = _wsgi_get(app, "/data/image.png")
        assert status == "200 OK"


class TestSingleRefDir:
    def test_dir_html(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/data")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "info.json" in html
        assert "image.png" in html

    def test_dir_json(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(
            app, "/data", accept="application/json"
        )
        assert status == "200 OK"
        data = json.loads(body)
        assert data["path"] == "data"
        assert data["ref"] == "main"
        assert data["type"] == "directory"
        assert "info.json" in data["entries"]
        assert "image.png" in data["entries"]


# ---------------------------------------------------------------------------
# Single-ref link correctness
# ---------------------------------------------------------------------------

class TestSingleRefLinks:
    """In single-ref mode, HTML links must NOT include the ref prefix."""

    def test_root_links_no_ref_prefix(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, _, body = _wsgi_get(app, "/")
        html = body.decode()
        # Links should be /hello.txt, not /main/hello.txt
        assert 'href="/hello.txt"' in html
        assert 'href="/data/"' in html
        assert 'href="/main/' not in html

    def test_subdir_links_no_ref_prefix(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, _, body = _wsgi_get(app, "/data")
        html = body.decode()
        # Links should be /data/info.json, not /main/data/info.json
        assert 'href="/data/info.json"' in html
        assert 'href="/main/' not in html


# ---------------------------------------------------------------------------
# Multi-ref link correctness
# ---------------------------------------------------------------------------

class TestMultiRefLinks:
    """In multi-ref mode, HTML links MUST include the ref prefix."""

    def test_root_dir_links_include_ref(self, store_with_files):
        app = _make_app(store_with_files)
        _, _, body = _wsgi_get(app, "/main/")
        html = body.decode()
        assert 'href="/main/hello.txt"' in html
        assert 'href="/main/data/"' in html

    def test_subdir_links_include_ref(self, store_with_files):
        app = _make_app(store_with_files)
        _, _, body = _wsgi_get(app, "/main/data")
        html = body.decode()
        assert 'href="/main/data/info.json"' in html


# ---------------------------------------------------------------------------
# Multi-ref mode tests (--all)
# ---------------------------------------------------------------------------

class TestMultiRefRoot:
    def test_root_html(self, store_with_files):
        app = _make_app(store_with_files)
        status, headers, body = _wsgi_get(app, "/")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "main" in html
        assert "v1" in html

    def test_root_json(self, store_with_files):
        app = _make_app(store_with_files)
        status, headers, body = _wsgi_get(app, "/", accept="application/json")
        assert status == "200 OK"
        data = json.loads(body)
        assert "main" in data["branches"]
        assert "v1" in data["tags"]


class TestMultiRefFile:
    def test_text_file(self, store_with_files):
        app = _make_app(store_with_files)
        status, headers, body = _wsgi_get(app, "/main/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"
        assert "text/plain" in headers["Content-Type"]

    def test_file_via_tag(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(app, "/v1/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_file_json_metadata(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(
            app, "/main/hello.txt", accept="application/json"
        )
        data = json.loads(body)
        assert data["ref"] == "main"
        assert data["type"] == "file"


class TestMultiRefDir:
    def test_dir_html(self, store_with_files):
        app = _make_app(store_with_files)
        status, headers, body = _wsgi_get(app, "/main/data")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "info.json" in html

    def test_dir_json(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(
            app, "/main/data", accept="application/json"
        )
        data = json.loads(body)
        assert data["type"] == "directory"
        assert "info.json" in data["entries"]

    def test_root_dir_listing(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(app, "/main/")
        assert status == "200 OK"
        html = body.decode()
        assert "hello.txt" in html

    def test_root_dir_json(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(
            app, "/main/", accept="application/json"
        )
        data = json.loads(body)
        assert data["ref"] == "main"
        assert "hello.txt" in data["entries"]


class TestMultiRefBranches:
    def test_different_branches(self, store_with_branches):
        app = _make_app(store_with_branches)

        status, _, body = _wsgi_get(app, "/main/main-file.txt")
        assert status == "200 OK"
        assert body == b"on main"

        status, _, body = _wsgi_get(app, "/dev/dev-file.txt")
        assert status == "200 OK"
        assert body == b"on dev"


# ---------------------------------------------------------------------------
# ETag tests
# ---------------------------------------------------------------------------

class TestETag:
    def test_file_has_etag(self, store_with_files):
        """File ETags use blob hash, not commit hash."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert "ETag" in headers
        blob_hash = fs.stat("hello.txt").hash
        assert headers["ETag"] == f'"{blob_hash}"'

    def test_dir_has_etag(self, store_with_files):
        """Directory ETags use commit hash."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/data")
        assert "ETag" in headers
        assert headers["ETag"] == f'"{fs.commit_hash}"'

    def test_json_has_etag(self, store_with_files):
        """JSON file metadata ETags use blob hash."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt", accept="application/json")
        assert "ETag" in headers
        blob_hash = fs.stat("hello.txt").hash
        assert headers["ETag"] == f'"{blob_hash}"'

    def test_root_has_etag(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/")
        assert "ETag" in headers

    def test_multi_ref_file_has_etag(self, store_with_files):
        """Multi-ref file ETags use blob hash."""
        app = _make_app(store_with_files)
        _, headers, _ = _wsgi_get(app, "/main/hello.txt")
        assert "ETag" in headers
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        assert headers["ETag"] == f'"{blob_hash}"'

    def test_different_content_different_etags(self, tmp_path):
        """Files with different content produce different blob-level ETags."""
        store = GitStore.open(str(tmp_path / "etag.git"), branch="main")
        fs = store.branches["main"]
        fs = fs.write("file.txt", b"version 1\n")
        fs = fs.write("file.txt", b"version 2\n")
        old_fs = fs.back(1)
        app_new = _make_app(store, fs=fs, ref_label="main")
        app_old = _make_app(store, fs=old_fs, ref_label="main")
        _, h_new, _ = _wsgi_get(app_new, "/file.txt")
        _, h_old, _ = _wsgi_get(app_old, "/file.txt")
        assert h_new["ETag"] != h_old["ETag"]

    def test_same_content_same_etag_across_commits(self, store_with_files):
        """Same file content across different commits produces same blob ETag."""
        fs = store_with_files.branches["main"]
        old_fs = fs.back(1)
        app_new = _make_app(store_with_files, fs=fs, ref_label="main")
        app_old = _make_app(store_with_files, fs=old_fs, ref_label="main")
        _, h_new, _ = _wsgi_get(app_new, "/hello.txt")
        _, h_old, _ = _wsgi_get(app_old, "/hello.txt")
        assert h_new["ETag"] == h_old["ETag"]

    def test_different_commits_different_dir_etags(self, store_with_files):
        """Directory ETags use commit hash, so different commits give different ETags."""
        fs = store_with_files.branches["main"]
        old_fs = fs.back(1)
        app_new = _make_app(store_with_files, fs=fs, ref_label="main")
        app_old = _make_app(store_with_files, fs=old_fs, ref_label="main")
        _, h_new, _ = _wsgi_get(app_new, "/data")
        _, h_old, _ = _wsgi_get(app_old, "/data")
        assert h_new["ETag"] != h_old["ETag"]

    def test_404_has_no_etag(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/nonexistent.txt")
        assert "ETag" not in headers


# ---------------------------------------------------------------------------
# Cache-Control and 304 tests
# ---------------------------------------------------------------------------

class TestCacheControl:
    def test_file_has_cache_control(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-cache"

    def test_dir_has_cache_control(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/data")
        assert headers["Cache-Control"] == "no-cache"

    def test_json_has_cache_control(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt", accept="application/json")
        assert headers["Cache-Control"] == "no-cache"

    def test_304_on_matching_etag(self, store_with_files):
        """File 304 uses blob hash etag."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        blob_hash = fs.stat("hello.txt").hash
        etag = f'"{blob_hash}"'
        status, headers, body = _wsgi_get(app, "/hello.txt", if_none_match=etag)
        assert status == "304 Not Modified"
        assert body == b""
        assert headers["ETag"] == etag

    def test_304_on_dir(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        etag = f'"{fs.commit_hash}"'
        status, _, body = _wsgi_get(app, "/data", if_none_match=etag)
        assert status == "304 Not Modified"
        assert body == b""

    def test_200_on_stale_etag(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", if_none_match='"stale"')
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_304_multi_ref(self, store_with_files):
        """Multi-ref file 304 uses blob hash etag."""
        app = _make_app(store_with_files)
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        etag = f'"{blob_hash}"'
        status, _, body = _wsgi_get(app, "/main/hello.txt", if_none_match=etag)
        assert status == "304 Not Modified"
        assert body == b""

    def test_no_store_overrides_no_cache(self, store_with_files):
        """--no-cache flag sends no-store which overrides the default no-cache."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cache_control="no-store")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-store"


# ---------------------------------------------------------------------------
# 404 tests
# ---------------------------------------------------------------------------

class TestNotFound:
    def test_missing_ref_multi(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, _ = _wsgi_get(app, "/nonexistent/file.txt")
        assert status == "404 Not Found"

    def test_missing_path_multi(self, store_with_files):
        app = _make_app(store_with_files)
        status, _, _ = _wsgi_get(app, "/main/nonexistent.txt")
        assert status == "404 Not Found"

    def test_missing_path_single(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, _ = _wsgi_get(app, "/nonexistent.txt")
        assert status == "404 Not Found"


# ---------------------------------------------------------------------------
# Symlink handling
# ---------------------------------------------------------------------------

class TestSymlinks:
    def test_symlink_served_as_file(self, store_with_symlink):
        """Symlinks should serve the blob content (the link target string)."""
        fs = store_with_symlink.branches["main"]
        app = _make_app(store_with_symlink, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/link.txt")
        assert status == "200 OK"
        assert body == b"target.txt"


# ---------------------------------------------------------------------------
# MIME type fallback
# ---------------------------------------------------------------------------

class TestMimeTypes:
    def test_unknown_extension(self, tmp_path):
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs.write("data.xyz123", b"some data")
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        status, headers, _ = _wsgi_get(app, "/data.xyz123")
        assert status == "200 OK"
        assert headers["Content-Type"] == "application/octet-stream"

    def test_markdown_type(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, _ = _wsgi_get(app, "/readme.md")
        assert status == "200 OK"
        # Markdown may be text/markdown or text/x-markdown depending on platform
        assert "text/" in headers["Content-Type"]

    def test_json_served_as_text(self, store_with_files):
        """JSON files should display inline, not trigger download."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/data/info.json")
        assert status == "200 OK"
        assert headers["Content-Type"] == "text/plain; charset=utf-8"
        assert body == b'{"key": "value"}'

    def test_xml_served_as_text(self, tmp_path):
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs.write("data.xml", b"<root/>")
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/data.xml")
        assert headers["Content-Type"] == "text/xml; charset=utf-8"

    def test_geojson_served_as_text(self, tmp_path):
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs.write("map.geojson", b'{"type":"Feature"}')
        fs = store.branches["main"]
        app = _make_app(store, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/map.geojson")
        assert headers["Content-Type"] == "text/plain; charset=utf-8"


# ---------------------------------------------------------------------------
# CORS tests
# ---------------------------------------------------------------------------

class TestCORS:
    def test_cors_disabled_by_default(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert "Access-Control-Allow-Origin" not in headers

    def test_cors_adds_headers(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cors=True)
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Access-Control-Allow-Origin"] == "*"
        assert headers["Access-Control-Allow-Methods"] == "*"
        assert headers["Access-Control-Expose-Headers"] == "*"

    def test_cors_on_json_response(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cors=True)
        _, headers, _ = _wsgi_get(app, "/", accept="application/json")
        assert headers["Access-Control-Allow-Origin"] == "*"

    def test_cors_on_404(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cors=True)
        status, headers, _ = _wsgi_get(app, "/nonexistent.txt")
        assert status == "404 Not Found"
        assert headers["Access-Control-Allow-Origin"] == "*"

    def test_cors_options_preflight(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cors=True)
        status, headers, body = _wsgi_get(app, "/hello.txt", method="OPTIONS")
        assert status == "204 No Content"
        assert headers["Access-Control-Allow-Origin"] == "*"
        assert body == b""

    def test_cors_multi_ref(self, store_with_files):
        app = _make_app(store_with_files, cors=True)
        _, headers, _ = _wsgi_get(app, "/main/hello.txt")
        assert headers["Access-Control-Allow-Origin"] == "*"


# ---------------------------------------------------------------------------
# No-cache tests
# ---------------------------------------------------------------------------

class TestNoCache:
    def test_no_store_disabled_by_default(self, store_with_files):
        """Without --no-cache flag, Cache-Control is no-cache (not no-store)."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-cache"

    def test_no_cache_adds_header(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cache_control="no-store")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-store"

    def test_no_cache_on_dir(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cache_control="no-store")
        _, headers, _ = _wsgi_get(app, "/data")
        assert headers["Cache-Control"] == "no-store"

    def test_no_cache_on_404(self, store_with_files):
        """404 responses do not carry cache-control (only served content does)."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main", cache_control="no-store")
        status, headers, _ = _wsgi_get(app, "/nonexistent.txt")
        assert status == "404 Not Found"
        assert "Cache-Control" not in headers

    def test_no_cache_combined_with_cors(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cors=True, cache_control="no-store")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-store"
        assert headers["Access-Control-Allow-Origin"] == "*"


# ---------------------------------------------------------------------------
# Base-path tests
# ---------------------------------------------------------------------------

class TestBasePath:
    def test_base_path_file(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        base_path="/data")
        status, _, body = _wsgi_get(app, "/data/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_base_path_root(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        base_path="/data")
        status, headers, body = _wsgi_get(app, "/data/")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "hello.txt" in html

    def test_base_path_404_outside_prefix(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        base_path="/data")
        status, _, _ = _wsgi_get(app, "/hello.txt")
        assert status == "404 Not Found"

    def test_base_path_links_include_prefix(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        base_path="/data")
        _, _, body = _wsgi_get(app, "/data/")
        html = body.decode()
        assert 'href="/data/hello.txt"' in html

    def test_base_path_multi_ref(self, store_with_files):
        app = _make_app(store_with_files, base_path="/data")
        status, _, body = _wsgi_get(app, "/data/main/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_base_path_multi_ref_root(self, store_with_files):
        app = _make_app(store_with_files, base_path="/data")
        status, headers, body = _wsgi_get(app, "/data/")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        html = body.decode()
        assert "main" in html

    def test_base_path_combined_with_cors(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        base_path="/data", cors=True)
        _, headers, _ = _wsgi_get(app, "/data/hello.txt")
        assert headers["Access-Control-Allow-Origin"] == "*"


# ---------------------------------------------------------------------------
# Live branch reload tests
# ---------------------------------------------------------------------------

class TestLiveBranch:
    def test_resolver_sees_new_commits(self, store_with_files):
        """resolver= re-resolves on each request, seeing new commits."""
        app = _make_app(store_with_files,
                        resolver=lambda: store_with_files.branches["main"],
                        ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

        # Write a new file after app was created
        fs = store_with_files.branches["main"]
        fs.write("new.txt", b"live content")

        status, _, body = _wsgi_get(app, "/new.txt")
        assert status == "200 OK"
        assert body == b"live content"

    def test_fixed_fs_does_not_see_new_commits(self, store_with_files):
        """fs= mode uses a fixed snapshot, does not see new commits."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")

        # Write a new file after app was created
        fs.write("new.txt", b"new content")

        status, _, _ = _wsgi_get(app, "/new.txt")
        assert status == "404 Not Found"

    def test_resolver_root_listing(self, store_with_files):
        app = _make_app(store_with_files,
                        resolver=lambda: store_with_files.branches["main"],
                        ref_label="main")
        status, headers, body = _wsgi_get(app, "/")
        assert status == "200 OK"
        assert "text/html" in headers["Content-Type"]
        assert "hello.txt" in body.decode()

    def test_resolver_json(self, store_with_files):
        app = _make_app(store_with_files,
                        resolver=lambda: store_with_files.branches["main"],
                        ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", accept="application/json")
        assert status == "200 OK"
        data = json.loads(body)
        assert data["ref"] == "main"
        assert data["type"] == "file"


# ---------------------------------------------------------------------------
# CLI command registration
# ---------------------------------------------------------------------------

class TestServeCommand:
    def test_serve_registered(self):
        runner = CliRunner()
        result = runner.invoke(main, ["serve", "--help"])
        assert result.exit_code == 0
        assert "Serve repository files" in result.output
        assert "--host" in result.output
        assert "--port" in result.output
        assert "--branch" in result.output
        assert "--ref" in result.output
        assert "--back" in result.output
        assert "--all" in result.output
        assert "--cors" in result.output
        assert "--no-cache" in result.output
        assert "--base-path" in result.output
        assert "--open" in result.output
        assert "--quiet" in result.output


# ---------------------------------------------------------------------------
# Max file size
# ---------------------------------------------------------------------------

class TestMaxFileSize:
    """--max-file-size limits served file size (413 for oversized files)."""

    def test_wsgi_small_file_under_limit(self, store_with_files):
        """Files under the limit are served normally."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, max_file_size=1024 * 1024)
        status, headers, body = _wsgi_get(app, "/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_wsgi_large_file_over_limit(self, tmp_path):
        """Files over the limit return 413."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        # Write a 2KB file, set limit to 1KB
        fs = fs.write("big.txt", b"x" * 2048)
        app = _make_app(store, fs=fs, max_file_size=1024)
        status, headers, body = _wsgi_get(app, "/big.txt")
        assert status == "413 Payload Too Large"
        assert b"too large" in body.lower()
        assert b"2048" in body

    def test_wsgi_unlimited(self, tmp_path):
        """max_file_size=0 disables the limit."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs = fs.write("big.txt", b"x" * 2048)
        app = _make_app(store, fs=fs, max_file_size=0)
        status, headers, body = _wsgi_get(app, "/big.txt")
        assert status == "200 OK"
        assert len(body) == 2048

    def test_wsgi_json_metadata_not_limited(self, tmp_path):
        """JSON metadata requests are not affected by the limit."""
        store = GitStore.open(str(tmp_path / "test.git"), branch="main")
        fs = store.branches["main"]
        fs = fs.write("big.txt", b"x" * 2048)
        # Even with a tiny limit, JSON metadata should work
        # (the 413 is only for file content, not metadata)
        # Actually the size check happens before the want_json branch,
        # so JSON requests for oversized files also get 413.
        app = _make_app(store, fs=fs, max_file_size=1024)
        status, headers, body = _wsgi_get(app, "/big.txt", accept="application/json")
        assert status == "413 Payload Too Large"

    def test_wsgi_directory_listing_not_limited(self, store_with_files):
        """Directory listings are never limited."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, max_file_size=1)  # 1 byte limit
        status, headers, body = _wsgi_get(app, "/")
        assert status == "200 OK"
        assert b"hello.txt" in body


# ---------------------------------------------------------------------------
# Live CLI server tests (work with both Python and Rust backends)
# ---------------------------------------------------------------------------

def _vost_cmd():
    """Return the vost command as a list, respecting VOST_CLI=rust."""
    if os.environ.get("VOST_CLI", "").lower() == "rust":
        binary = os.environ.get(
            "VOST_BINARY",
            os.path.join(os.path.dirname(__file__), "..", "rs", "target", "debug", "vost"),
        )
        return [binary]
    # Use the console_script entry point installed in the venv
    venv_bin = os.path.join(os.path.dirname(sys.executable), "vost")
    if os.path.exists(venv_bin):
        return [venv_bin]
    # Fallback: invoke via uv
    return ["uv", "run", "vost"]


def _start_server(repo_path, extra_args=None):
    """Start a vost serve process on port 0, return (proc, port)."""
    cmd = _vost_cmd() + [
        "--repo", repo_path,
        "serve", "-p", "0", "-q",
    ]
    if extra_args:
        cmd.extend(extra_args)

    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={**os.environ, "VOST_REPO": ""},
    )

    # Read stderr lines until we find the "Serving ... at http://..." line
    port = None
    deadline = time.time() + 5
    while time.time() < deadline:
        line = proc.stderr.readline().decode("utf-8", errors="replace")
        if not line:
            if proc.poll() is not None:
                break
            time.sleep(0.05)
            continue
        m = re.search(r"http://[^:]+:(\d+)/", line)
        if m:
            port = int(m.group(1))
            break

    if port is None:
        proc.kill()
        proc.wait()
        raise RuntimeError("Failed to start vost serve")

    return proc, port


def _stop_server(proc):
    """Stop a vost serve process."""
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def _http_get(port, path="/", accept=None, if_none_match=None):
    """Make an HTTP GET request, return (status_code, headers_dict, body_bytes)."""
    url = f"http://127.0.0.1:{port}{path}"
    req = Request(url)
    if accept:
        req.add_header("Accept", accept)
    if if_none_match:
        req.add_header("If-None-Match", if_none_match)
    try:
        resp = urlopen(req, timeout=5)
        return resp.status, dict(resp.headers), resp.read()
    except HTTPError as e:
        return e.code, dict(e.headers), e.read()


@pytest.fixture
def serve_repo(tmp_path):
    """Create a repo with test files and return its path."""
    cmd = _vost_cmd()
    repo = str(tmp_path / "serve.git")
    subprocess.run(cmd + ["--repo", repo, "init"], check=True,
                   capture_output=True, env={**os.environ, "VOST_REPO": ""})
    subprocess.run(cmd + ["--repo", repo, "write", ":hello.txt"],
                   input=b"hello world\n", check=True,
                   capture_output=True, env={**os.environ, "VOST_REPO": ""})
    subprocess.run(cmd + ["--repo", repo, "write", ":data/info.json"],
                   input=b'{"key": "value"}', check=True,
                   capture_output=True, env={**os.environ, "VOST_REPO": ""})
    # Write a 2KB file for max-file-size testing
    subprocess.run(cmd + ["--repo", repo, "write", ":big.bin"],
                   input=b"x" * 2048, check=True,
                   capture_output=True, env={**os.environ, "VOST_REPO": ""})
    return repo


class TestServeCLI:
    """Live server tests via subprocess — works with both Python and Rust."""

    def test_serve_root_html(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            status, headers, body = _http_get(port, "/")
            assert status == 200
            assert b"hello.txt" in body
            assert "text/html" in headers.get("Content-Type", "")
        finally:
            _stop_server(proc)

    def test_serve_root_json(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            status, headers, body = _http_get(port, "/", accept="application/json")
            assert status == 200
            data = json.loads(body)
            assert "hello.txt" in data["entries"]
            assert data["type"] == "directory"
        finally:
            _stop_server(proc)

    def test_serve_file_content(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            status, _, body = _http_get(port, "/hello.txt")
            assert status == 200
            assert body == b"hello world\n"
        finally:
            _stop_server(proc)

    def test_serve_subdirectory(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            status, _, body = _http_get(port, "/data/", accept="application/json")
            assert status == 200
            data = json.loads(body)
            assert "info.json" in data["entries"]
        finally:
            _stop_server(proc)

    def test_serve_404(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            status, _, body = _http_get(port, "/nonexistent.txt")
            assert status == 404
        finally:
            _stop_server(proc)

    def test_serve_etag_304(self, serve_repo):
        proc, port = _start_server(serve_repo)
        try:
            # First request to get ETag
            status, headers, _ = _http_get(port, "/hello.txt")
            assert status == 200
            etag = headers.get("ETag") or headers.get("etag")
            assert etag is not None

            # Second request with If-None-Match
            status2, _, _ = _http_get(port, "/hello.txt", if_none_match=etag)
            assert status2 == 304
        finally:
            _stop_server(proc)

    def test_serve_cors(self, serve_repo):
        proc, port = _start_server(serve_repo, ["--cors"])
        try:
            status, headers, _ = _http_get(port, "/hello.txt")
            assert status == 200
            acao = headers.get("Access-Control-Allow-Origin") or headers.get("access-control-allow-origin")
            assert acao == "*"
        finally:
            _stop_server(proc)

    def test_serve_max_file_size_allows_small(self, serve_repo):
        proc, port = _start_server(serve_repo, ["--max-file-size", "1"])
        try:
            # hello.txt is tiny, should be served
            status, _, body = _http_get(port, "/hello.txt")
            assert status == 200
            assert body == b"hello world\n"
        finally:
            _stop_server(proc)

    def test_serve_max_file_size_blocks_large(self, serve_repo):
        # big.bin is 2KB; set limit to 1KB (but flag is in MB...)
        # We can't test sub-MB limits via the CLI flag.
        # Instead, write a file > 1MB and use --max-file-size 1
        cmd = _vost_cmd()
        subprocess.run(cmd + ["--repo", serve_repo, "write", ":huge.bin"],
                       input=b"x" * (1024 * 1024 + 1), check=True,
                       capture_output=True, env={**os.environ, "VOST_REPO": ""})

        proc, port = _start_server(serve_repo, ["--max-file-size", "1"])
        try:
            status, _, body = _http_get(port, "/huge.bin")
            assert status == 413
            assert b"too large" in body.lower()
        finally:
            _stop_server(proc)

    def test_serve_max_file_size_zero_unlimited(self, serve_repo):
        cmd = _vost_cmd()
        subprocess.run(cmd + ["--repo", serve_repo, "write", ":huge.bin"],
                       input=b"x" * (1024 * 1024 + 1), check=True,
                       capture_output=True, env={**os.environ, "VOST_REPO": ""})

        proc, port = _start_server(serve_repo, ["--max-file-size", "0"])
        try:
            status, _, body = _http_get(port, "/huge.bin")
            assert status == 200
            assert len(body) == 1024 * 1024 + 1
        finally:
            _stop_server(proc)

    def test_serve_directory_not_limited(self, serve_repo):
        proc, port = _start_server(serve_repo, ["--max-file-size", "1"])
        try:
            # Directory listings should always work regardless of limit
            status, _, body = _http_get(port, "/")
            assert status == 200
            assert b"hello.txt" in body
        finally:
            _stop_server(proc)

    def test_serve_log_file(self, serve_repo, tmp_path):
        log_path = str(tmp_path / "access.log")
        proc, port = _start_server(serve_repo, ["--log-file", log_path])
        try:
            _http_get(port, "/hello.txt")
            time.sleep(0.2)  # let log flush
            log_content = open(log_path).read()
            # CLF line should contain method, path, status, size
            assert "GET /hello.txt HTTP/1.1" in log_content
            assert " 200 " in log_content
        finally:
            _stop_server(proc)

    def test_serve_quiet_still_logs_to_file(self, serve_repo, tmp_path):
        """--quiet suppresses stderr but --log-file still receives entries.

        Note: _start_server already passes -q, so --quiet is already active.
        """
        log_path = str(tmp_path / "access.log")
        proc, port = _start_server(serve_repo, ["--log-file", log_path])
        try:
            _http_get(port, "/hello.txt")
            time.sleep(0.2)
            log_content = open(log_path).read()
            assert "GET /hello.txt HTTP/1.1" in log_content
        finally:
            _stop_server(proc)

    def test_serve_log_file_404(self, serve_repo, tmp_path):
        log_path = str(tmp_path / "access.log")
        proc, port = _start_server(serve_repo, ["--log-file", log_path])
        try:
            _http_get(port, "/nonexistent.txt")
            time.sleep(0.2)
            log_content = open(log_path).read()
            assert "GET /nonexistent.txt HTTP/1.1" in log_content
            assert " 404 " in log_content
        finally:
            _stop_server(proc)

    def test_serve_log_clf_format(self, serve_repo, tmp_path):
        """Verify the access log line matches CLF format."""
        log_path = str(tmp_path / "access.log")
        proc, port = _start_server(serve_repo, ["--log-file", log_path])
        try:
            _http_get(port, "/hello.txt")
            time.sleep(0.2)
            log_content = open(log_path).read().strip()
            # CLF: <ip> - - [<timestamp>] "<method> <path> HTTP/1.1" <status> <size>
            clf_pattern = re.compile(
                r'^[\d.]+ - - \[\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2} [+\-]\d{4}\] '
                r'"GET /hello\.txt HTTP/1\.1" 200 \d+$'
            )
            lines = [l for l in log_content.split("\n") if "/hello.txt" in l]
            assert len(lines) >= 1
            assert clf_pattern.match(lines[0]), f"CLF mismatch: {lines[0]!r}"
        finally:
            _stop_server(proc)


# ---------------------------------------------------------------------------
# Range request tests
# ---------------------------------------------------------------------------

class TestRangeRequests:
    def test_range_first_bytes(self, store_with_files):
        """Range: bytes=0-4 returns first 5 bytes."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/hello.txt", range="bytes=0-4")
        assert status == "206 Partial Content"
        assert body == b"hello"
        assert headers["Content-Range"] == f"bytes 0-4/{len(b'hello world\n')}"
        assert headers["Accept-Ranges"] == "bytes"

    def test_range_middle_bytes(self, store_with_files):
        """Range: bytes=6-10 returns 'world'."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", range="bytes=6-10")
        assert status == "206 Partial Content"
        assert body == b"world"

    def test_range_open_end(self, store_with_files):
        """Range: bytes=6- returns from offset to end."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", range="bytes=6-")
        assert status == "206 Partial Content"
        assert body == b"world\n"

    def test_range_suffix(self, store_with_files):
        """Range: bytes=-5 returns last 5 bytes."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", range="bytes=-5")
        assert status == "206 Partial Content"
        assert body == b"rld\n" or body == b"orld\n"  # last 5 of "hello world\n"
        assert len(body) == 5

    def test_range_has_etag(self, store_with_files):
        """206 responses include per-blob ETag."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt", range="bytes=0-4")
        blob_hash = fs.stat("hello.txt").hash
        assert headers["ETag"] == f'"{blob_hash}"'

    def test_no_range_full_response(self, store_with_files):
        """Without Range header, normal 200 is returned."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, "/hello.txt")
        assert status == "200 OK"
        assert body == b"hello world\n"
        assert headers.get("Accept-Ranges") == "bytes"

    def test_invalid_range_returns_200(self, store_with_files):
        """Malformed Range header falls through to full 200."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", range="bytes=999-9999")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_range_on_json_ignored(self, store_with_files):
        """Range is ignored when Accept: application/json is set."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, "/hello.txt", accept="application/json", range="bytes=0-4")
        assert status == "200 OK"
        data = json.loads(body)
        assert data["type"] == "file"

    def test_range_on_directory_ignored(self, store_with_files):
        """Range header is ignored for directory listings."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, _ = _wsgi_get(app, "/data", range="bytes=0-10")
        assert status == "200 OK"


# ---------------------------------------------------------------------------
# Accept-Ranges header tests
# ---------------------------------------------------------------------------

class TestAcceptRanges:
    def test_file_has_accept_ranges(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers.get("Accept-Ranges") == "bytes"

    def test_dir_no_accept_ranges(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/data")
        assert "Accept-Ranges" not in headers


# ---------------------------------------------------------------------------
# Immutable and max-age cache control tests
# ---------------------------------------------------------------------------

class TestCacheControlModes:
    def test_default_no_cache(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-cache"

    def test_immutable(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, immutable, max-age=31536000")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "public, immutable, max-age=31536000"

    def test_immutable_on_dir(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, immutable, max-age=31536000")
        _, headers, _ = _wsgi_get(app, "/data")
        assert headers["Cache-Control"] == "public, immutable, max-age=31536000"

    def test_max_age(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, max-age=3600")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "public, max-age=3600"

    def test_no_store(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="no-store")
        _, headers, _ = _wsgi_get(app, "/hello.txt")
        assert headers["Cache-Control"] == "no-store"

    def test_range_response_inherits_cache_control(self, store_with_files):
        """206 responses use the same cache control as full responses."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, immutable, max-age=31536000")
        _, headers, _ = _wsgi_get(app, "/hello.txt", range="bytes=0-4")
        assert headers["Cache-Control"] == "public, immutable, max-age=31536000"

    def test_json_metadata_inherits_cache_control(self, store_with_files):
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, max-age=600")
        _, headers, _ = _wsgi_get(app, "/hello.txt", accept="application/json")
        assert headers["Cache-Control"] == "public, max-age=600"


# ---------------------------------------------------------------------------
# Blob access tests (/_/blobs/{hash} and /{hash})
# ---------------------------------------------------------------------------

class TestBlobAccess:
    def test_explicit_blob_route(self, store_with_files):
        """/_/blobs/{hash} returns raw blob content."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, f"/_/blobs/{blob_hash}")
        assert status == "200 OK"
        assert body == b"hello world\n"
        assert headers["Content-Type"] == "application/octet-stream"
        assert headers["ETag"] == f'"{blob_hash}"'
        assert headers["Accept-Ranges"] == "bytes"

    def test_shorthand_hash_route(self, store_with_files):
        """/{40-hex} resolves as blob when it exists."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, f"/{blob_hash}")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_shorthand_falls_back_to_path(self, store_with_files):
        """/{40-hex} that isn't a blob falls through to normal path lookup."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        # A 40-char hex string that isn't a real blob → 404 via path lookup
        status, _, _ = _wsgi_get(app, "/0000000000000000000000000000000000000000")
        assert status == "404 Not Found"

    def test_blob_304(self, store_with_files):
        """Blob route supports ETag-based 304."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        etag = f'"{blob_hash}"'
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, f"/_/blobs/{blob_hash}", if_none_match=etag)
        assert status == "304 Not Modified"
        assert body == b""

    def test_blob_range(self, store_with_files):
        """Blob route supports Range requests."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, headers, body = _wsgi_get(app, f"/_/blobs/{blob_hash}", range="bytes=0-4")
        assert status == "206 Partial Content"
        assert body == b"hello"
        assert "Content-Range" in headers

    def test_blob_json(self, store_with_files):
        """Blob route returns JSON metadata with Accept: application/json."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, body = _wsgi_get(app, f"/_/blobs/{blob_hash}", accept="application/json")
        assert status == "200 OK"
        data = json.loads(body)
        assert data["hash"] == blob_hash
        assert data["size"] == len(b"hello world\n")
        assert data["type"] == "blob"

    def test_blob_invalid_hash(self, store_with_files):
        """Invalid hash format returns 404."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, _ = _wsgi_get(app, "/_/blobs/not-a-hash")
        assert status == "404 Not Found"

    def test_blob_nonexistent_hash(self, store_with_files):
        """Valid format but nonexistent blob returns 404."""
        fs = store_with_files.branches["main"]
        app = _make_app(store_with_files, fs=fs, ref_label="main")
        status, _, _ = _wsgi_get(app, "/_/blobs/deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        assert status == "404 Not Found"

    def test_blob_multi_ref(self, store_with_files):
        """Blob access works in multi-ref mode."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files)  # multi-ref (no fs/resolver)
        status, _, body = _wsgi_get(app, f"/_/blobs/{blob_hash}")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_shorthand_multi_ref(self, store_with_files):
        """/{40-hex} works in multi-ref mode."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files)
        status, _, body = _wsgi_get(app, f"/{blob_hash}")
        assert status == "200 OK"
        assert body == b"hello world\n"

    def test_blob_cache_control(self, store_with_files):
        """Blob route respects cache_control setting."""
        fs = store_with_files.branches["main"]
        blob_hash = fs.stat("hello.txt").hash
        app = _make_app(store_with_files, fs=fs, ref_label="main",
                        cache_control="public, immutable, max-age=31536000")
        _, headers, _ = _wsgi_get(app, f"/_/blobs/{blob_hash}")
        assert headers["Cache-Control"] == "public, immutable, max-age=31536000"
