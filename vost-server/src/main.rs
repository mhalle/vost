use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use clap::Parser;
use dashmap::DashMap;
use tower_http::cors::{Any, CorsLayer};
use vost::fs::LogOptions;
use vost::GitStore;

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

fn parse_before(value: &str) -> Result<u64, String> {
    use chrono::prelude::*;
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.timestamp() as u64);
    }
    if let Ok(nd) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = nd.and_hms_opt(23, 59, 59).unwrap().and_utc();
        return Ok(dt.timestamp() as u64);
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(ndt.and_utc().timestamp() as u64);
    }
    Err(format!(
        "Invalid date: {} (use ISO 8601, e.g. 2024-01-15 or 2024-01-15T14:30:00)",
        value
    ))
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "vost-server", about = "High-performance HTTP file server for vost repositories")]
struct Args {
    /// Path to bare git repository (or set VOST_REPO).
    #[arg(short, long, env = "VOST_REPO")]
    repo: String,
    /// Bind address.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port to listen on (0 for OS-assigned).
    #[arg(short, long, default_value_t = 8000)]
    port: u16,
    /// Branch to serve (default: repo's current branch).
    #[arg(short, long)]
    branch: Option<String>,
    /// Branch, tag, or commit hash to read from.
    #[arg(long = "ref")]
    ref_name: Option<String>,
    /// Walk back N commits from tip.
    #[arg(long, default_value_t = 0)]
    back: usize,
    /// Use latest commit that changed this path.
    #[arg(long = "path")]
    at_path: Option<String>,
    /// Use latest commit matching this message pattern (* and ?).
    #[arg(long = "match")]
    match_pattern: Option<String>,
    /// Use latest commit on or before this date (ISO 8601).
    #[arg(long)]
    before: Option<String>,
    /// Multi-ref mode: expose all branches and tags via /<ref>/<path>.
    #[arg(long = "all")]
    all_refs: bool,
    /// Enable CORS headers.
    #[arg(long)]
    cors: bool,
    /// Send Cache-Control: no-store on every response.
    #[arg(long)]
    no_cache: bool,
    /// Set Cache-Control: immutable, max-age=31536000 (1 year). Ideal for
    /// content-addressed data like Zarr chunks that never change.
    #[arg(long)]
    immutable: bool,
    /// Set Cache-Control: max-age=N (seconds). Overridden by --no-cache or --immutable.
    #[arg(long)]
    max_age: Option<u64>,
    /// URL prefix to mount under (e.g. /data).
    #[arg(long, default_value = "")]
    base_path: String,
    /// Open the URL in the default browser on start.
    #[arg(long = "open")]
    open_browser: bool,
    /// Maximum file size to serve in MB (default: 250, 0 = unlimited).
    #[arg(long, default_value_t = 250)]
    max_file_size: u64,
    /// Enable gzip compression. Use --no-compress-types to skip specific MIME types.
    #[arg(long)]
    compress: bool,
    /// MIME type prefixes to skip compression for (repeatable).
    /// Default when --compress is on: application/octet-stream, image/, video/, audio/,
    /// application/zip, application/gzip, application/x-tar, font/.
    #[arg(long = "no-compress-type")]
    no_compress_types: Vec<String>,
    /// Blob cache size (number of objects). 0 to disable.
    #[arg(long, default_value_t = 4096)]
    cache_size: usize,
    /// Upstream server URL for blob redirect on cache miss.
    /// When a blob is not found locally, redirects to this server.
    #[arg(long)]
    upstream: Option<String>,
}

// ---------------------------------------------------------------------------
// Blob cache (lock-free, bounded, zero-copy via Bytes)
// ---------------------------------------------------------------------------

struct BlobCache {
    map: DashMap<String, Bytes>,
    count: AtomicUsize,
    capacity: usize,
}

impl BlobCache {
    fn new(capacity: usize) -> Self {
        Self {
            map: DashMap::with_capacity(capacity.min(1024)),
            count: AtomicUsize::new(0),
            capacity,
        }
    }

