/**
 * Copy/sync/remove/move operations between local disk and git repo.
 *
 * Ports the Python vost copy/ subpackage into a single module.
 */

import { createHash } from 'node:crypto';
import { join, relative, basename, dirname } from 'node:path';
import git from 'isomorphic-git';
import {
  MODE_BLOB,
  MODE_BLOB_EXEC,
  MODE_LINK,
  FileNotFoundError,
  IsADirectoryError,
  NotADirectoryError,
  fileTypeFromMode,
  fileEntryFromMode,
  emptyChangeReport,
  finalizeChanges,
  type FsModule,
  type FileEntry,
  type ChangeReport,
  type ChangeError,
  FileType,
} from './types.js';
import { normalizePath } from './paths.js';
import { entryAtPath, modeFromDisk, walkTo } from './tree.js';
import { globMatch } from './glob.js';
import type { FS } from './fs.js';
import type { ExcludeFilter } from './exclude.js';
import type { Batch } from './batch.js';

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

const HASH_CHUNK_SIZE = 65536;

function blobHasher(size: number) {
  const h = createHash('sha1');
  h.update(`blob ${size}\0`);
  return h;
}

async function localFileOid(
  fsModule: FsModule,
  fullPath: string,
  followSymlinks = false,
): Promise<string> {
  const stat = await fsModule.promises.lstat(fullPath);
  if (!followSymlinks && stat.isSymbolicLink()) {
    const target = await fsModule.promises.readlink(fullPath);
    const data = Buffer.from(target, 'utf8');
    const h = blobHasher(data.length);
    h.update(data);
    return h.digest('hex');
  }
  const data = (await fsModule.promises.readFile(fullPath)) as Uint8Array;
  const h = blobHasher(data.length);
  h.update(data);
  return h.digest('hex');
}

// ---------------------------------------------------------------------------
// Directory walking
// ---------------------------------------------------------------------------

async function walkLocalPaths(
  fsModule: FsModule,
  localPath: string,
  followSymlinks = false,
  exclude?: ExcludeFilter,
): Promise<Set<string>> {
  const result = new Set<string>();

  async function recurse(dir: string, relDir: string) {
    let entries: string[];
    try {
      entries = (await fsModule.promises.readdir(dir)) as string[];
    } catch {
      return;
    }
    for (const name of entries) {
      const full = join(dir, name);
      const rel = relDir ? `${relDir}/${name}` : name;
      let stat;
      try {
        stat = await fsModule.promises.lstat(full);
      } catch {
        continue;
      }
      if (stat.isDirectory()) {
        if (exclude && exclude.active && exclude.isExcluded(rel, true)) continue;
        if (!followSymlinks && stat.isSymbolicLink()) {
          result.add(rel);
        } else {
          await recurse(full, rel);
        }
      } else {
        if (exclude && exclude.active && exclude.isExcluded(rel, false)) continue;
        result.add(rel);
      }
    }
  }

  await recurse(localPath, '');
  return result;
}

/**
 * Walk a git tree and return file entries as a map.
 *
 * Builds a `{relativePath: {oid, mode}}` map for all files under
 * `repoPath`. OID values are hex strings suitable for comparison
 * against local file hashes. Returns an empty map when `repoPath`
 * does not exist or is not a directory.
 *
 * @param fs - Filesystem snapshot to walk.
 * @param repoPath - Root path in the repo tree (empty string for root).
 * @returns Map of relative paths to `{oid, mode}` objects.
 */
export async function walkRepo(
  fs: FS,
  repoPath: string,
): Promise<Map<string, { oid: string; mode: string }>> {
  const result = new Map<string, { oid: string; mode: string }>();
  if (repoPath) {
    if (!(await fs.exists(repoPath))) return result;
    if (!(await fs.isDir(repoPath))) return result;
  }
  const walkPath = repoPath || null;
  for await (const [dirpath, , files] of fs.walk(walkPath)) {
    for (const fe of files) {
      const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
      let rel: string;
      if (repoPath && storePath.startsWith(repoPath + '/')) {
        rel = storePath.slice(repoPath.length + 1);
      } else {
        rel = storePath;
      }
      result.set(rel, { oid: fe.oid, mode: fe.mode });
    }
  }
  return result;
}

// ---------------------------------------------------------------------------
// Disk-side glob
// ---------------------------------------------------------------------------

/**
 * Expand a glob pattern against the local filesystem.
 *
 * Uses the same dotfile-aware rules as the repo-side `fs.glob()`:
 * `*` and `?` do not match a leading `.` unless the pattern segment
 * itself starts with `.`.
 *
 * @param fsModule - Node.js-compatible filesystem module.
 * @param pattern - Glob pattern (e.g. `"src/**\/*.ts"`). Supports `*`, `?`, and `**`.
 * @returns Sorted list of matching paths.
 */
export async function diskGlob(fsModule: FsModule, pattern: string): Promise<string[]> {
  pattern = pattern.replace(/\/+$/, '');
  if (!pattern) return [];
  pattern = pattern.replace(/\\/g, '/');

  const isAbs = pattern.startsWith('/');
  if (isAbs) {
    const rest = pattern.slice(1);
    const segments = rest ? rest.split('/') : [];
    const results = await diskGlobWalk(fsModule, segments, '/');
    return results.sort();
  }
  const segments = pattern.split('/');
  const results = await diskGlobWalk(fsModule, segments, '');
  return results.sort();
}

