use clap::Args;
use std::io::Write;

use crate::store::GitStore;

use super::error::CliError;
use super::helpers::*;

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Bind address.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Port to listen on (0 for OS-assigned).
    #[arg(short, long, default_value_t = 8000)]
    pub port: u16,
    /// Branch.
    #[arg(short, long)]
    pub branch: Option<String>,
    #[command(flatten)]
    pub snap: SnapshotArgs,
    /// Multi-ref mode: expose all branches and tags via /<ref>/<path>.
    #[arg(long = "all")]
    pub all_refs: bool,
    /// Enable CORS headers (Access-Control-Allow-Origin: *).
    #[arg(long)]
    pub cors: bool,
    /// Send Cache-Control: no-store on every response.
    #[arg(long)]
    pub no_cache: bool,
    /// Set Cache-Control: immutable, max-age=31536000.
    #[arg(long)]
    pub immutable: bool,
    /// Set Cache-Control: max-age=N (seconds). Overridden by --no-cache or --immutable.
    #[arg(long)]
    pub max_age: Option<u64>,
    /// URL prefix to mount under (e.g. /data).
    #[arg(long, default_value = "")]
    pub base_path: String,
    /// Open the URL in the default browser on start.
    #[arg(long = "open")]
    pub open_browser: bool,
    /// Suppress access log on stderr (errors still logged; --log-file still writes).
    #[arg(short, long)]
    pub quiet: bool,
    /// Append access log to file (CLF format).
    #[arg(long)]
    pub log_file: Option<String>,
    /// Maximum file size to serve in MB (default: 250, 0 = unlimited).
    #[arg(long, default_value_t = 250)]
    pub max_file_size: u64,
}

// ---------------------------------------------------------------------------
// MIME type guessing
// ---------------------------------------------------------------------------

fn guess_mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        // Markup
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        // Text/data (browser-friendly: display, don't download)
        "json" | "geojson" | "jsonl" | "ndjson" | "jsonc" | "json5"
            => "text/plain; charset=utf-8",
        "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "env"
        | "properties" | "editorconfig"
            => "text/plain; charset=utf-8",
        "txt" | "md" | "csv" | "tsv" | "log" | "rst" | "tex" | "bib"
        | "adoc" | "org" | "diff" | "patch"
            => "text/plain; charset=utf-8",
        // Programming languages
        "py" | "pyi" | "rs" | "go" | "c" | "h" | "cpp" | "hpp" | "cc"
        | "cxx" | "hxx" | "cs" | "java" | "kt" | "kts" | "scala"
        | "clj" | "cljs" | "erl" | "ex" | "exs" | "hs" | "ml" | "mli"
        | "r" | "jl" | "lua" | "rb" | "pl" | "pm" | "php" | "swift"
        | "m" | "v" | "zig" | "nim" | "d" | "ada" | "pas"
            => "text/plain; charset=utf-8",
        // Shell / scripting
        "sh" | "bash" | "zsh" | "fish" | "csh" | "ksh"
        | "ps1" | "bat" | "cmd"
            => "text/plain; charset=utf-8",
        // Web
        "ts" | "tsx" | "jsx" | "vue" | "svelte" | "astro"
        | "sass" | "scss" | "less" | "styl"
        | "pug" | "hbs" | "mustache" | "ejs"
        | "graphql" | "gql" | "proto"
            => "text/plain; charset=utf-8",
        // Build / config
        "cmake" | "mk" | "makefile" | "nix" | "tf" | "hcl"
        | "dockerfile" | "gitignore" | "gitattributes" | "dockerignore"
        | "sql" | "dot" | "gv"
            => "text/plain; charset=utf-8",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        // Binary
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
            // Check for well-known extensionless filenames
            let basename = path.rsplit('/').next().unwrap_or(path);
            match basename {
                "Makefile" | "Dockerfile" | "Vagrantfile" | "Gemfile"
                | "Rakefile" | "Procfile" | "Brewfile" | "Justfile"
                | "CMakeLists.txt" | "OWNERS" | "CODEOWNERS"
                | "LICENSE" | "LICENCE" | "AUTHORS" | "CONTRIBUTORS"
                    => "text/plain; charset=utf-8",
                _ => "application/octet-stream",
            }
        }
    }
}

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