    fn get(&self, key: &str) -> Option<Bytes> {
        self.map.get(key).map(|v| v.value().clone()) // Bytes::clone is cheap (ref-count bump)
    }

    fn insert(&self, key: String, value: Bytes) {
        if self.capacity == 0 {
            return;
        }
        if self.map.contains_key(&key) {
            return;
        }
        // Evict if at capacity (random eviction — simple and fast under concurrent load)
        if self.count.load(Ordering::Relaxed) >= self.capacity {
            if let Some(entry) = self.map.iter().next() {
                let evict_key = entry.key().clone();
                drop(entry);
                if self.map.remove(&evict_key).is_some() {
                    self.count.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
        if self.map.insert(key, value).is_none() {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct AppState {
    store: GitStore,
    branch: Option<String>,
    ref_name: Option<String>,
    back: usize,
    at_path: Option<String>,
    match_pattern: Option<String>,
    before: Option<u64>,
    #[allow(dead_code)]
    all_refs: bool,
    no_cache: bool,
    immutable: bool,
    max_age: Option<u64>,
    base_path: String,
    max_file_size: u64,
    blob_cache: BlobCache,
    no_compress_types: Vec<String>,
    upstream: Option<String>,
}

impl AppState {
    fn default_branch(&self) -> String {
        if let Some(ref b) = self.branch {
            return b.clone();
        }
        self.store
            .branches()
            .get_current_name()
            .ok()
            .flatten()
            .unwrap_or_else(|| "main".to_string())
    }

    fn resolve_fs(&self, ref_name: &str) -> Option<vost::Fs> {
        self.store.fs(ref_name).ok()
    }

    fn resolve_single_ref_fs(&self) -> Option<vost::Fs> {
        let branch = self.default_branch();
        let ref_to_use = self.ref_name.as_deref().unwrap_or(&branch);
        let mut fs = self.resolve_fs(ref_to_use)?;
        fs = self.apply_snapshot_filters(fs).ok()?;
        Some(fs)
    }

    fn apply_snapshot_filters(&self, mut fs: vost::Fs) -> Result<vost::Fs, String> {
        if self.at_path.is_some() || self.match_pattern.is_some() || self.before.is_some() {
            let entries = fs
                .log(LogOptions {
                    path: self.at_path.clone(),
                    match_pattern: self.match_pattern.clone(),
                    before: self.before,
                    ..Default::default()
                })
                .map_err(|e| e.to_string())?;
            if entries.is_empty() {
                return Err("No matching commits found".to_string());
            }
            fs = fs
                .at_commit(&entries[0].commit_hash)
                .map_err(|e| e.to_string())?;
        }
        if self.back > 0 {
            fs = fs.back(self.back).map_err(|e| e.to_string())?;
        }
        Ok(fs)
    }

    fn ref_label(&self) -> String {
        self.ref_name
            .clone()
            .unwrap_or_else(|| self.default_branch())
    }

    fn max_file_bytes(&self) -> u64 {
        self.max_file_size * 1024 * 1024
    }

    /// Read a blob, using the cache if available.
    /// Key is the blob OID (content-addressable = perfect cache key).
    /// Returns Bytes (zero-copy on cache hit — just a ref-count bump).
    fn read_cached(&self, fs: &vost::Fs, path: &str, blob_hash: &str) -> Option<Bytes> {
        if let Some(data) = self.blob_cache.get(blob_hash) {
            return Some(data);
        }
        let data = fs.read(path).ok()?;
        let bytes = Bytes::from(data);
        self.blob_cache.insert(blob_hash.to_string(), bytes.clone());
        Some(bytes)
    }

    /// Compute the Cache-Control header value.
    fn cache_control(&self) -> String {
        if self.no_cache {
            "no-store".to_string()
        } else if self.immutable {
            "public, immutable, max-age=31536000".to_string()
        } else if let Some(max_age) = self.max_age {
            format!("public, max-age={}", max_age)
        } else {
            "no-cache".to_string()
        }
    }

    /// Check if a MIME type should skip compression.
    fn should_skip_compression(&self, mime: &str) -> bool {
        self.no_compress_types
            .iter()
            .any(|prefix| mime.starts_with(prefix.as_str()))
    }
}

// ---------------------------------------------------------------------------
// MIME type guessing
// ---------------------------------------------------------------------------

fn guess_mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" | "geojson" | "jsonl" | "ndjson" | "jsonc" | "json5" => {
            "text/plain; charset=utf-8"
        }
        "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "env" | "properties"
        | "editorconfig" => "text/plain; charset=utf-8",
        "txt" | "md" | "csv" | "tsv" | "log" | "rst" | "tex" | "bib" | "adoc" | "org"
        | "diff" | "patch" => "text/plain; charset=utf-8",
        "py" | "pyi" | "rs" | "go" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "hxx"
        | "cs" | "java" | "kt" | "kts" | "scala" | "clj" | "cljs" | "erl" | "ex" | "exs"
        | "hs" | "ml" | "mli" | "r" | "jl" | "lua" | "rb" | "pl" | "pm" | "php" | "swift"
        | "m" | "v" | "zig" | "nim" | "d" | "ada" | "pas" => "text/plain; charset=utf-8",
        "sh" | "bash" | "zsh" | "fish" | "csh" | "ksh" | "ps1" | "bat" | "cmd" => {
            "text/plain; charset=utf-8"
        }
        "ts" | "tsx" | "jsx" | "vue" | "svelte" | "astro" | "sass" | "scss" | "less"
        | "styl" | "pug" | "hbs" | "mustache" | "ejs" | "graphql" | "gql" | "proto" => {
            "text/plain; charset=utf-8"
        }
        "cmake" | "mk" | "makefile" | "nix" | "tf" | "hcl" | "dockerfile" | "gitignore"
        | "gitattributes" | "dockerignore" | "sql" | "dot" | "gv" => {
            "text/plain; charset=utf-8"
        }
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => {
            let basename = path.rsplit('/').next().unwrap_or(path);
            match basename {
                "Makefile" | "Dockerfile" | "Vagrantfile" | "Gemfile" | "Rakefile"
                | "Procfile" | "Brewfile" | "Justfile" | "CMakeLists.txt" | "OWNERS"
                | "CODEOWNERS" | "LICENSE" | "LICENCE" | "AUTHORS" | "CONTRIBUTORS" => {
                    "text/plain; charset=utf-8"
                }
                _ => "application/octet-stream",
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

fn make_href(base: &str, segments: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !base.is_empty() {
        parts.push(base.trim_matches('/').to_string());
    }
    for s in segments {
        if !s.is_empty() {
            parts.push(url_encode(s));
        }
    }
    let raw = parts.join("/");
    if raw.starts_with('/') {
        html_escape(&raw)
    } else {
        html_escape(&format!("/{}", raw))
    }
}

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map_or(false, |v| v.contains("application/json"))
}

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, msg.to_string()).into_response()
}

fn json_response(value: serde_json::Value, etag: &str, cache_control: &str) -> Response {
    let body = value.to_string();
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, cache_control);
    if !etag.is_empty() {
        builder = builder.header(header::ETAG, etag);
    }
    builder.body(Body::from(body)).unwrap()
}

fn file_response(
    data: Bytes,
    mime: &str,
    etag: &str,
    cache_control: &str,
    skip_compress: bool,
) -> Response {
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::CONTENT_LENGTH, data.len().to_string())
        .header(header::ACCEPT_RANGES, "bytes");

    if skip_compress {
        builder = builder.header(header::CONTENT_ENCODING, "identity");
    }

    builder.body(Body::from(data)).unwrap()
}

/// Respond with a 206 Partial Content for a Range request.
fn range_response(
    data: Bytes,
    mime: &str,
    etag: &str,
    cache_control: &str,
    start: u64,
    end: u64,
    total: u64,
    skip_compress: bool,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::CONTENT_LENGTH, data.len().to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, total),
        );