async function diskGlobWalk(
  fsModule: FsModule,
  segments: string[],
  prefix: string,
): Promise<string[]> {
  const seg = segments[0];
  const rest = segments.slice(1);
  const scanDir = prefix || '.';

  if (seg === '**') {
    let entries: string[];
    try {
      entries = (await fsModule.promises.readdir(scanDir)) as string[];
    } catch {
      return [];
    }
    const results: string[] = [];
    if (rest.length > 0) {
      results.push(...(await diskGlobWalk(fsModule, rest, prefix)));
    } else {
      for (const name of entries) {
        if (name.startsWith('.')) continue;
        results.push(prefix ? join(prefix, name) : name);
      }
    }
    for (const name of entries) {
      if (name.startsWith('.')) continue;
      const full = prefix ? join(prefix, name) : name;
      try {
        const stat = await fsModule.promises.stat(full);
        if (stat.isDirectory()) {
          results.push(...(await diskGlobWalk(fsModule, segments, full)));
        }
      } catch { /* skip */ }
    }
    return results;
  }

  const hasWild = seg.includes('*') || seg.includes('?');

  if (hasWild) {
    let entries: string[];
    try {
      entries = (await fsModule.promises.readdir(scanDir)) as string[];
    } catch {
      return [];
    }
    const results: string[] = [];
    for (const name of entries) {
      if (!globMatch(seg, name)) continue;
      const full = prefix ? join(prefix, name) : name;
      if (rest.length > 0) {
        results.push(...(await diskGlobWalk(fsModule, rest, full)));
      } else {
        results.push(full);
      }
    }
    return results;
  }

  const full = prefix ? join(prefix, seg) : seg;
  if (rest.length > 0) {
    return diskGlobWalk(fsModule, rest, full);
  }
  try {
    await fsModule.promises.access(full);
    return [full];
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// Source resolution
// ---------------------------------------------------------------------------

type ResolvedSource = { localPath: string; mode: 'file' | 'dir' | 'contents'; prefix: string };

/**
 * A resolved repo source path with its copy mode.
 * - `repoPath` — normalized path in the repo.
 * - `mode` — 'file' for a single file, 'dir' for a directory, 'contents' for trailing-slash contents mode.
 * - `prefix` — prefix for pivot marker support.
 */
export type ResolvedRepoSource = { repoPath: string; mode: 'file' | 'dir' | 'contents'; prefix: string };

async function resolveDiskSources(
  fsModule: FsModule,
  sources: string[],
): Promise<ResolvedSource[]> {
  const resolved: ResolvedSource[] = [];
  for (const src of sources) {
    const contentsMode = src.endsWith('/');

    if (contentsMode) {
      const path = src.replace(/\/+$/, '');
      let stat;
      try {
        stat = await fsModule.promises.stat(path);
      } catch {
        throw new NotADirectoryError(path);
      }
      if (!stat.isDirectory()) throw new NotADirectoryError(path);
      resolved.push({ localPath: path, mode: 'contents', prefix: '' });
    } else {
      let stat;
      try {
        stat = await fsModule.promises.stat(src);
      } catch {
        throw new FileNotFoundError(src);
      }
      if (stat.isDirectory()) {
        resolved.push({ localPath: src, mode: 'dir', prefix: '' });
      } else {
        resolved.push({ localPath: src, mode: 'file', prefix: '' });
      }
    }
  }
  return resolved;
}

/**
 * Resolve repo source paths into their copy mode (file/dir/contents).
 *
 * Trailing `/` means contents mode; bare directory names mean directory mode;
 * files mean file mode. Empty string or `/` means root contents.
 *
 * @param fs      - The FS snapshot to resolve paths against.
 * @param sources - Array of repo paths to resolve.
 * @returns Array of resolved sources with mode and prefix.
 * @throws {FileNotFoundError} If a source path does not exist.
 * @throws {NotADirectoryError} If a trailing-`/` source is not a directory.
 */
export async function resolveRepoSources(
  fs: FS,
  sources: string[],
): Promise<ResolvedRepoSource[]> {
  const resolved: ResolvedRepoSource[] = [];
  for (const src of sources) {
    const contentsMode = src.endsWith('/');

    if (contentsMode) {
      const path = src.replace(/\/+$/, '');
      const normalized = path ? normalizePath(path) : '';
      if (normalized && !(await fs.isDir(normalized))) {
        throw new NotADirectoryError(normalized);
      }
      resolved.push({ repoPath: normalized, mode: 'contents', prefix: '' });
    } else {
      const normalized = src ? normalizePath(src) : '';
      if (!normalized) {
        resolved.push({ repoPath: '', mode: 'contents', prefix: '' });
      } else if (!(await fs.exists(normalized))) {
        throw new FileNotFoundError(normalized);
      } else if (await fs.isDir(normalized)) {
        resolved.push({ repoPath: normalized, mode: 'dir', prefix: '' });
      } else {
        resolved.push({ repoPath: normalized, mode: 'file', prefix: '' });
      }
    }
  }
  return resolved;
}

// ---------------------------------------------------------------------------
// File enumeration
// ---------------------------------------------------------------------------

async function enumDiskToRepo(
  fsModule: FsModule,
  resolved: ResolvedSource[],
  dest: string,
  followSymlinks = false,
  exclude?: ExcludeFilter,
): Promise<Array<[string, string]>> {
  const pairs: Array<[string, string]> = [];
  for (const { localPath, mode, prefix } of resolved) {
    const _dest = [dest, prefix].filter(Boolean).join('/');

    if (mode === 'file') {
      const name = basename(localPath);
      if (exclude && exclude.active && exclude.isExcluded(name, false)) continue;
      const repoFile = _dest ? `${_dest}/${name}` : name;
      pairs.push([localPath, normalizePath(repoFile)]);
    } else if (mode === 'dir') {
      const dirName = basename(localPath);
      const target = _dest ? `${_dest}/${dirName}` : dirName;
      const rels = await walkLocalPaths(fsModule, localPath, followSymlinks, exclude);
      for (const rel of [...rels].sort()) {
        pairs.push([join(localPath, rel), normalizePath(`${target}/${rel}`)]);
      }
    } else {
      // contents
      const rels = await walkLocalPaths(fsModule, localPath, followSymlinks, exclude);
      for (const rel of [...rels].sort()) {
        const repoFile = _dest ? `${_dest}/${rel}` : rel;
        pairs.push([join(localPath, rel), normalizePath(repoFile)]);
      }
    }
  }
  return pairs;
}

async function enumRepoToDisk(
  fs: FS,
  resolved: ResolvedRepoSource[],
  dest: string,
): Promise<Array<[string, string]>> {
  const pairs: Array<[string, string]> = [];
  for (const { repoPath, mode, prefix } of resolved) {
    const _dest = prefix ? join(dest, prefix) : dest;

    if (mode === 'file') {
      const name = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
      pairs.push([repoPath, join(_dest, name)]);
    } else if (mode === 'dir') {
      const dirName = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
      const target = join(_dest, dirName);
      for await (const [dirpath, , files] of fs.walk(repoPath)) {
        for (const fe of files) {
          const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
          const rel = repoPath && storePath.startsWith(repoPath + '/')
            ? storePath.slice(repoPath.length + 1)
            : storePath;
          pairs.push([storePath, join(target, rel)]);
        }
      }
    } else {
      // contents
      const walkPath = repoPath || null;
      for await (const [dirpath, , files] of fs.walk(walkPath)) {
        for (const fe of files) {
          const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
          const rel = repoPath && storePath.startsWith(repoPath + '/')
            ? storePath.slice(repoPath.length + 1)
            : storePath;
          pairs.push([storePath, join(_dest, rel)]);
        }
      }
    }
  }
  return pairs;
}

async function enumRepoToRepo(
  fs: FS,
  resolved: ResolvedRepoSource[],
  dest: string,
): Promise<Array<[string, string]>> {
  const pairs: Array<[string, string]> = [];
  for (const { repoPath, mode, prefix } of resolved) {
    const _dest = [dest, prefix].filter(Boolean).join('/');

    if (mode === 'file') {
      const name = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
      const destFile = _dest ? `${_dest}/${name}` : name;
      pairs.push([repoPath, normalizePath(destFile)]);
    } else if (mode === 'dir') {
      const dirName = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
      const target = _dest ? `${_dest}/${dirName}` : dirName;
      for await (const [dirpath, , files] of fs.walk(repoPath)) {
        for (const fe of files) {
          const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
          const rel = repoPath && storePath.startsWith(repoPath + '/')
            ? storePath.slice(repoPath.length + 1)
            : storePath;
          pairs.push([storePath, normalizePath(`${target}/${rel}`)]);
        }
      }
    } else {
      const walkPath = repoPath || null;
      for await (const [dirpath, , files] of fs.walk(walkPath)) {
        for (const fe of files) {
          const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
          const rel = repoPath && storePath.startsWith(repoPath + '/')
            ? storePath.slice(repoPath.length + 1)
            : storePath;
          const destFile = _dest ? `${_dest}/${rel}` : rel;
          pairs.push([storePath, normalizePath(destFile)]);
        }
      }
    }
  }
  return pairs;
}

// ---------------------------------------------------------------------------
// File writing helpers
// ---------------------------------------------------------------------------

async function writeFilesToRepo(
  batch: Batch,
  fsModule: FsModule,
  pairs: Array<[string, string]>,
  opts: { followSymlinks?: boolean; mode?: string; ignoreErrors?: boolean; errors?: ChangeError[] },
): Promise<void> {
  for (const [localPath, repoPath] of pairs) {
    try {
      const stat = await fsModule.promises.lstat(localPath);
      if (!opts.followSymlinks && stat.isSymbolicLink()) {
        const target = await fsModule.promises.readlink(localPath);
        await batch.writeSymlink(repoPath, target);
      } else {
        await batch.writeFromFile(repoPath, localPath, { mode: opts.mode });
      }
    } catch (err: any) {
      if (!opts.ignoreErrors) throw err;
      opts.errors?.push({ path: localPath, error: String(err) });
    }
  }
}

async function writeFilesToDisk(
  fs: FS,
  pairs: Array<[string, string]>,
  opts: { ignoreErrors?: boolean; errors?: ChangeError[]; commitTs?: number },
): Promise<void> {
  const fsModule = fs._store._fsModule;
  for (const [repoPath, localPath] of pairs) {
    try {
      // Clear blocking parent paths: if a parent is a file, remove it
      const parentDir = dirname(localPath);
      const parts = parentDir.split('/');
      for (let i = 1; i <= parts.length; i++) {
        const p = parts.slice(0, i).join('/');
        if (!p) continue;
        try {
          const st = await fsModule.promises.lstat(p);
          if (!st.isDirectory()) {
            await fsModule.promises.unlink(p);
            break;
          }
        } catch { /* doesn't exist yet */ break; }
      }
      await fsModule.promises.mkdir(parentDir, { recursive: true });

      // If dest is a directory but we need a file, remove the dir tree
      try {
        const st = await fsModule.promises.lstat(localPath);
        if (st.isDirectory() && !st.isSymbolicLink()) {
          await fsModule.promises.rm!(localPath, { recursive: true, force: true });
        } else {
          await fsModule.promises.unlink(localPath);
        }
      } catch { /* doesn't exist */ }

      const entry = await entryAtPath(
        fsModule,
        fs._store._gitdir,
        fs._treeOid,
        repoPath,
      );
      if (entry && entry.mode === MODE_LINK) {
        const target = await fs.readlink(repoPath);
        await fsModule.promises.symlink(target, localPath);
      } else {
        const data = await fs.read(repoPath);
        await fsModule.promises.writeFile(localPath, data);
        if (entry && entry.mode === MODE_BLOB_EXEC) {
          await fsModule.promises.chmod(localPath, 0o755);
        }
      }
    } catch (err: any) {
      if (!opts.ignoreErrors) throw err;
      opts.errors?.push({ path: localPath, error: String(err) });
    }
  }
}

function filterTreeConflicts(writePaths: Set<string>, deletes: string[]): string[] {
  return deletes.filter((d) => {
    for (const w of writePaths) {
      if (d.startsWith(w + '/') || w.startsWith(d + '/')) return false;
    }
    return true;
  });
}

async function pruneEmptyDirs(fsModule: FsModule, basePath: string): Promise<void> {
  // Simple bottom-up directory pruning
  async function recurse(dir: string): Promise<boolean> {
    let entries: string[];
    try {
      entries = (await fsModule.promises.readdir(dir)) as string[];
    } catch {
      return false;
    }
    let empty = true;
    for (const name of entries) {
      const full = join(dir, name);
      try {
        const stat = await fsModule.promises.stat(full);
        if (stat.isDirectory()) {
          const childEmpty = await recurse(full);
          if (!childEmpty) empty = false;
        } else {
          empty = false;
        }
      } catch {
        empty = false;
      }
    }
    if (empty && dir !== basePath) {
      try {
        await fsModule.promises.rmdir(dir);
      } catch { /* ignore */ }
    }
    return empty;
  }
  await recurse(basePath);
}

// ---------------------------------------------------------------------------
// Entry helpers
// ---------------------------------------------------------------------------

function makeEntriesFromDisk(
  fsModule: FsModule,
  rels: string[],
  relToAbs: Map<string, string>,
): FileEntry[] {
  return rels.map((rel) => {
    const fullPath = relToAbs.get(rel) ?? rel;
    return { path: rel, type: FileType.BLOB, src: fullPath };
  });
}

async function makeEntriesFromRepo(
  fs: FS,
  rels: string[],
  basePath: string,
): Promise<FileEntry[]> {
  const entries: FileEntry[] = [];
  for (const rel of rels) {
    const fullPath = basePath ? `${basePath}/${rel}` : rel;
    const entry = await entryAtPath(
      fs._store._fsModule,
      fs._store._gitdir,
      fs._treeOid,
      fullPath,
    );
    if (entry) {
      entries.push(fileEntryFromMode(rel, entry.mode, fullPath));
    } else {
      entries.push({ path: rel, type: FileType.BLOB, src: fullPath });
    }
  }
  return entries;
}

async function makeEntriesFromRepoDict(
  fs: FS,
  rels: string[],
  relToRepo: Map<string, string>,
): Promise<FileEntry[]> {
  const entries: FileEntry[] = [];
  for (const rel of rels) {
    const repoPath = relToRepo.get(rel) ?? rel;
    const entry = await entryAtPath(
      fs._store._fsModule,
      fs._store._gitdir,
      fs._treeOid,
      repoPath,
    );
    if (entry) {
      entries.push(fileEntryFromMode(rel, entry.mode, repoPath));
    } else {
      entries.push({ path: rel, type: FileType.BLOB, src: repoPath });
    }
  }
  return entries;
}

// ---------------------------------------------------------------------------
// Copy: disk → repo
// ---------------------------------------------------------------------------

/**
 * Copy local files, directories, or globs into the repo.
 *
 * Sources may use a trailing `/` for "contents" mode (pour directory
 * contents into `dest` without creating a subdirectory).
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * With `delete: true`, files under `dest` that are not covered by
 * `sources` are removed (rsync `--delete` semantics).
 *
 * @param fs - Filesystem snapshot (must be writable, i.e. a branch).
 * @param sources - One or more local paths to copy. A trailing `/` means "contents of directory".
 * @param dest - Destination path in the repo tree.
 * @param opts - Copy options.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.followSymlinks - Dereference symlinks instead of storing them. Default `false`.
 * @param opts.message - Custom commit message.
 * @param opts.mode - Override file mode for all written files.
 * @param opts.ignoreExisting - Skip files that already exist at the destination. Default `false`.
 * @param opts.delete - Remove destination files not present in sources. Default `false`.
 * @param opts.ignoreErrors - Continue on per-file errors, collecting them in `changes.errors`. Default `false`.
 * @param opts.checksum - Use content hashing to detect changes. When `false`, uses mtime comparison. Default `true`.
 * @param opts.operation - Operation name for the commit message. Default `"cp"`.
 * @returns FS snapshot after the copy, with `.changes` set.
 * @throws {FileNotFoundError} If a source path does not exist (unless `ignoreErrors` is set).
 * @throws {NotADirectoryError} If a trailing-`/` source is not a directory.
 */
export async function copyIn(
  fs: FS,
  sources: string | string[],
  dest: string,
  opts: {
    dryRun?: boolean;
    followSymlinks?: boolean;
    message?: string;
    mode?: string;
    ignoreExisting?: boolean;
    delete?: boolean;
    ignoreErrors?: boolean;
    checksum?: boolean;
    operation?: string;
    exclude?: ExcludeFilter;
    parents?: FS[];
  } = {},
): Promise<FS> {
  const srcList = typeof sources === 'string' ? [sources] : sources;
  const fsModule = fs._store._fsModule;
  const changes = emptyChangeReport();

  let resolved: ResolvedSource[];
  if (opts.ignoreErrors) {
    resolved = [];
    for (const src of srcList) {
      try {
        resolved.push(...await resolveDiskSources(fsModule, [src]));
      } catch (err: any) {
        changes.errors.push({ path: src, error: String(err.message ?? err) });
      }
    }
    if (resolved.length === 0) {
      if (changes.errors.length > 0) {
        throw new Error(`All files failed to copy: ${changes.errors.map((e) => e.error).join(', ')}`);
      }
      fs._changes = finalizeChanges(changes);
      return fs;
    }
  } else {
    resolved = await resolveDiskSources(fsModule, srcList);
  }
  let pairs = await enumDiskToRepo(fsModule, resolved, dest, opts.followSymlinks, opts.exclude);

  if (opts.delete) {
    // Build {repo_rel: local_abs} map
    const pairMap = new Map<string, string>();
    for (const [localPath, repoPath] of pairs) {
      const rel = dest && repoPath.startsWith(dest + '/')
        ? repoPath.slice(dest.length + 1)
        : repoPath;
      if (!pairMap.has(rel)) pairMap.set(rel, localPath);
    }

    const repoFiles = await walkRepo(fs, dest);
    const localRels = new Set(pairMap.keys());
    const repoRels = new Set(repoFiles.keys());

    const addRels = [...localRels].filter((r) => !repoRels.has(r)).sort();
    const deleteRels = [...repoRels].filter((r) => !localRels.has(r)).sort();
    const both = [...localRels].filter((r) => repoRels.has(r)).sort();

    const commitTs = opts.checksum === false ? await fs._getCommitTime() : 0;

    const updateRels: string[] = [];
    for (const rel of both) {
      try {
        const repoInfo = repoFiles.get(rel)!;
        const localPath = pairMap.get(rel)!;

        // mtime fast path: if file is older than commit, assume unchanged
        if (opts.checksum === false) {
          try {
            const st = await fsModule.promises.stat(localPath);
            if (Math.floor(st.mtimeMs / 1000) <= commitTs) continue;
          } catch { /* fall through to hash */ }
        }

        const localOid = await localFileOid(fsModule, localPath, opts.followSymlinks);
        if (localOid !== repoInfo.oid) {
          updateRels.push(rel);
        } else if (repoInfo.mode !== MODE_LINK) {
          const diskMode = await modeFromDisk(fsModule, localPath);
          if (diskMode !== repoInfo.mode) updateRels.push(rel);
        }
      } catch {
        updateRels.push(rel);
      }
    }

    if (opts.dryRun) {
      changes.add = makeEntriesFromDisk(fsModule, addRels, pairMap);
      changes.update = makeEntriesFromDisk(fsModule, opts.ignoreExisting ? [] : updateRels, pairMap);
      changes.delete = await makeEntriesFromRepo(fs, deleteRels, dest);
      fs._changes = finalizeChanges(changes);
      return fs;
    }

    const writeRels = [...addRels, ...(opts.ignoreExisting ? [] : updateRels)];
    const writePairs: Array<[string, string]> = writeRels.map((rel) => [
      pairMap.get(rel)!,
      dest ? `${dest}/${rel}` : rel,
    ]);
    const safeDeletes = filterTreeConflicts(new Set(writeRels), deleteRels);
    const deleteFull = safeDeletes.map((rel) => (dest ? `${dest}/${rel}` : rel));

    changes.add = makeEntriesFromDisk(fsModule, addRels, pairMap);
    changes.update = makeEntriesFromDisk(fsModule, opts.ignoreExisting ? [] : updateRels, pairMap);
    changes.delete = await makeEntriesFromRepo(fs, safeDeletes, dest);

    if (writePairs.length === 0 && deleteFull.length === 0) {
      fs._changes = finalizeChanges(changes);
      return fs;
    }

    const batch = fs.batch({ message: opts.message, operation: opts.operation ?? 'cp', parents: opts.parents });
    await writeFilesToRepo(batch, fsModule, writePairs, {
      followSymlinks: opts.followSymlinks,
      mode: opts.mode,
      ignoreErrors: opts.ignoreErrors,
      errors: changes.errors,
    });
    for (const path of deleteFull) {
      try {
        await batch.remove(path);
      } catch { /* ignore missing */ }
    }
    const result = await batch.commit();
    result._changes = finalizeChanges(changes);
    return result;
  }

  // Non-delete mode
  if (opts.ignoreExisting) {
    const filtered: Array<[string, string]> = [];
    for (const [l, r] of pairs) {
      if (!(await fs.exists(r))) filtered.push([l, r]);
    }
    pairs = filtered;
  }

  if (pairs.length === 0) {
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  // Classify as add vs update
  const addRels: string[] = [];
  const updateRels: string[] = [];
  const pairMap = new Map<string, string>();
  for (const [localPath, repoPath] of pairs) {
    const rel = dest && repoPath.startsWith(dest + '/')
      ? repoPath.slice(dest.length + 1)
      : repoPath;
    pairMap.set(rel, localPath);
    if (await fs.exists(repoPath)) {
      updateRels.push(rel);
    } else {
      addRels.push(rel);
    }
  }

  if (opts.dryRun) {
    changes.add = makeEntriesFromDisk(fsModule, addRels, pairMap);
    changes.update = makeEntriesFromDisk(fsModule, updateRels, pairMap);
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  changes.add = makeEntriesFromDisk(fsModule, addRels, pairMap);
  changes.update = makeEntriesFromDisk(fsModule, updateRels, pairMap);

  const batch = fs.batch({ message: opts.message, operation: opts.operation ?? 'cp', parents: opts.parents });
  await writeFilesToRepo(batch, fsModule, pairs, {
    followSymlinks: opts.followSymlinks,
    mode: opts.mode,
    ignoreErrors: opts.ignoreErrors,
    errors: changes.errors,
  });
  const result = await batch.commit();
  result._changes = finalizeChanges(changes);
  return result;
}

// ---------------------------------------------------------------------------
// Copy: repo → disk
// ---------------------------------------------------------------------------

/**
 * Copy repo files, directories, or globs to local disk.
 *
 * Sources may use a trailing `/` for "contents" mode (pour directory
 * contents into `dest` without creating a subdirectory).
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * With `delete: true`, local files under `dest` that are not covered
 * by `sources` are removed (rsync `--delete` semantics).
 *
 * @param fs - Filesystem snapshot to copy from.
 * @param sources - One or more repo paths to copy. A trailing `/` means "contents of directory".
 * @param dest - Destination directory on local disk.
 * @param opts - Copy options.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.ignoreExisting - Skip files that already exist at the destination. Default `false`.
 * @param opts.delete - Remove local files not present in sources. Default `false`.
 * @param opts.ignoreErrors - Continue on per-file errors, collecting them in `changes.errors`. Default `false`.
 * @param opts.checksum - Use content hashing to detect changes. When `false`, uses mtime comparison. Default `true`.
 * @param opts.operation - Operation name for the commit message.
 * @returns FS snapshot with `.changes` set describing what was copied.
 * @throws {FileNotFoundError} If a source path does not exist in the repo (unless `ignoreErrors` is set).
 * @throws {NotADirectoryError} If a trailing-`/` source is not a directory.
 */
export async function copyOut(
  fs: FS,
  sources: string | string[],
  dest: string,
  opts: {
    dryRun?: boolean;
    ignoreExisting?: boolean;
    delete?: boolean;
    ignoreErrors?: boolean;
    checksum?: boolean;
    operation?: string;
  } = {},
): Promise<FS> {
  const srcList = typeof sources === 'string' ? [sources] : sources;
  const fsModule = fs._store._fsModule;
  const changes = emptyChangeReport();

  let resolved: ResolvedRepoSource[];
  if (opts.ignoreErrors) {
    resolved = [];
    for (const src of srcList) {
      try {
        resolved.push(...await resolveRepoSources(fs, [src]));
      } catch (err: any) {
        changes.errors.push({ path: src, error: String(err.message ?? err) });
      }
    }
    if (resolved.length === 0) {
      if (changes.errors.length > 0) {
        throw new Error(`All files failed to copy: ${changes.errors.map((e) => e.error).join(', ')}`);
      }
      fs._changes = finalizeChanges(changes);
      return fs;
    }
  } else {
    resolved = await resolveRepoSources(fs, srcList);
  }
  let pairs = await enumRepoToDisk(fs, resolved, dest);

  if (opts.delete) {
    await fsModule.promises.mkdir(dest, { recursive: true });

    const pairMap = new Map<string, string>();
    for (const [repoPath, localPath] of pairs) {
      const rel = relative(dest, localPath).replace(/\\/g, '/');
      if (!pairMap.has(rel)) pairMap.set(rel, repoPath);
    }

    const repoFiles = new Map<string, { oid: string; mode: string }>();
    for (const [rel, rp] of pairMap) {
      const entry = await entryAtPath(fsModule, fs._store._gitdir, fs._treeOid, rp);
      if (entry) repoFiles.set(rel, entry);
    }

    const localPaths = await walkLocalPaths(fsModule, dest);
    const sourceRels = new Set(pairMap.keys());

    const addRels = [...sourceRels].filter((r) => !localPaths.has(r)).sort();
    const deleteRels = [...localPaths].filter((r) => !sourceRels.has(r)).sort();
    const both = [...sourceRels].filter((r) => localPaths.has(r)).sort();

    const commitTs = opts.checksum === false ? await fs._getCommitTime() : 0;

    const updateRels: string[] = [];
    for (const rel of both) {
      const repoInfo = repoFiles.get(rel);
      if (!repoInfo) continue;
      try {
        const localPath = join(dest, rel);

        // mtime fast path: if file is older than commit, assume unchanged
        if (opts.checksum === false) {
          try {
            const st = await fsModule.promises.stat(localPath);
            if (Math.floor(st.mtimeMs / 1000) <= commitTs) continue;
          } catch { /* fall through to hash */ }
        }

        const oid = await localFileOid(fsModule, localPath);
        if (oid !== repoInfo.oid) {
          updateRels.push(rel);
        } else if (repoInfo.mode !== MODE_LINK) {
          const diskMode = await modeFromDisk(fsModule, localPath);
          if (diskMode !== repoInfo.mode) updateRels.push(rel);
        }
      } catch {
        updateRels.push(rel);
      }
    }

    if (opts.dryRun) {
      changes.add = await makeEntriesFromRepoDict(fs, addRels, pairMap);
      changes.update = await makeEntriesFromRepoDict(fs, opts.ignoreExisting ? [] : updateRels, pairMap);
      changes.delete = deleteRels.map((rel) => ({ path: rel, type: FileType.BLOB as FileType }));
      fs._changes = finalizeChanges(changes);
      return fs;
    }

    // Delete local files
    for (const rel of deleteRels) {
      try {
        await fsModule.promises.unlink(join(dest, rel));
      } catch { /* ignore */ }
    }

    const writeRels = [...addRels, ...(opts.ignoreExisting ? [] : updateRels)];
    const writePairs: Array<[string, string]> = writeRels.map((rel) => [
      pairMap.get(rel)!,
      join(dest, rel),
    ]);

    await writeFilesToDisk(fs, writePairs, {
      ignoreErrors: opts.ignoreErrors,
      errors: changes.errors,
    });
    await pruneEmptyDirs(fsModule, dest);

    changes.add = await makeEntriesFromRepoDict(fs, addRels, pairMap);
    changes.update = await makeEntriesFromRepoDict(fs, opts.ignoreExisting ? [] : updateRels, pairMap);
    changes.delete = deleteRels.map((rel) => ({ path: rel, type: FileType.BLOB as FileType }));
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  // Non-delete mode
  if (opts.ignoreExisting) {
    const filtered: Array<[string, string]> = [];
    for (const [r, l] of pairs) {
      try {
        await fsModule.promises.access(l);
      } catch {
        filtered.push([r, l]);
      }
    }
    pairs = filtered;
  }

  if (pairs.length === 0) {
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  if (opts.dryRun) {
    const addRels: string[] = [];
    const updateRels: string[] = [];
    const relToRepo = new Map<string, string>();
    for (const [repoPath, localPath] of pairs) {
      const rel = relative(dest, localPath).replace(/\\/g, '/');
      relToRepo.set(rel, repoPath);
      try {
        await fsModule.promises.access(localPath);
        updateRels.push(rel);
      } catch {
        addRels.push(rel);
      }
    }
    changes.add = await makeEntriesFromRepoDict(fs, addRels.sort(), relToRepo);
    changes.update = await makeEntriesFromRepoDict(fs, updateRels.sort(), relToRepo);
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  // Classify and write
  const addRels: string[] = [];
  const updateRels: string[] = [];
  const relToRepo = new Map<string, string>();
  for (const [repoPath, localPath] of pairs) {
    const rel = relative(dest, localPath).replace(/\\/g, '/');
    relToRepo.set(rel, repoPath);
    try {
      await fsModule.promises.access(localPath);
      updateRels.push(rel);
    } catch {
      addRels.push(rel);
    }
  }

  await writeFilesToDisk(fs, pairs, {
    ignoreErrors: opts.ignoreErrors,
    errors: changes.errors,
  });

  changes.add = await makeEntriesFromRepoDict(fs, addRels, relToRepo);
  changes.update = await makeEntriesFromRepoDict(fs, updateRels, relToRepo);
  fs._changes = finalizeChanges(changes);
  return fs;
}

// ---------------------------------------------------------------------------
// Remove
// ---------------------------------------------------------------------------

async function collectRemovePaths(
  fs: FS,
  sources: string[],
  recursive = false,
): Promise<string[]> {
  const resolved = await resolveRepoSources(fs, sources);
  const deletePaths: string[] = [];
  for (const { repoPath, mode } of resolved) {
    if (mode === 'file') {
      deletePaths.push(repoPath);
    } else if (mode === 'dir' || mode === 'contents') {
      if (!recursive) {
        throw new IsADirectoryError(`${repoPath} is a directory (use recursive=true)`);
      }
      const walkRoot = repoPath || null;
      for await (const [dirpath, , files] of fs.walk(walkRoot)) {
        for (const fe of files) {
          deletePaths.push(dirpath ? `${dirpath}/${fe.name}` : fe.name);
        }
      }
    }
  }
  return [...new Set(deletePaths)].sort();
}

/**
 * Remove files or directories from the repo.
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * @param fs - Filesystem snapshot (must be writable, i.e. a branch).
 * @param sources - One or more repo paths to remove.
 * @param opts - Remove options.
 * @param opts.recursive - Allow removal of directories. Default `false`.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.message - Custom commit message.
 * @returns FS snapshot after the removal, with `.changes` set.
 * @throws {FileNotFoundError} If no source matches any file.
 * @throws {IsADirectoryError} If a source is a directory and `recursive` is `false`.
 */
export async function remove(
  fs: FS,
  sources: string | string[],
  opts: { recursive?: boolean; dryRun?: boolean; message?: string; parents?: FS[] } = {},
): Promise<FS> {
  const srcList = typeof sources === 'string' ? [sources] : sources;
  const deletePaths = await collectRemovePaths(fs, srcList, opts.recursive);
  if (deletePaths.length === 0) throw new FileNotFoundError(`No matches for: ${srcList}`);

  const changes = emptyChangeReport();
  const relToRepo = new Map(deletePaths.map((p) => [p, p]));
  changes.delete = await makeEntriesFromRepoDict(fs, deletePaths, relToRepo);

  if (opts.dryRun) {
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  const batch = fs.batch({ message: opts.message, operation: 'rm', parents: opts.parents });
  for (const path of deletePaths) {
    await batch.remove(path);
  }
  const result = await batch.commit();
  result._changes = finalizeChanges(changes);
  return result;
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/**
 * Make `repoPath` identical to `localPath` (disk to repo sync).
 *
 * Copies new and changed files from `localPath` into `repoPath` and
 * deletes repo files that do not exist on disk. Equivalent to
 * `copyIn` with `delete: true`.
 *
 * If `localPath` does not exist, all files under `repoPath` are
 * deleted (treating the source as empty).
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * @param fs - Filesystem snapshot (must be writable, i.e. a branch).
 * @param localPath - Source directory on local disk.
 * @param repoPath - Destination path in the repo tree.
 * @param opts - Sync options.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.message - Custom commit message.
 * @param opts.ignoreErrors - Continue on per-file errors. Default `false`.
 * @param opts.checksum - Use content hashing. When `false`, uses mtime. Default `true`.
 * @returns FS snapshot after sync, with `.changes` set.
 * @throws {PermissionError} If the FS is read-only.
 */
export async function syncIn(
  fs: FS,
  localPath: string,
  repoPath: string,
  opts: {
    dryRun?: boolean;
    message?: string;
    ignoreErrors?: boolean;
    checksum?: boolean;
    exclude?: ExcludeFilter;
    parents?: FS[];
  } = {},
): Promise<FS> {
  const src = localPath.endsWith('/') ? localPath : localPath + '/';
  try {
    return await copyIn(fs, [src], repoPath, {
      ...opts,
      delete: true,
      operation: 'sync',
    });
  } catch (err: any) {
    if (err instanceof FileNotFoundError || err instanceof NotADirectoryError) {
      // Nonexistent local → delete everything under repoPath
      const dest = repoPath ? normalizePath(repoPath) : '';
      const repoFiles = await walkRepo(fs, dest);
      if (repoFiles.size === 0) {
        // Check if dest is a single file
        if (dest && (await fs.exists(dest)) && !(await fs.isDir(dest))) {
          if (opts.dryRun) {
            const entry = await entryAtPath(
              fs._store._fsModule,
              fs._store._gitdir,
              fs._treeOid,
              dest,
            );
            const changes = emptyChangeReport();
            changes.delete = entry ? [fileEntryFromMode(dest, entry.mode)] : [{ path: dest, type: FileType.BLOB }];
            fs._changes = changes;
            return fs;
          }
          const batch = fs.batch({ message: opts.message, operation: 'sync', parents: opts.parents });
          await batch.remove(dest);
          return await batch.commit();
        }
        fs._changes = null;
        return fs;
      }
      if (opts.dryRun) {
        const changes = emptyChangeReport();
        changes.delete = await makeEntriesFromRepo(fs, [...repoFiles.keys()].sort(), dest);
        fs._changes = finalizeChanges(changes);
        return fs;
      }
      const batch = fs.batch({ message: opts.message, parents: opts.parents });
      for (const rel of [...repoFiles.keys()].sort()) {
        const full = dest ? `${dest}/${rel}` : rel;
        await batch.remove(full);
      }
      const result = await batch.commit();
      result._changes = emptyChangeReport();
      result._changes.delete = await makeEntriesFromRepo(fs, [...repoFiles.keys()].sort(), dest);
      return result;
    }
    throw err;
  }
}

/**
 * Make `localPath` identical to `repoPath` (repo to disk sync).
 *
 * Copies new and changed files from the repo to `localPath` and
 * deletes local files that do not exist in the repo. Equivalent to
 * `copyOut` with `delete: true`.
 *
 * If `repoPath` does not exist in the repo, all files under
 * `localPath` are deleted (treating the source as empty).
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * @param fs - Filesystem snapshot to sync from.
 * @param repoPath - Source path in the repo tree.
 * @param localPath - Destination directory on local disk.
 * @param opts - Sync options.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.ignoreErrors - Continue on per-file errors. Default `false`.
 * @param opts.checksum - Use content hashing. When `false`, uses mtime. Default `true`.
 * @returns FS snapshot with `.changes` set describing what was synced.
 */
export async function syncOut(
  fs: FS,
  repoPath: string,
  localPath: string,
  opts: {
    dryRun?: boolean;
    ignoreErrors?: boolean;
    checksum?: boolean;
  } = {},
): Promise<FS> {
  const src = repoPath.endsWith('/') ? repoPath : repoPath + '/';
  try {
    return await copyOut(fs, [src], localPath, { ...opts, delete: true });
  } catch (err: any) {
    if (err instanceof FileNotFoundError || err instanceof NotADirectoryError) {
      // Nonexistent repo path → delete everything local
      const fsModule = fs._store._fsModule;
      const localPaths = await walkLocalPaths(fsModule, localPath);
      if (localPaths.size === 0) {
        fs._changes = null;
        return fs;
      }
      if (opts.dryRun) {
        const changes = emptyChangeReport();
        changes.delete = [...localPaths].sort().map((p) => ({ path: p, type: FileType.BLOB as FileType }));
        fs._changes = finalizeChanges(changes);
        return fs;
      }
      for (const rel of [...localPaths].sort()) {
        try {
          await fsModule.promises.unlink(join(localPath, rel));
        } catch { /* ignore */ }
      }
      await pruneEmptyDirs(fsModule, localPath);
      const changes = emptyChangeReport();
      changes.delete = [...localPaths].sort().map((p) => ({ path: p, type: FileType.BLOB as FileType }));
      fs._changes = changes;
      return fs;
    }
    throw err;
  }
}

// ---------------------------------------------------------------------------
// Move
// ---------------------------------------------------------------------------

/**
 * Move or rename files within the repo.
 *
 * Implements POSIX `mv` semantics: when there is a single source file
 * and `dest` is not an existing directory and does not end with `/`,
 * the destination is the exact target path (rename). Otherwise files
 * are placed inside `dest`.
 *
 * With `dryRun: true`, no changes are written; the input FS is
 * returned with `.changes` populated.
 *
 * @param fs - Filesystem snapshot (must be writable, i.e. a branch).
 * @param sources - One or more repo paths to move.
 * @param dest - Destination path in the repo tree.
 * @param opts - Move options.
 * @param opts.recursive - Allow moving directories. Default `false`.
 * @param opts.dryRun - Preview changes without writing. Default `false`.
 * @param opts.message - Custom commit message.
 * @returns FS snapshot after the move, with `.changes` set.
 * @throws {FileNotFoundError} If no source matches any file.
 * @throws {IsADirectoryError} If a source is a directory and `recursive` is `false`.
 * @throws {Error} If source and destination are the same path.
 */
export async function move(
  fs: FS,
  sources: string | string[],
  dest: string,
  opts: { recursive?: boolean; dryRun?: boolean; message?: string; parents?: FS[] } = {},
): Promise<FS> {
  const srcList = typeof sources === 'string' ? [sources] : sources;
  const resolved = await resolveRepoSources(fs, srcList);

  const destNorm = dest.replace(/\/+$/, '') ? normalizePath(dest.replace(/\/+$/, '')) : '';
  const destExistsAsDir = destNorm ? await fs.isDir(destNorm) : false;

  // POSIX mv rename detection
  const isRename =
    resolved.length === 1 &&
    (resolved[0].mode === 'file' || resolved[0].mode === 'dir') &&
    !dest.endsWith('/') &&
    !destExistsAsDir;

  let pairs: Array<[string, string]>;
  if (isRename && resolved[0].mode === 'file') {
    pairs = [[resolved[0].repoPath, destNorm || resolved[0].repoPath.split('/').pop()!]];
  } else if (isRename && resolved[0].mode === 'dir') {
    const renamed: ResolvedRepoSource[] = [
      { repoPath: resolved[0].repoPath, mode: 'contents', prefix: '' },
    ];
    pairs = await enumRepoToRepo(fs, renamed, destNorm);
  } else {
    pairs = await enumRepoToRepo(fs, resolved, destNorm);
  }

  if (pairs.length === 0) throw new FileNotFoundError(`No matches for: ${srcList}`);

  // Validate no src == dest
  for (const [src, dst] of pairs) {
    if (src === dst) throw new Error(`Source and destination are the same: ${src}`);
  }

  const deletePaths = await collectRemovePaths(fs, srcList, opts.recursive);

  const changes = emptyChangeReport();
  const destRelToRepo = new Map(pairs.map(([, dp]) => [dp, dp]));
  changes.add = await makeEntriesFromRepoDict(fs, pairs.map(([, dp]) => dp), destRelToRepo);
  const srcRelToRepo = new Map(deletePaths.map((p) => [p, p]));
  changes.delete = await makeEntriesFromRepoDict(fs, deletePaths, srcRelToRepo);

  if (opts.dryRun) {
    fs._changes = finalizeChanges(changes);
    return fs;
  }

  const batch = fs.batch({ message: opts.message, operation: 'mv', parents: opts.parents });
  for (const [src, dst] of pairs) {
    // Copy blob from src to dst
    const entry = await entryAtPath(fs._store._fsModule, fs._store._gitdir, fs._treeOid, src);
    if (entry) {
      const { blob } = await git.readBlob({
        fs: fs._store._fsModule,
        gitdir: fs._store._gitdir,
        oid: entry.oid,
      });
      await batch.write(dst, blob, { mode: entry.mode });
    }
  }
  for (const path of deletePaths) {
    await batch.remove(path);
  }
  const result = await batch.commit();
  result._changes = finalizeChanges(changes);
  return result;
}