fn href(base: &str, segments: &[&str]) -> String {
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
// Response helpers
// ---------------------------------------------------------------------------

fn respond(
    request: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) {
    respond_owned(request, status, content_type, body.to_vec(), extra_headers);
}

fn respond_owned(
    request: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    extra_headers: &[(&str, &str)],
) {
    let mut headers: Vec<tiny_http::Header> = vec![
        tiny_http::Header::from_bytes("Content-Type", content_type).unwrap(),
    ];
    for (k, v) in extra_headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            headers.push(h);
        }
    }
    let len = body.len();
    let status_code = tiny_http::StatusCode(status);
    let response = tiny_http::Response::new(
        status_code,
        headers,
        std::io::Cursor::new(body),
        Some(len),
        None,
    );
    let _ = request.respond(response);
}

fn respond_304(request: tiny_http::Request, etag: &str) {
    let headers = vec![
        tiny_http::Header::from_bytes("ETag", etag).unwrap(),
    ];
    let response = tiny_http::Response::new(
        tiny_http::StatusCode(304),
        headers,
        std::io::Cursor::new(Vec::new()),
        Some(0),
        None,
    );
    let _ = request.respond(response);
}

fn respond_404(request: tiny_http::Request, message: &str) {
    respond(request, 404, "text/plain", message.as_bytes(), &[]);
}

// ---------------------------------------------------------------------------
// Access logging (CLF)
// ---------------------------------------------------------------------------

struct AccessLogger {
    quiet: bool,
    file: Option<std::fs::File>,
}