    if skip_compress {
        builder = builder.header(header::CONTENT_ENCODING, "identity");
    }

    builder.body(Body::from(data)).unwrap()
}

fn html_response(html: String, etag: &str, cache_control: &str) -> Response {
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, cache_control);
    if !etag.is_empty() {
        builder = builder.header(header::ETAG, etag);
    }
    builder.body(Body::from(html)).unwrap()
}

// ---------------------------------------------------------------------------
// Blob hash check
// ---------------------------------------------------------------------------

fn is_hex40(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn redirect_upstream(upstream: &str, path: &str) -> Response {
    let location = format!("{}/{}", upstream.trim_end_matches('/'), path.trim_start_matches('/'));
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap()
}

fn serve_blob_response(state: &AppState, hash: &str, headers: &HeaderMap) -> Response {
    let etag = format!("\"{}\"", hash);
    let cc = state.cache_control();
    let want_json = wants_json(headers);

    // 304
    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        if inm.to_str().ok() == Some(&etag) {
            return Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag)
                .body(Body::empty())
                .unwrap();
        }
    }

    // Check cache first, then read from git via store.read_by_hash (no Fs needed)
    let data: Bytes = if let Some(cached) = state.blob_cache.get(hash) {
        cached
    } else {
        match state.store.read_by_hash(hash, 0, None) {
            Ok(d) => {
                let bytes = Bytes::from(d);
                state.blob_cache.insert(hash.to_string(), bytes.clone());
                bytes
            }
            Err(_) => {
                if let Some(ref upstream) = state.upstream {
                    return redirect_upstream(upstream, &format!("_/blobs/{}", hash));
                }
                return not_found(&format!("Blob not found: {}", hash));
            }
        }
    };

    if want_json {
        return json_response(
            serde_json::json!({
                "hash": hash,
                "size": data.len(),
                "type": "blob",
            }),
            &etag,
            &cc,
        );
    }

    let total = data.len() as u64;
    if let Some((start, end)) = parse_range(headers, total) {
        let slice = data.slice(start as usize..(end + 1) as usize);
        return range_response(slice, "application/octet-stream", &etag, &cc, start, end, total, true);
    }

    file_response(data, "application/octet-stream", &etag, &cc, true)
}

