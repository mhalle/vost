#pragma once

/// @file mirror.h
/// Mirror (backup/restore) operations for vost.

#include "types.h"

#include <memory>
#include <string>

namespace vost {

struct GitStoreInner;

namespace mirror {

/// Push local refs to @p dest, creating a mirror or bundle.
///
/// Without refs filtering this is a full mirror: remote-only refs are
/// deleted.  With ``opts.refs`` only the specified refs are pushed (no
/// deletes).  If ``opts.format == "bundle"`` or @p dest ends in
/// ``.bundle``, a git bundle file is written instead.
///
/// @param inner  Shared inner state of the GitStore.
/// @param dest   Destination URL, local path, or bundle file path.
/// @param opts   BackupOptions (dry_run, refs filter, format).
/// @return MirrorDiff describing what changed (or would change).
/// @throws InvalidPathError for scp-style URLs.
/// @throws GitError on transport failures.
MirrorDiff backup(const std::shared_ptr<GitStoreInner>& inner,
                  const std::string& dest,
                  const BackupOptions& opts = {});

/// Fetch refs from @p src additively (no deletes).
///
/// Restore is **additive**: it adds and updates refs but never deletes
/// local-only refs.  If ``opts.format == "bundle"`` or @p src ends in
/// ``.bundle``, refs are imported from a git bundle file.
///
/// @param inner  Shared inner state of the GitStore.
/// @param src    Source URL, local path, or bundle file path.
/// @param opts   RestoreOptions (dry_run, refs filter, format).
/// @return MirrorDiff describing what changed (or would change).
/// @throws InvalidPathError for scp-style URLs.
/// @throws GitError on transport failures.
MirrorDiff restore(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& src,
                   const RestoreOptions& opts = {});

/// Export refs to a git bundle file.
///
/// @param inner    Shared inner state of the GitStore.
/// @param path     Path to the bundle file to write.
/// @param refs     Ref names to export (empty = all refs).
/// @param ref_map  Rename map: source ref -> destination ref name in bundle
///                 (empty = no renaming).
/// @throws GitError on failures.
void bundle_export(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& path,
                   const std::vector<std::string>& refs = {},
                   const std::map<std::string, std::string>& ref_map = {},
                   bool squash = false);

/// Import refs from a git bundle file.
///
/// @param inner    Shared inner state of the GitStore.
/// @param path     Path to the bundle file to read.
/// @param refs     Ref names to import (empty = all refs).
/// @param ref_map  Rename map: bundle ref name -> local ref name
///                 (empty = no renaming).
/// @throws GitError on failures.
void bundle_import(const std::shared_ptr<GitStoreInner>& inner,
                   const std::string& path,
                   const std::vector<std::string>& refs = {},
                   const std::map<std::string, std::string>& ref_map = {});

} // namespace mirror

/// Inject credentials into an HTTPS URL if available.
///
/// Tries `git credential fill` first (works with any configured helper:
/// osxkeychain, wincred, libsecret, `gh auth setup-git`, etc.).  Falls
/// back to `gh auth token` for GitHub hosts.  Non-HTTPS URLs and URLs
/// that already contain credentials are returned unchanged.
///
/// @param url  The URL to resolve credentials for.
/// @return The URL with credentials injected, or the original URL.
std::string resolve_credentials(const std::string& url);

} // namespace vost