impl AccessLogger {
    fn new(quiet: bool, log_file: Option<&str>) -> std::io::Result<Self> {
        let file = match log_file {
            Some(p) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)?,
            ),
            None => None,
        };
        Ok(Self { quiet, file })
    }

    fn log(&mut self, client_ip: &str, method: &str, path: &str, status: u16, size: usize) {
        let now = chrono::Utc::now().format("%d/%b/%Y:%H:%M:%S %z");
        let line = format!(
            "{} - - [{}] \"{} {} HTTP/1.1\" {} {}\n",
            client_ip, now, method, path, status, size
        );
        if !self.quiet {
            eprint!("{}", line);
        }
        if let Some(ref mut f) = self.file {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}

struct ResponseInfo {
    status: u16,
    size: usize,
}

fn respond_tracked(
    request: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> ResponseInfo {
    let size = body.len();
    respond(request, status, content_type, body, extra_headers);
    ResponseInfo { status, size }
}

fn respond_owned_tracked(
    request: tiny_http::Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    extra_headers: &[(&str, &str)],
) -> ResponseInfo {
    let size = body.len();
    respond_owned(request, status, content_type, body, extra_headers);
    ResponseInfo { status, size }
}

fn respond_304_tracked(request: tiny_http::Request, etag: &str) -> ResponseInfo {
    respond_304(request, etag);
    ResponseInfo { status: 304, size: 0 }
}

fn respond_404_tracked(request: tiny_http::Request, message: &str) -> ResponseInfo {
    let size = message.len();
    respond_404(request, message);
    ResponseInfo { status: 404, size }
}

// ---------------------------------------------------------------------------
// Serving logic
// ---------------------------------------------------------------------------

fn serve_file(
    request: tiny_http::Request,
    fs: &crate::Fs,
    path: &str,
    want_json: bool,
    cors: bool,
    cache_control: &str,
    ref_label: &str,
    max_file_size: u64,
) -> ResponseInfo {
    let st = match fs.stat(path) {
        Ok(st) => st,
        Err(_) => return respond_404_tracked(request, &format!("Not found: {}", path)),
    };

    if max_file_size > 0 && st.size > max_file_size {
        let msg = format!(
            "File too large: {} ({} bytes, limit {} bytes)",
            path, st.size, max_file_size
        );
        return respond_tracked(request, 413, "text/plain", msg.as_bytes(), &[]);
    }

    let etag = format!("\"{}\"", st.hash);

    let data = match fs.read(path) {
        Ok(d) => d,
        Err(_) => return respond_404_tracked(request, &format!("Not found: {}", path)),
    };

    let mut headers: Vec<(&str, &str)> = vec![
        ("ETag", &etag),
        ("Cache-Control", cache_control),
        ("Accept-Ranges", "bytes"),
    ];
    if cors {
        headers.push(("Access-Control-Allow-Origin", "*"));
    }

    // Range request support
    let range_header = request.headers().iter()
        .find(|h| h.field.as_str() == "Range" || h.field.as_str() == "range")
        .map(|h| h.value.as_str().to_string());

    if let Some(ref range_val) = range_header {
        if let Some(range) = parse_range(range_val, data.len() as u64) {
            let (start, end) = range;
            let slice = &data[start as usize..(end + 1) as usize];
            let content_range = format!("bytes {}-{}/{}", start, end, data.len());
            let mime = guess_mime(path);
            let mut range_headers: Vec<(&str, &str)> = vec![
                ("ETag", &etag),
                ("Cache-Control", cache_control),
                ("Accept-Ranges", "bytes"),
                ("Content-Range", &content_range),
            ];
            if cors {
                range_headers.push(("Access-Control-Allow-Origin", "*"));
            }
            return respond_tracked(request, 206, mime, slice, &range_headers);
        }
    }

    if want_json {
        let json = serde_json::json!({
            "path": path,
            "ref": ref_label,
            "size": data.len(),
            "type": "file",
        });
        let body = json.to_string();
        respond_tracked(request, 200, "application/json", body.as_bytes(), &headers)
    } else {
        let mime = guess_mime(path);
        respond_owned_tracked(request, 200, mime, data, &headers)
    }
}

fn serve_dir(
    request: tiny_http::Request,
    fs: &crate::Fs,
    ref_label: &str,
    link_prefix: &str,
    path: &str,
    etag: &str,
    want_json: bool,
    cors: bool,
    cache_control: &str,
) -> ResponseInfo {
    let entries = match fs.ls(path) {
        Ok(e) => e,
        Err(_) => return respond_404_tracked(request, &format!("Not found: {}", path)),
    };

    let mut headers: Vec<(&str, &str)> = vec![
        ("ETag", etag),
        ("Cache-Control", cache_control),
    ];
    if cors {
        headers.push(("Access-Control-Allow-Origin", "*"));
    }

    let mut sorted: Vec<&String> = entries.iter().collect();
    sorted.sort();

    // Classify entries as files or directories
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
        let json = serde_json::json!({
            "path": path,
            "ref": ref_label,
            "entries": json_entries,
            "type": "directory",
        });
        let body = json.to_string();
        respond_tracked(request, 200, "application/json", body.as_bytes(), &headers)
    } else {
        let display_path = if path.is_empty() { "/" } else { path };
        let mut html = format!(
            "<html><body><h1>{}</h1><ul>",
            html_escape(display_path)
        );
        for (entry, is_dir) in &classified {
            let h = if path.is_empty() {
                href(link_prefix, &[entry])
            } else {
                href(link_prefix, &[path, entry])
            };
            let suffix = if *is_dir { "/" } else { "" };
            let href_suffix = if *is_dir { "/" } else { "" };
            html.push_str(&format!(
                "<li><a href=\"{}{}\">{}{}</a></li>",
                h,
                href_suffix,
                html_escape(entry),
                suffix,
            ));
        }
        html.push_str("</ul></body></html>");
        respond_tracked(
            request,
            200,
            "text/html; charset=utf-8",
            html.as_bytes(),
            &headers,
        )
    }
}

fn serve_ref_listing(
    request: tiny_http::Request,
    store: &GitStore,
    base_path: &str,
    want_json: bool,
    cors: bool,
) -> ResponseInfo {
    let branches = store.branches().list().unwrap_or_default();
    let tags = store.tags().list().unwrap_or_default();

    let mut headers: Vec<(&str, &str)> = Vec::new();
    if cors {
        headers.push(("Access-Control-Allow-Origin", "*"));
    }

    if want_json {
        let json = serde_json::json!({
            "branches": branches,
            "tags": tags,
        });
        let body = json.to_string();
        respond_tracked(request, 200, "application/json", body.as_bytes(), &headers)
    } else {
        let mut html = String::from("<html><body><h1>Branches</h1><ul>");
        for b in &branches {
            html.push_str(&format!(
                "<li><a href=\"{}/\">{}</a></li>",
                href(base_path, &[b]),
                html_escape(b)
            ));
        }
        html.push_str("</ul><h1>Tags</h1><ul>");
        for t in &tags {
            html.push_str(&format!(
                "<li><a href=\"{}/\">{}</a></li>",
                href(base_path, &[t]),
                html_escape(t)
            ));
        }
        html.push_str("</ul></body></html>");
        respond_tracked(
            request,
            200,
            "text/html; charset=utf-8",
            html.as_bytes(),
            &headers,
        )
    }
}

fn serve_path(
    request: tiny_http::Request,
    fs: &crate::Fs,
    ref_label: &str,
    link_prefix: &str,
    path: &str,
    cors: bool,
    cache_control: &str,
    max_file_size: u64,
) -> ResponseInfo {
    let etag = format!(
        "\"{}\"",
        fs.commit_hash().unwrap_or_default()
    );

    // Check If-None-Match for directories (commit-level etag)
    let if_none_match: Option<String> = request
        .headers()
        .iter()
        .find(|h| h.field.as_str() == "If-None-Match" || h.field.as_str() == "if-none-match")
        .map(|h| h.value.as_str().to_string());

    let accept = request
        .headers()
        .iter()
        .find(|h| h.field.as_str() == "Accept" || h.field.as_str() == "accept")
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let want_json = accept.contains("application/json");

    if path.is_empty() {
        if let Some(ref inm) = if_none_match {
            if inm == &etag {
                return respond_304_tracked(request, &etag);
            }
        }
        return serve_dir(
            request, fs, ref_label, link_prefix, "", &etag, want_json, cors, cache_control,
        );
    }

    if !fs.exists(path).unwrap_or(false) {
        return respond_404_tracked(request, &format!("Not found: {}", path));
    }

    if fs.is_dir(path).unwrap_or(false) {
        if let Some(ref inm) = if_none_match {
            if inm == &etag {
                return respond_304_tracked(request, &etag);
            }
        }
        serve_dir(
            request, fs, ref_label, link_prefix, path, &etag, want_json, cors, cache_control,
        )
    } else {
        // Per-blob ETag for files
        if let Ok(st) = fs.stat(path) {
            let blob_etag = format!("\"{}\"", st.hash);
            if let Some(ref inm) = if_none_match {
                if inm == &blob_etag {
                    return respond_304_tracked(request, &blob_etag);
                }
            }
        }
        serve_file(request, fs, path, want_json, cors, cache_control, ref_label, max_file_size)
    }
}

// ---------------------------------------------------------------------------
// Main command
// ---------------------------------------------------------------------------

pub fn cmd_serve(repo_path: &str, args: &ServeArgs, _verbose: bool) -> Result<(), CliError> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp(None)
        .init();

    let store = open_store(repo_path)?;

    if args.all_refs
        && (args.snap.ref_name.is_some()
            || args.snap.at_path.is_some()
            || args.snap.match_pattern.is_some()
            || args.snap.before.is_some()
            || args.snap.back > 0)
    {
        return Err(CliError::new(
            "--all cannot be combined with --ref, --path, --match, --before, or --back",
        ));
    }

    let base_path = if args.base_path.is_empty() {
        String::new()
    } else {
        let trimmed = args.base_path.trim_matches('/');
        format!("/{}", trimmed)
    };

    // Determine mode
    let mode_label: String;
    let branch: String;
    let ref_label: String;

    if args.all_refs {
        mode_label = "multi-ref".to_string();
        branch = String::new();
        ref_label = String::new();
    } else {
        branch = args
            .branch
            .clone()
            .unwrap_or_else(|| current_branch(&store));
        ref_label = args
            .snap
            .ref_name
            .clone()
            .unwrap_or_else(|| branch.clone());
        mode_label = format!("branch {} (live)", branch);
    }

    let max_file_bytes = if args.max_file_size > 0 {
        args.max_file_size * 1024 * 1024
    } else {
        0
    };

    let cache_control = if args.no_cache {
        "no-store".to_string()
    } else if args.immutable {
        "public, immutable, max-age=31536000".to_string()
    } else if let Some(max_age) = args.max_age {
        format!("public, max-age={}", max_age)
    } else {
        "no-cache".to_string()
    };

    let mut access_logger = AccessLogger::new(args.quiet, args.log_file.as_deref())
        .map_err(|e| CliError::new(format!("Failed to open log file: {}", e)))?;

    let addr = format!("{}:{}", args.host, args.port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| CliError::new(format!("Failed to bind {}: {}", addr, e)))?;

    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(args.port);
    let url = format!("http://{}:{}{}/", args.host, actual_port, base_path);
    log::info!("Serving {} ({}) at {}", repo_path, mode_label, url);
    log::info!("Press Ctrl+C to stop.");

    if args.open_browser {
        let _ = open_url(&url);
    }

    loop {
        let request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };

        let client_ip = request
            .remote_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "-".to_string());
        let method = request.method().as_str().to_string();
        let url_path = request.url().to_string();

        // Handle CORS preflight
        if args.cors && method == "OPTIONS" {
            let info = respond_tracked(
                request,
                204,
                "text/plain",
                b"",
                &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS"),
                    ("Access-Control-Allow-Headers", "Accept, If-None-Match"),
                    ("Access-Control-Expose-Headers", "ETag, Content-Length"),
                ],
            );
            access_logger.log(&client_ip, &method, &url_path, info.status, info.size);
            continue;
        }

        // Strip base path
        let raw_path = url_path.clone();
        let path = if !base_path.is_empty() {
            if let Some(rest) = raw_path.strip_prefix(&base_path) {
                rest.to_string()
            } else {
                let info = respond_404_tracked(request, "Not found");
                access_logger.log(&client_ip, &method, &url_path, info.status, info.size);
                continue;
            }
        } else {
            raw_path
        };

        let path = path.trim_matches('/').to_string();
        // Percent-decode
        let path = percent_decode(&path);

        let info = if args.all_refs {
            // Multi-ref mode
            if path.is_empty() {
                let want_json = request
                    .headers()
                    .iter()
                    .any(|h| (h.field.as_str() == "Accept" || h.field.as_str() == "accept")
                        && h.value.as_str().contains("application/json"));
                serve_ref_listing(request, &store, &base_path, want_json, args.cors)
            } else {
                let (ref_name, rest) = match path.find('/') {
                    Some(i) => (&path[..i], &path[i + 1..]),
                    None => (path.as_str(), ""),
                };

                match store.fs(ref_name) {
                    Ok(fs) => {
                        let link_pfx = format!("{}/{}", base_path, ref_name);
                        serve_path(request, &fs, ref_name, &link_pfx, rest, args.cors, &cache_control, max_file_bytes)
                    }
                    Err(_) => {
                        respond_404_tracked(request, &format!("Unknown ref: {}", ref_name))
                    }
                }
            }
        } else {
            // Single-ref mode: resolve fresh FS each request (live)
            match resolve_fs(&store, &branch, &args.snap) {
                Ok(fs) => {
                    serve_path(
                        request,
                        &fs,
                        &ref_label,
                        &base_path,
                        &path,
                        args.cors,
                        &cache_control,
                        max_file_bytes,
                    )
                }
                Err(e) => {
                    respond_404_tracked(request, &e.message)
                }
            }
        };

        access_logger.log(&client_ip, &method, &url_path, info.status, info.size);
    }

    log::info!("Stopped.");
    Ok(())
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?;
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
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
        if start > end || start >= total { None } else { Some((start, end)) }
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(url).spawn().map(|_| ())
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd").args(["/c", "start", url]).spawn().map(|_| ())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_url(_url: &str) -> std::io::Result<()> {
    Ok(())
}