// ---------------------------------------------------------------------------
// Serving logic
// ---------------------------------------------------------------------------

fn serve_dir_listing(
    state: &AppState,
    fs: &vost::Fs,
    ref_label: &str,
    link_prefix: &str,
    path: &str,
    etag: &str,
    want_json: bool,
) -> Response {
    let cc = state.cache_control();
    let path = path.trim_matches('/');
    let entries = match fs.ls(path) {
        Ok(e) => e,
        Err(_) => return not_found(&format!("Not found: {}", path)),
    };

    let mut sorted: Vec<&String> = entries.iter().collect();
    sorted.sort();

    let classified: Vec<(&String, bool)> = sorted
        .iter()
        .map(|name| {
            let entry_path = if path.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", path, name)
            };
            let is_dir = fs.is_dir(&entry_path).unwrap_or(false);
            (*name, is_dir)
        })
        .collect();

    if want_json {
        let json_entries: Vec<String> = classified
            .iter()
            .map(|(name, is_dir)| {
                if *is_dir {
                    format!("{}/", name)
                } else {
                    name.to_string()
                }
            })
            .collect();
        return json_response(
            serde_json::json!({
                "path": path,
                "ref": ref_label,
                "entries": json_entries,
                "type": "directory",
            }),
            etag,
            &cc,
        );
    }

    let display_path = if path.is_empty() { "/" } else { path };
    let mut html = format!(
        "<html><body><h1>{}</h1><ul>",
        html_escape(display_path)
    );
    for (entry, is_dir) in &classified {
        let h = if path.is_empty() {
            make_href(link_prefix, &[entry])
        } else {
            make_href(link_prefix, &[path, entry])
        };
        let suffix = if *is_dir { "/" } else { "" };
        let href_suffix = if *is_dir { "/" } else { "" };
        html.push_str(&format!(
            "<li><a href=\"{}{}\">{}{}</a></li>",
            h, href_suffix,
            html_escape(entry),
            suffix,
        ));
    }
    html.push_str("</ul></body></html>");
    html_response(html, etag, &cc)
}

