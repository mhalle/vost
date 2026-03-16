use clap::Args;

use crate::store::GitStore;
use crate::types::FileType;

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
    /// URL prefix to mount under (e.g. /data).
    #[arg(long, default_value = "")]
    pub base_path: String,
    /// Open the URL in the default browser on start.
    #[arg(long = "open")]
    pub open_browser: bool,
    /// Suppress per-request log output.
    #[arg(short, long)]
    pub quiet: bool,
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
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "text/plain; charset=utf-8",      // browser-friendly override
        "geojson" => "text/plain; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "yaml" | "yml" => "text/plain; charset=utf-8",
        "txt" | "md" | "csv" | "tsv" | "log" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
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
        _ => "application/octet-stream",
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
// Serving logic
// ---------------------------------------------------------------------------

fn serve_file(
    request: tiny_http::Request,
    fs: &crate::Fs,
    path: &str,
    etag: &str,
    want_json: bool,
    cors: bool,
    no_cache: bool,
    ref_label: &str,
    max_file_size: u64,
) {
    // Check size before reading
    if max_file_size > 0 {
        if let Ok(file_size) = fs.size(path) {
            if file_size > max_file_size {
                let msg = format!(
                    "File too large: {} ({} bytes, limit {} bytes)",
                    path, file_size, max_file_size
                );
                return respond(request, 413, "text/plain", msg.as_bytes(), &[]);
            }
        }
    }

    let data = match fs.read(path) {
        Ok(d) => d,
        Err(_) => return respond_404(request, &format!("Not found: {}", path)),
    };

    let mut headers: Vec<(&str, &str)> = vec![
        ("ETag", etag),
        ("Cache-Control", "no-cache"),
    ];
    if cors {
        headers.push(("Access-Control-Allow-Origin", "*"));
    }
    if no_cache {
        headers.push(("Cache-Control", "no-store"));
    }

    if want_json {
        let json = serde_json::json!({
            "path": path,
            "ref": ref_label,
            "size": data.len(),
            "type": "file",
        });
        let body = json.to_string();
        respond(request, 200, "application/json", body.as_bytes(), &headers);
    } else {
        let mime = guess_mime(path);
        respond_owned(request, 200, mime, data, &headers);
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
    no_cache: bool,
) {
    let entries = match fs.ls(path) {
        Ok(e) => e,
        Err(_) => return respond_404(request, &format!("Not found: {}", path)),
    };

    let mut headers: Vec<(&str, &str)> = vec![
        ("ETag", etag),
        ("Cache-Control", "no-cache"),
    ];
    if cors {
        headers.push(("Access-Control-Allow-Origin", "*"));
    }
    if no_cache {
        headers.push(("Cache-Control", "no-store"));
    }

    let sorted: Vec<&String> = {
        let mut v: Vec<&String> = entries.iter().collect();
        v.sort();
        v
    };

    if want_json {
        let json = serde_json::json!({
            "path": path,
            "ref": ref_label,
            "entries": sorted,
            "type": "directory",
        });
        let body = json.to_string();
        respond(request, 200, "application/json", body.as_bytes(), &headers);
    } else {
        let display_path = if path.is_empty() { "/" } else { path };
        let mut html = format!(
            "<html><body><h1>{}</h1><ul>",
            html_escape(display_path)
        );
        for entry in &sorted {
            let h = if path.is_empty() {
                href(link_prefix, &[entry])
            } else {
                href(link_prefix, &[path, entry])
            };
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                h,
                html_escape(entry)
            ));
        }
        html.push_str("</ul></body></html>");
        respond(
            request,
            200,
            "text/html; charset=utf-8",
            html.as_bytes(),
            &headers,
        );
    }
}

fn serve_ref_listing(
    request: tiny_http::Request,
    store: &GitStore,
    base_path: &str,
    want_json: bool,
    cors: bool,
) {
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
        respond(request, 200, "application/json", body.as_bytes(), &headers);
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
        respond(
            request,
            200,
            "text/html; charset=utf-8",
            html.as_bytes(),
            &headers,
        );
    }
}