/// Parse an HTTP Range header value like "bytes=0-99" into (start, end).
/// Returns None if the header is missing or malformed.
fn parse_range(headers: &HeaderMap, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let range_val = headers.get(header::RANGE)?.to_str().ok()?;
    let range_val = range_val.strip_prefix("bytes=")?;

    if let Some((start_s, end_s)) = range_val.split_once('-') {
        if start_s.is_empty() {
            // Suffix range: bytes=-N → last N bytes
            let suffix: u64 = end_s.parse().ok()?;
            let start = total.saturating_sub(suffix);
            Some((start, total - 1))
        } else {
            let start: u64 = start_s.parse().ok()?;
            let end = if end_s.is_empty() {
                total - 1
            } else {
                end_s.parse::<u64>().ok()?.min(total - 1)
            };
            if start > end || start >= total {
                None
            } else {
                Some((start, end))
            }
        }
    } else {
        None
    }
}

fn serve_file_content(
    state: &AppState,
    fs: &vost::Fs,
    ref_label: &str,
    path: &str,
    want_json: bool,
    headers: &HeaderMap,
) -> Response {
    let cc = state.cache_control();

    // stat() gives us blob hash + size in one call
    let st = match fs.stat(path) {
        Ok(st) => st,
        Err(_) => return not_found(&format!("Not found: {}", path)),
    };

    let max = state.max_file_bytes();
    if max > 0 && st.size > max {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "File too large: {} ({} bytes, limit {} bytes)",
                path, st.size, max
            ),
        )
            .into_response();
    }

    // Per-blob ETag: survives commits to other files
    let etag = format!("\"{}\"", st.hash);

    if want_json {
        return json_response(
            serde_json::json!({
                "path": path,
                "ref": ref_label,
                "size": st.size,
                "hash": st.hash,
                "type": "file",
            }),
            &etag,
            &cc,
        );
    }

    // Read with blob cache
    let data = match state.read_cached(fs, path, &st.hash) {
        Some(d) => d,
        None => return not_found(&format!("Not found: {}", path)),
    };

    let mime = guess_mime(path);
    let skip_compress = state.should_skip_compression(mime);
    let total = data.len() as u64;

    // Handle Range requests
    if let Some((start, end)) = parse_range(headers, total) {
        let start_usize = start as usize;
        let end_usize = (end + 1) as usize;
        let slice = data.slice(start_usize..end_usize.min(data.len()));
        return range_response(slice, mime, &etag, &cc, start, end, total, skip_compress);
    }

    file_response(data, mime, &etag, &cc, skip_compress)
}

fn serve_path(
    state: &AppState,
    fs: &vost::Fs,
    ref_label: &str,
    link_prefix: &str,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    let want_json = wants_json(headers);

    // For directories, use commit-level ETag
    if path.is_empty() {
        let etag = format!("\"{}\"", fs.commit_hash().unwrap_or_default());
        if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
            if inm.to_str().ok() == Some(&etag) {
                return Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(header::ETAG, &etag)
                    .body(Body::empty())
                    .unwrap();
            }
        }
        return serve_dir_listing(state, fs, ref_label, link_prefix, "", &etag, want_json);
    }

    if !fs.exists(path).unwrap_or(false) {
        return not_found(&format!("Not found: {}", path));
    }

    if fs.is_dir(path).unwrap_or(false) {
        let etag = format!("\"{}\"", fs.commit_hash().unwrap_or_default());
        if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
            if inm.to_str().ok() == Some(&etag) {
                return Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(header::ETAG, &etag)
                    .body(Body::empty())
                    .unwrap();
            }
        }
        serve_dir_listing(state, fs, ref_label, link_prefix, path, &etag, want_json)
    } else {
        // For files: use per-blob ETag — check 304 against blob hash
        if let Ok(st) = fs.stat(path) {
            let etag = format!("\"{}\"", st.hash);
            if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
                if inm.to_str().ok() == Some(&etag) {
                    return Response::builder()
                        .status(StatusCode::NOT_MODIFIED)
                        .header(header::ETAG, &etag)
                        .body(Body::empty())
                        .unwrap();
                }
            }
        }
        serve_file_content(state, fs, ref_label, path, want_json, headers)
    }
}

// ---------------------------------------------------------------------------
// Axum handlers — all use spawn_blocking to avoid blocking the async runtime
// ---------------------------------------------------------------------------

async fn handle_single_ref(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    path: Option<Path<String>>,
) -> Response {
    let repo_path = path
        .map(|Path(p)| p.trim_matches('/').to_string())
        .unwrap_or_default();
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let ref_label = state.ref_label();
        let fs = match state.resolve_single_ref_fs() {
            Some(fs) => fs,
            None => return not_found(&format!("Ref not found: {}", ref_label)),
        };

        serve_path(&state, &fs, &ref_label, &state.base_path, &repo_path, &headers)
    })
    .await
    .unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
    })
}

async fn handle_ref_listing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let branches = state.store.branches().list().unwrap_or_default();
        let tags = state.store.tags().list().unwrap_or_default();

        if wants_json(&headers) {
            return json_response(
                serde_json::json!({ "branches": branches, "tags": tags }),
                "",
                &state.cache_control(),
            );
        }

        let mut html = String::from("<html><body><h1>Branches</h1><ul>");
        for b in &branches {
            html.push_str(&format!(
                "<li><a href=\"{}/\">{}</a></li>",
                make_href(&state.base_path, &[b]),
                html_escape(b)
            ));
        }
        html.push_str("</ul><h1>Tags</h1><ul>");
        for t in &tags {
            html.push_str(&format!(
                "<li><a href=\"{}/\">{}</a></li>",
                make_href(&state.base_path, &[t]),
                html_escape(t)
            ));
        }
        html.push_str("</ul></body></html>");
        Html(html).into_response()
    })
    .await
    .unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
    })
}

async fn handle_multi_ref(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(params): Path<(String, String)>,
) -> Response {
    let (ref_name, repo_path) = params;
    let repo_path = repo_path.trim_matches('/').to_string();
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let fs = match state.resolve_fs(&ref_name) {
            Some(fs) => fs,
            None => return not_found(&format!("Unknown ref: {}", ref_name)),
        };
        let link_prefix = format!("{}/{}", state.base_path, ref_name);
        serve_path(&state, &fs, &ref_name, &link_prefix, &repo_path, &headers)
    })
    .await
    .unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
    })
}

async fn handle_multi_ref_root(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(ref_name): Path<String>,
) -> Response {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let fs = match state.resolve_fs(&ref_name) {
            Some(fs) => fs,
            None => return not_found(&format!("Unknown ref: {}", ref_name)),
        };
        let link_prefix = format!("{}/{}", state.base_path, ref_name);
        serve_path(&state, &fs, &ref_name, &link_prefix, "", &headers)
    })
    .await
    .unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
    })
}