fn serve_path(
    request: tiny_http::Request,
    fs: &crate::Fs,
    ref_label: &str,
    link_prefix: &str,
    path: &str,
    cors: bool,
    no_cache: bool,
    max_file_size: u64,
) {
    let etag = format!(
        "\"{}\"",
        fs.commit_hash().unwrap_or_default()
    );

    // Check If-None-Match
    let if_none_match: Option<String> = request
        .headers()
        .iter()
        .find(|h| h.field.as_str() == "If-None-Match" || h.field.as_str() == "if-none-match")
        .map(|h| h.value.as_str().to_string());
    if let Some(ref inm) = if_none_match {
        if inm == &etag {
            return respond_304(request, &etag);
        }
    }

    let accept = request
        .headers()
        .iter()
        .find(|h| h.field.as_str() == "Accept" || h.field.as_str() == "accept")
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let want_json = accept.contains("application/json");

    if path.is_empty() {
        return serve_dir(
            request, fs, ref_label, link_prefix, "", &etag, want_json, cors, no_cache,
        );
    }

    if !fs.exists(path).unwrap_or(false) {
        return respond_404(request, &format!("Not found: {}", path));
    }

    if fs.is_dir(path).unwrap_or(false) {
        serve_dir(
            request, fs, ref_label, link_prefix, path, &etag, want_json, cors, no_cache,
        )
    } else {
        serve_file(request, fs, path, &etag, want_json, cors, no_cache, ref_label, max_file_size)
    }
}

// ---------------------------------------------------------------------------
// Main command
// ---------------------------------------------------------------------------

pub fn cmd_serve(repo_path: &str, args: &ServeArgs, _verbose: bool) -> Result<(), CliError> {
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

    let addr = format!("{}:{}", args.host, args.port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| CliError::new(format!("Failed to bind {}: {}", addr, e)))?;

    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(args.port);
    let url = format!("http://{}:{}{}/", args.host, actual_port, base_path);
    eprintln!("Serving {} ({}) at {}", repo_path, mode_label, url);
    eprintln!("Press Ctrl+C to stop.");

    if args.open_browser {
        let _ = open_url(&url);
    }

    loop {
        let request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };

        // Log request
        if !args.quiet {
            eprintln!(
                "{} - {} {}",
                request
                    .remote_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                request.method(),
                request.url(),
            );
        }

        // Handle CORS preflight
        if args.cors && request.method().as_str() == "OPTIONS" {
            respond(
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
            continue;
        }

        // Strip base path
        let raw_path = request.url().to_string();
        let path = if !base_path.is_empty() {
            if let Some(rest) = raw_path.strip_prefix(&base_path) {
                rest.to_string()
            } else {
                respond_404(request, "Not found");
                continue;
            }
        } else {
            raw_path
        };

        let path = path.trim_matches('/').to_string();
        // Percent-decode
        let path = percent_decode(&path);

        if args.all_refs {
            // Multi-ref mode
            if path.is_empty() {
                let want_json = request
                    .headers()
                    .iter()
                    .any(|h| (h.field.as_str() == "Accept" || h.field.as_str() == "accept")
                        && h.value.as_str().contains("application/json"));
                serve_ref_listing(request, &store, &base_path, want_json, args.cors);
                continue;
            }

            let (ref_name, rest) = match path.find('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => (path.as_str(), ""),
            };

            let fs = match store.fs(ref_name) {
                Ok(fs) => fs,
                Err(_) => {
                    respond_404(request, &format!("Unknown ref: {}", ref_name));
                    continue;
                }
            };

            let link_pfx = format!("{}/{}", base_path, ref_name);
            serve_path(request, &fs, ref_name, &link_pfx, rest, args.cors, args.no_cache, max_file_bytes);
        } else {
            // Single-ref mode: resolve fresh FS each request (live)
            let fs = match resolve_fs(&store, &branch, &args.snap) {
                Ok(fs) => fs,
                Err(e) => {
                    respond_404(request, &e.message);
                    continue;
                }
            };

            serve_path(
                request,
                &fs,
                &ref_label,
                &base_path,
                &path,
                args.cors,
                args.no_cache,
                max_file_bytes,
            );
        }
    }

    Ok(())
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