async fn handle_blob(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Response {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        if !is_hex40(&hash) {
            return not_found(&format!("Invalid blob hash: {}", hash));
        }

        serve_blob_response(&state, &hash, &headers)
    })
    .await
    .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let store = GitStore::open(
        &args.repo,
        vost::OpenOptions {
            create: false,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| {
        eprintln!("Error: failed to open repository: {}", e);
        std::process::exit(1);
    });

    let base_path = if args.base_path.is_empty() {
        String::new()
    } else {
        format!("/{}", args.base_path.trim_matches('/'))
    };

    if args.all_refs
        && (args.ref_name.is_some()
            || args.at_path.is_some()
            || args.match_pattern.is_some()
            || args.before.is_some()
            || args.back > 0)
    {
        eprintln!(
            "Error: --all cannot be combined with --ref, --path, --match, --before, or --back"
        );
        std::process::exit(1);
    }

    let before_epoch = args
        .before
        .as_deref()
        .map(parse_before)
        .transpose()
        .unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });

    // Default no-compress types: already-compressed formats
    let no_compress_types = if args.compress && args.no_compress_types.is_empty() {
        vec![
            "application/octet-stream".to_string(),
            "image/".to_string(),
            "video/".to_string(),
            "audio/".to_string(),
            "application/zip".to_string(),
            "application/gzip".to_string(),
            "application/x-tar".to_string(),
            "font/".to_string(),
        ]
    } else {
        args.no_compress_types.clone()
    };

    let state = Arc::new(AppState {
        store,
        branch: args.branch.clone(),
        ref_name: args.ref_name.clone(),
        back: args.back,
        at_path: args.at_path.clone(),
        match_pattern: args.match_pattern.clone(),
        before: before_epoch,
        all_refs: args.all_refs,
        no_cache: args.no_cache,
        immutable: args.immutable,
        max_age: args.max_age,
        base_path: base_path.clone(),
        max_file_size: args.max_file_size,
        blob_cache: BlobCache::new(args.cache_size),
        no_compress_types,
        upstream: args.upstream.clone(),
    });

    // Note: axum automatically handles HEAD requests on GET routes
    // by running the handler and stripping the response body.
    let app = if args.all_refs {
        let mut router = Router::new()
            .route("/", get(handle_ref_listing))
            .route("/_/blobs/{hash}", get(handle_blob))
            .route("/{ref_name}", get(handle_multi_ref_root))
            .route("/{ref_name}/", get(handle_multi_ref_root))
            .route("/{ref_name}/{*path}", get(handle_multi_ref));
        if !base_path.is_empty() {
            router = Router::new().nest(&base_path, router);
        }
        router.with_state(state.clone())
    } else {
        let mut router = Router::new()
            .route("/", get(handle_single_ref))
            .route("/_/blobs/{hash}", get(handle_blob))
            .route("/{*path}", get(handle_single_ref));
        if !base_path.is_empty() {
            router = Router::new().nest(&base_path, router);
        }
        router.with_state(state.clone())
    };

    let app = if args.cors {
        app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .expose_headers(Any),
        )
    } else {
        app
    };

    let app = if args.compress {
        use tower_http::compression::CompressionLayer;
        app.layer(CompressionLayer::new().gzip(true))
    } else {
        app
    };

    let addr = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to bind {}: {}", addr, e);
            std::process::exit(1);
        });

    let actual_addr = listener.local_addr().unwrap();
    let url = format!("http://{}{}/", actual_addr, base_path);
    let mode = if args.all_refs {
        "multi-ref".to_string()
    } else {
        let mut m = format!("{} (live)", state.ref_label());
        if args.back > 0 {
            m.push_str(&format!(" ~{}", args.back));
        }
        if args.at_path.is_some() {
            m.push_str(&format!(" --path {}", args.at_path.as_deref().unwrap()));
        }
        if args.match_pattern.is_some() {
            m.push_str(&format!(
                " --match {}",
                args.match_pattern.as_deref().unwrap()
            ));
        }
        if args.before.is_some() {
            m.push_str(&format!(" --before {}", args.before.as_deref().unwrap()));
        }
        m
    };
    eprintln!("Serving {} ({}) at {}", args.repo, mode, url);
    if args.cache_size > 0 {
        eprintln!("Blob cache: {} entries", args.cache_size);
    }
    eprintln!("Press Ctrl+C to stop.");

    if args.open_browser {
        let _ = open_url(&url);
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    eprintln!("Stopped.");
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_url(_url: &str) -> std::io::Result<()> {
    Ok(())
}
