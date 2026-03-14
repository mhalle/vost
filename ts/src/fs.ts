/**
 * FS: immutable snapshot of a committed tree state.
 *
 * Read-only when writable is false (tag/detached snapshot).
 * Writable when writable is true — writes auto-commit and return a new FS.
 */

import git from 'isomorphic-git';
import {
  MODE_TREE,
  MODE_BLOB,
  MODE_LINK,
  modeToInt,
  FileNotFoundError,
  IsADirectoryError,
  NotADirectoryError,
  PermissionError,
  StaleSnapshotError,
  fileTypeFromMode,
  fileModeFromType,
  fileEntryFromMode,
  emptyChangeReport,
  formatCommitMessage,
  finalizeChanges,
  type FsModule,
  type FileType,
  type WalkEntry,
  type WriteEntry,
  type ChangeReport,
  type CommitInfo,
  type StatResult,
} from './types.js';
import { normalizePath, isRootPath } from './paths.js';
import {
  entryAtPath,
  walkTo,
  readBlobAtPath,
  listTreeAtPath,
  listEntriesAtPath,
  walkTree,
  existsAtPath,
  rebuildTree,
  countSubdirs,
  type TreeWrite,
} from './tree.js';
import { globMatch } from './glob.js';
import { withRepoLock } from './lock.js';
import { readReflog, writeReflogEntry, ZERO_SHA } from './reflog.js';
import { Batch } from './batch.js';
import { FsWriter } from './fileobj.js';

import type { GitStore } from './gitstore.js';

/**
 * An immutable snapshot of a committed tree.
 *
 * Read-only when `writable` is false (tag snapshot).
 * Writable when `writable` is true -- writes auto-commit and return a new FS.
 */
export class FS {
  /** @internal */
  _store: GitStore;
  /** @internal */
  _commitOid: string;
  /** @internal */
  _refName: string | null;
  /** @internal */
  _writable: boolean;
  /** @internal */
  _treeOid: string;
  /** @internal */
  _changes: ChangeReport | null = null;
  /** @internal */
  _commitTime: number | null = null;

  /** @internal */
  get _fsModule(): FsModule {
    return this._store._fsModule;
  }

  /** @internal */
  get _gitdir(): string {
    return this._store._gitdir;
  }

  constructor(store: GitStore, commitOid: string, treeOid: string, refName: string | null, writable?: boolean) {
    this._store = store;
    this._commitOid = commitOid;
    this._refName = refName;
    this._writable = writable ?? (refName !== null);
    this._treeOid = treeOid;
  }

  /**
   * @internal Create an FS from a commit OID (reads the commit to get tree OID).
   */
  static async _fromCommit(
    store: GitStore,
    commitOid: string,
    refName: string | null,
    writable?: boolean,
  ): Promise<FS> {
    const { commit } = await git.readCommit({
      fs: store._fsModule,
      gitdir: store._gitdir,
      oid: commitOid,
    });
    return new FS(store, commitOid, commit.tree, refName, writable);
  }

  toString(): string {
    const short = this._commitOid.slice(0, 7);
    const parts: string[] = [];
    if (this._refName) parts.push(`refName='${this._refName}'`);
    parts.push(`commit=${short}`);
    if (!this._writable) parts.push('readonly');
    return `FS(${parts.join(', ')})`;
  }

  /** @internal */
  private _readonlyError(verb: string): PermissionError {
    if (this._refName) {
      return new PermissionError(`Cannot ${verb} read-only snapshot (ref '${this._refName}')`);
    }
    return new PermissionError(`Cannot ${verb} read-only snapshot`);
  }

  // ---------------------------------------------------------------------------
  // Properties
  // ---------------------------------------------------------------------------

  /** The 40-character hex SHA of this snapshot's commit. */
  get commitHash(): string {
    return this._commitOid;
  }

  /** The branch or tag name, or `null` for detached snapshots. */
  get refName(): string | null {
    return this._refName;
  }

  /** Whether this snapshot can be written to. */
  get writable(): boolean {
    return this._writable;
  }

  /**
   * Fetch commit metadata (message, time, author name/email) in a single read.
   *
   * @returns Commit info object with message, time, authorName, and authorEmail.
   */
  async getCommitInfo(): Promise<CommitInfo> {
    const { commit } = await git.readCommit({
      fs: this._fsModule,
      gitdir: this._gitdir,
      oid: this._commitOid,
    });
    const offsetMs = commit.author.timezoneOffset * 60 * 1000;
    return {
      message: commit.message.replace(/\n$/, ''),
      time: new Date(commit.author.timestamp * 1000 - offsetMs),
      authorName: commit.author.name,
      authorEmail: commit.author.email,
    };
  }

  /** The commit message (trailing newline stripped). */
  async getMessage(): Promise<string> {
    return (await this.getCommitInfo()).message;
  }

  /** Timezone-aware commit timestamp. */
  async getTime(): Promise<Date> {
    return (await this.getCommitInfo()).time;
  }

  /** The commit author's name. */
  async getAuthorName(): Promise<string> {
    return (await this.getCommitInfo()).authorName;
  }

  /** The commit author's email address. */
  async getAuthorEmail(): Promise<string> {
    return (await this.getCommitInfo()).authorEmail;
  }

  /** Report of the operation that created this snapshot, or `null`. */
  get changes(): ChangeReport | null {
    return this._changes;
  }

  /** The 40-char hex SHA of the root tree. */
  get treeHash(): string {
    return this._treeOid;
  }

  /** @internal */
  async _getCommitTime(): Promise<number> {
    if (this._commitTime !== null) return this._commitTime;
    const { commit } = await git.readCommit({
      fs: this._fsModule,
      gitdir: this._gitdir,
      oid: this._commitOid,
    });
    this._commitTime = commit.committer.timestamp;
    return this._commitTime;
  }

  // ---------------------------------------------------------------------------
  // Read operations
  // ---------------------------------------------------------------------------

  /**
   * Read file contents as bytes.
   *
   * @param path - File path in the repo.
   * @param opts - Optional read options.
   * @param opts.offset - Byte offset to start reading from.
   * @param opts.size - Maximum bytes to return (undefined for all).
   * @returns Raw file contents as Uint8Array.
   * @throws {FileNotFoundError} If path does not exist.
   * @throws {IsADirectoryError} If path is a directory.
   */
  async read(path: string, opts?: { offset?: number; size?: number }): Promise<Uint8Array> {
    const blob = await readBlobAtPath(this._fsModule, this._gitdir, this._treeOid, path);
    if (opts && (opts.offset !== undefined || opts.size !== undefined)) {
      const offset = opts.offset ?? 0;
      const end = opts.size !== undefined ? offset + opts.size : blob.length;
      return blob.subarray(offset, end);
    }
    return blob;
  }

  /**
   * Read file contents as a string.
   *
   * @param path - File path in the repo.
   * @param encoding - Text encoding (default `"utf-8"`).
   * @returns Decoded text content.
   * @throws {FileNotFoundError} If path does not exist.
   * @throws {IsADirectoryError} If path is a directory.
   */
  async readText(path: string, encoding: string = 'utf-8'): Promise<string> {
    const data = await this.read(path);
    return new TextDecoder(encoding).decode(data);
  }

  /**
   * List entry names at path (or root if null/undefined).
   *
   * @param path - Directory path, or null/undefined for the repo root.
   * @returns Array of entry names (files and subdirectories).
   * @throws {NotADirectoryError} If path is a file.
   */
  async ls(path?: string | null): Promise<string[]> {
    return listTreeAtPath(this._fsModule, this._gitdir, this._treeOid, path);
  }

  /**
   * Walk the repo tree recursively, like `os.walk`.
   *
   * Yields `[dirpath, dirnames, fileEntries]` tuples. Each file entry is a
   * WalkEntry with `name`, `oid`, and `mode`.
   *
   * @param path - Subtree to walk, or null/undefined for root.
   * @throws {NotADirectoryError} If path is a file.
   */
  async *walk(
    path?: string | null,
  ): AsyncGenerator<[string, string[], WalkEntry[]]> {
    if (path == null || isRootPath(path)) {
      yield* walkTree(this._fsModule, this._gitdir, this._treeOid);
    } else {
      const normalized = normalizePath(path);
      const entry = await walkTo(this._fsModule, this._gitdir, this._treeOid, normalized);
      if (entry.mode !== MODE_TREE) throw new NotADirectoryError(normalized);
      yield* walkTree(this._fsModule, this._gitdir, entry.oid, normalized);
    }
  }

  /**
   * Return true if path exists (file or directory).
   *
   * @param path - Path to check.
   */
  async exists(path: string): Promise<boolean> {
    return existsAtPath(this._fsModule, this._gitdir, this._treeOid, path);
  }

  /**
   * Return true if path is a directory (tree) in the repo.
   *
   * @param path - Path to check.
   */
  async isDir(path: string): Promise<boolean> {
    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) return false;
    return entry.mode === MODE_TREE;
  }

  /**
   * Return the FileType of path.
   *
   * Returns `'blob'`, `'executable'`, `'link'`, or `'tree'`.
   *
   * @param path - Path to check.
   * @throws {FileNotFoundError} If path does not exist.
   */
  async fileType(path: string): Promise<FileType> {
    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) throw new FileNotFoundError(normalized);
    return fileTypeFromMode(entry.mode);
  }

  /**
   * Return the size in bytes of the object at path.
   *
   * @param path - Path to check.
   * @returns Size in bytes.
   * @throws {FileNotFoundError} If path does not exist.
   */
  async size(path: string): Promise<number> {
    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) throw new FileNotFoundError(normalized);
    const { blob } = await git.readBlob({ fs: this._fsModule, gitdir: this._gitdir, oid: entry.oid });
    return blob.length;
  }

  /**
   * Return the 40-character hex SHA of the object at path.
   *
   * For files this is the blob SHA; for directories the tree SHA.
   *
   * @param path - Path to check.
   * @returns 40-char hex SHA string.
   * @throws {FileNotFoundError} If path does not exist.
   */
  async objectHash(path: string): Promise<string> {
    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) throw new FileNotFoundError(normalized);
    return entry.oid;
  }

  /**
   * Read the target of a symlink.
   *
   * @param path - Symlink path in the repo.
   * @returns The symlink target string.
   * @throws {FileNotFoundError} If path does not exist.
   * @throws {Error} If path is not a symlink.
   */
  async readlink(path: string): Promise<string> {
    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) throw new FileNotFoundError(normalized);
    if (entry.mode !== MODE_LINK) throw new Error(`Not a symlink: ${normalized}`);
    const { blob } = await git.readBlob({ fs: this._fsModule, gitdir: this._gitdir, oid: entry.oid });
    return new TextDecoder().decode(blob);
  }

  /**
   * Read raw blob data by hash, bypassing tree lookup.
   *
   * FUSE pattern: `stat()` -> cache hash -> `readByHash(hash)`.
   *
   * @param hash - 40-char hex SHA of the blob.
   * @param opts - Optional read options.
   * @param opts.offset - Byte offset to start reading from.
   * @param opts.size - Maximum bytes to return (undefined for all).
   * @returns Raw blob contents as Uint8Array.
   */
  async readByHash(hash: string, opts?: { offset?: number; size?: number }): Promise<Uint8Array> {
    const { blob } = await git.readBlob({ fs: this._fsModule, gitdir: this._gitdir, oid: hash });
    if (opts && (opts.offset !== undefined || opts.size !== undefined)) {
      const offset = opts.offset ?? 0;
      const end = opts.size !== undefined ? offset + opts.size : blob.length;
      return blob.subarray(offset, end);
    }
    return blob;
  }

  /**
   * Return a StatResult for path (or root if null/undefined).
   *
   * Combines fileType, size, oid, nlink, and mtime in a single call --
   * the hot path for FUSE `getattr`.
   *
   * @param path - Path to stat, or null/undefined for root.
   * @returns StatResult with mode, fileType, size, hash, nlink, and mtime.
   * @throws {FileNotFoundError} If path does not exist.
   */
  async stat(path?: string | null): Promise<StatResult> {
    const mtime = await this._getCommitTime();

    if (path == null || isRootPath(path)) {
      const nlink = 2 + await countSubdirs(this._fsModule, this._gitdir, this._treeOid);
      return {
        mode: modeToInt(MODE_TREE),
        fileType: fileTypeFromMode(MODE_TREE),
        size: 0,
        hash: this._treeOid,
        nlink,
        mtime,
      };
    }

    const normalized = normalizePath(path);
    const entry = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, normalized);
    if (entry === null) throw new FileNotFoundError(normalized);

    if (entry.mode === MODE_TREE) {
      const nlink = 2 + await countSubdirs(this._fsModule, this._gitdir, entry.oid);
      return {
        mode: modeToInt(entry.mode),
        fileType: fileTypeFromMode(entry.mode),
        size: 0,
        hash: entry.oid,
        nlink,
        mtime,
      };
    }

    const { blob } = await git.readBlob({ fs: this._fsModule, gitdir: this._gitdir, oid: entry.oid });
    return {
      mode: modeToInt(entry.mode),
      fileType: fileTypeFromMode(entry.mode),
      size: blob.length,
      hash: entry.oid,
      nlink: 1,
      mtime,
    };
  }

  /**
   * List directory entries with name, oid, and mode.
   *
   * Like `ls()` but returns WalkEntry objects so callers get entry types
   * (useful for FUSE `readdir` d_type).
   *
   * @param path - Directory path, or null/undefined for root.
   * @returns Array of WalkEntry objects.
   */
  async listdir(path?: string | null): Promise<WalkEntry[]> {
    return listEntriesAtPath(this._fsModule, this._gitdir, this._treeOid, path);
  }

  // ---------------------------------------------------------------------------
  // Glob
  // ---------------------------------------------------------------------------

  /**
   * Expand a glob pattern against the repo tree.
   *
   * Supports `*`, `?`, and `**`. `*` and `?` do not match a leading `.`
   * unless the pattern segment itself starts with `.`. `**` matches zero or
   * more directory levels, skipping directories whose names start with `.`.
   *
   * @param pattern - Glob pattern to match.
   * @returns Sorted, deduplicated list of matching paths (files and directories).
   */
  async glob(pattern: string): Promise<string[]> {
    const results: string[] = [];
    for await (const path of this.iglob(pattern)) {
      results.push(path);
    }
    return results.sort();
  }

  /**
   * Expand a glob pattern against the repo tree, yielding unique matches.
   *
   * Like `glob()` but returns an unordered async iterator instead of a
   * sorted list. Useful when you only need to iterate once and don't
   * need sorted output.
   *
   * A `/./` pivot marker (rsync `-R` style) is preserved in the output
   * so that callers can reconstruct partial source paths.
   *
   * @param pattern - Glob pattern to match.
   */
  async *iglob(pattern: string): AsyncGenerator<string> {
    pattern = pattern.replace(/^\/+|\/+$/g, '');
    if (!pattern) return;

    // Handle /./  pivot marker (rsync -R style)
    const pivotIdx = pattern.indexOf('/./');
    if (pivotIdx > 0) {
      const base = pattern.slice(0, pivotIdx);
      const rest = pattern.slice(pivotIdx + 3);
      const flat = rest ? `${base}/${rest}` : base;
      const basePrefix = base + '/';
      const seen = new Set<string>();
      for await (const path of this._iglobWalk(flat.split('/'), null, this._treeOid)) {
        if (!seen.has(path)) {
          seen.add(path);
          yield path.startsWith(basePrefix)
            ? `${base}/./${path.slice(basePrefix.length)}`
            : `${base}/./${path}`;
        }
      }
      return;
    }

    const seen = new Set<string>();
    for await (const path of this._iglobWalk(pattern.split('/'), null, this._treeOid)) {
      if (!seen.has(path)) {
        seen.add(path);
        yield path;
      }
    }
  }

  /** @internal */
  private async _iglobEntries(
    treeOid: string,
  ): Promise<Array<[string, boolean, string]>> {
    try {
      const { tree } = await git.readTree({ fs: this._fsModule, gitdir: this._gitdir, oid: treeOid });
      return tree.map((e) => [e.path, e.mode === MODE_TREE, e.oid] as [string, boolean, string]);
    } catch {
      return [];
    }
  }

  /** @internal */
  private async *_iglobWalk(
    segments: string[],
    prefix: string | null,
    treeOid: string,
  ): AsyncGenerator<string> {
    if (segments.length === 0) return;
    const seg = segments[0];
    const rest = segments.slice(1);

    if (seg === '**') {
      const entries = await this._iglobEntries(treeOid);
      if (rest.length > 0) {
        yield* this._iglobMatchEntries(rest, prefix, entries);
      } else {
        for (const [name, , ] of entries) {
          if (name.startsWith('.')) continue;
          yield prefix ? `${prefix}/${name}` : name;
        }
      }
      for (const [name, isDir, oid] of entries) {
        if (name.startsWith('.')) continue;
        const full = prefix ? `${prefix}/${name}` : name;
        if (isDir) {
          yield* this._iglobWalk(segments, full, oid); // keep **
        }
      }
      return;
    }

    const hasWild = seg.includes('*') || seg.includes('?');

    if (hasWild) {
      const entries = await this._iglobEntries(treeOid);
      for (const [name, isDir, oid] of entries) {
        if (!globMatch(seg, name)) continue;
        const full = prefix ? `${prefix}/${name}` : name;
        if (rest.length > 0) {
          if (isDir) yield* this._iglobWalk(rest, full, oid);
        } else {
          yield full;
        }
      }
    } else {
      // Literal segment — look up directly
      try {
        const { tree } = await git.readTree({ fs: this._fsModule, gitdir: this._gitdir, oid: treeOid });
        const entry = tree.find((e) => e.path === seg);
        if (!entry) return;
        const full = prefix ? `${prefix}/${seg}` : seg;
        if (rest.length > 0) {
          if (entry.mode === MODE_TREE) yield* this._iglobWalk(rest, full, entry.oid);
        } else {
          yield full;
        }
      } catch {
        return;
      }
    }
  }

  /** @internal */
  private async *_iglobMatchEntries(
    segments: string[],
    prefix: string | null,
    entries: Array<[string, boolean, string]>,
  ): AsyncGenerator<string> {
    const seg = segments[0];
    const rest = segments.slice(1);
    const hasWild = seg.includes('*') || seg.includes('?');

    if (hasWild) {
      for (const [name, , oid] of entries) {
        if (!globMatch(seg, name)) continue;
        const full = prefix ? `${prefix}/${name}` : name;
        if (rest.length > 0) {
          yield* this._iglobWalk(rest, full, oid);
        } else {
          yield full;
        }
      }
    } else {
      for (const [name, , oid] of entries) {
        if (name === seg) {
          const full = prefix ? `${prefix}/${seg}` : seg;
          if (rest.length > 0) {
            yield* this._iglobWalk(rest, full, oid);
          } else {
            yield full;
          }
          return;
        }
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Write operations
  // ---------------------------------------------------------------------------

  /**
   * @internal Build ChangeReport from writes and removes with type detection.
   */
  async _buildChanges(
    writes: Map<string, TreeWrite>,
    removes: Set<string>,
  ): Promise<ChangeReport> {
    const changes = emptyChangeReport();

    for (const [path, write] of writes) {
      const existing = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, path);
      if (existing !== null) {
        // Compare OID + mode to skip unchanged
        const newOid = write.oid ?? (write.data
          ? await git.writeBlob({ fs: this._fsModule, gitdir: this._gitdir, blob: write.data })
          : null);
        if (newOid === existing.oid && write.mode === existing.mode) continue;
        changes.update.push(fileEntryFromMode(path, write.mode));
      } else {
        changes.add.push(fileEntryFromMode(path, write.mode));
      }
    }

    for (const path of removes) {
      const existing = await entryAtPath(this._fsModule, this._gitdir, this._treeOid, path);
      if (existing) {
        changes.delete.push(fileEntryFromMode(path, existing.mode));
      } else {
        changes.delete.push({ path, type: 'blob' });
      }
    }

    return changes;
  }

  /**
   * @internal Commit changes: rebuild tree, create commit, update ref atomically.
   */
  async _commitChanges(
    writes: Map<string, TreeWrite>,
    removes: Set<string>,
    message?: string | null,
    operation?: string | null,
    parents?: FS[],
  ): Promise<FS> {
    if (!this._writable) throw this._readonlyError('write to');

    const changes = await this._buildChanges(writes, removes);
    const finalMessage = formatCommitMessage(changes, message, operation);

    const newTreeOid = await rebuildTree(
      this._fsModule,
      this._gitdir,
      this._treeOid,
      writes,
      removes,
    );

    // Atomic check-and-update under lock
    const refName = `refs/heads/${this._refName}`;
    const sig = this._store._signature;
    const committerStr = `${sig.name} <${sig.email}>`;
    const commitOid = this._commitOid;
    const store = this._store;

    const newCommitOid = await withRepoLock(this._fsModule, this._gitdir, async () => {
      // Check for stale snapshot
      const currentOid = await git.resolveRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
      });
      if (currentOid !== commitOid) {
        throw new StaleSnapshotError(
          `Branch '${this._refName}' has advanced since this snapshot`,
        );
      }

      if (newTreeOid === this._treeOid) {
        return null; // nothing changed
      }

      // Create commit
      const parentOids = [commitOid];
      if (parents) {
        for (const p of parents) {
          if (!p._commitOid) throw new Error('parent has no commit');
          parentOids.push(p._commitOid);
        }
      }
      const now = Math.floor(Date.now() / 1000);
      const oid = await git.writeCommit({
        fs: this._fsModule,
        gitdir: this._gitdir,
        commit: {
          message: finalMessage + '\n',
          tree: newTreeOid,
          parent: parentOids,
          author: { name: sig.name, email: sig.email, timestamp: now, timezoneOffset: 0 },
          committer: { name: sig.name, email: sig.email, timestamp: now, timezoneOffset: 0 },
        },
      });

      // Update ref
      await git.writeRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
        value: oid,
        force: true,
      });

      // Write reflog entry
      await writeReflogEntry(
        this._fsModule,
        this._gitdir,
        refName,
        commitOid,
        oid,
        committerStr,
        `commit: ${finalMessage}`,
      );

      return oid;
    });

    if (newCommitOid === null) return this; // nothing changed

    const newFs = new FS(store, newCommitOid, newTreeOid, this._refName, this._writable);
    newFs._changes = changes;
    return newFs;
  }

  /**
   * Write data to path and commit, returning a new FS.
   *
   * @param path - Destination path in the repo.
   * @param data - Raw bytes to write.
   * @param opts - Optional write options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.mode - File mode override (e.g. `'executable'`).
   * @returns New FS snapshot with the write committed.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async write(
    path: string,
    data: Uint8Array,
    opts?: { message?: string; mode?: FileType | string; parents?: FS[] },
  ): Promise<FS> {
    const normalized = normalizePath(path);
    const mode = opts?.mode
      ? resolveMode(opts.mode)
      : MODE_BLOB;
    const writes = new Map<string, TreeWrite>([[normalized, { data, mode }]]);
    return this._commitChanges(writes, new Set(), opts?.message, undefined, opts?.parents);
  }

  /**
   * Write text to path and commit, returning a new FS.
   *
   * @param path - Destination path in the repo.
   * @param text - String content (encoded as UTF-8).
   * @param opts - Optional write options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.mode - File mode override (e.g. `'executable'`).
   * @returns New FS snapshot with the write committed.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async writeText(
    path: string,
    text: string,
    opts?: { message?: string; mode?: FileType | string; parents?: FS[] },
  ): Promise<FS> {
    const data = new TextEncoder().encode(text);
    return this.write(path, data, opts);
  }

  /**
   * Write a local file into the repo and commit, returning a new FS.
   *
   * Executable permission is auto-detected from disk unless `mode` is set.
   *
   * @param path - Destination path in the repo.
   * @param localPath - Path to the local file.
   * @param opts - Optional write options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.mode - File mode override (e.g. `'executable'`).
   * @returns New FS snapshot with the write committed.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async writeFromFile(
    path: string,
    localPath: string,
    opts?: { message?: string; mode?: FileType | string; parents?: FS[] },
  ): Promise<FS> {
    const normalized = normalizePath(path);
    const detectedMode = await modeFromDisk(this._fsModule, localPath);
    const mode = opts?.mode
      ? resolveMode(opts.mode)
      : detectedMode;
    const data = (await this._fsModule.promises.readFile(localPath)) as Uint8Array;
    const blobOid = await git.writeBlob({ fs: this._fsModule, gitdir: this._gitdir, blob: data });
    const writes = new Map<string, TreeWrite>([[normalized, { oid: blobOid, mode }]]);
    return this._commitChanges(writes, new Set(), opts?.message, undefined, opts?.parents);
  }

  /**
   * Create a symbolic link entry and commit, returning a new FS.
   *
   * @param path - Symlink path in the repo.
   * @param target - The symlink target string.
   * @param opts - Optional write options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @returns New FS snapshot with the symlink committed.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async writeSymlink(
    path: string,
    target: string,
    opts?: { message?: string; parents?: FS[] },
  ): Promise<FS> {
    const normalized = normalizePath(path);
    const data = new TextEncoder().encode(target);
    const writes = new Map<string, TreeWrite>([[normalized, { data, mode: MODE_LINK }]]);
    return this._commitChanges(writes, new Set(), opts?.message, undefined, opts?.parents);
  }

  /**
   * Apply multiple writes and removes in a single atomic commit.
   *
   * `writes` maps repo paths to content. Values may be:
   * - `Uint8Array` -- raw blob data
   * - `string` -- UTF-8 text (encoded automatically)
   * - `WriteEntry` -- full control over source, mode, and symlinks
   *
   * `removes` lists repo paths to delete (string, array, or Set).
   *
   * @param writes - Map of repo paths to content.
   * @param removes - Path(s) to delete.
   * @param opts - Optional apply options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.operation - Operation name for auto-generated messages.
   * @returns New FS snapshot with the changes committed.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async apply(
    writes?: Record<string, WriteEntry | Uint8Array | string> | null,
    removes?: string | string[] | Set<string> | null,
    opts?: { message?: string; operation?: string; parents?: FS[] },
  ): Promise<FS> {
    const internalWrites = new Map<string, TreeWrite>();

    for (const [path, value] of Object.entries(writes ?? {})) {
      const normalized = normalizePath(path);

      // Normalize to WriteEntry
      let entry: WriteEntry;
      if (value instanceof Uint8Array) {
        entry = { data: value };
      } else if (typeof value === 'string') {
        entry = { data: value };
      } else if (typeof value === 'object' && value !== null) {
        entry = value as WriteEntry;
      } else {
        throw new TypeError(
          `Expected WriteEntry, Uint8Array, or string for '${path}', got ${typeof value}`
        );
      }

      if (entry.target != null) {
        // Symlink
        const data = new TextEncoder().encode(entry.target);
        const blobOid = await git.writeBlob({
          fs: this._fsModule,
          gitdir: this._gitdir,
          blob: data,
        });
        internalWrites.set(normalized, { oid: blobOid, mode: MODE_LINK });
      } else if (entry.data != null) {
        const data =
          typeof entry.data === 'string'
            ? new TextEncoder().encode(entry.data)
            : entry.data;
        const mode = entry.mode
          ? resolveMode(entry.mode)
          : MODE_BLOB;
        internalWrites.set(normalized, { data, mode });
      }
    }

    // Normalize removes
    let removeSet: Set<string>;
    if (removes == null) {
      removeSet = new Set();
    } else if (typeof removes === 'string') {
      removeSet = new Set([normalizePath(removes)]);
    } else if (removes instanceof Set) {
      removeSet = new Set([...removes].map(normalizePath));
    } else {
      removeSet = new Set(removes.map(normalizePath));
    }

    return this._commitChanges(internalWrites, removeSet, opts?.message, opts?.operation, opts?.parents);
  }

  /**
   * Return a Batch for accumulating multiple writes in one commit.
   *
   * @param opts - Optional batch options.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.operation - Operation name for auto-generated messages.
   * @returns A Batch instance. Call `batch.commit()` to finalize.
   * @throws {PermissionError} If this snapshot is read-only.
   */
  batch(opts?: { message?: string; operation?: string; parents?: FS[] }): Batch {
    return new Batch(this, opts?.message, opts?.operation, opts?.parents);
  }

  /**
   * Return a buffered writer that commits on close.
   *
   * Accepts `Uint8Array` or `string` via `write()`. Strings are UTF-8 encoded.
   *
   * @param path - Destination path in the repo.
   * @returns An FsWriter instance. Call `close()` to flush and commit.
   * @throws {PermissionError} If this snapshot is read-only.
   */
  writer(path: string): FsWriter {
    if (!this._writable) throw this._readonlyError('write to');
    return new FsWriter(this, path);
  }

  // ---------------------------------------------------------------------------
  // Copy / Sync / Remove / Move (delegates to copy module)
  // ---------------------------------------------------------------------------

  /**
   * Copy local files into the repo.
   *
   * Sources must be literal paths; use `diskGlob()` to expand patterns
   * before calling.
   *
   * @param sources - Local path(s). Trailing `/` copies contents; `/./` is a pivot marker.
   * @param dest - Destination path in the repo.
   * @param opts - Copy-in options.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.followSymlinks - Dereference symlinks on disk.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.mode - Override file mode for all files.
   * @param opts.ignoreExisting - Skip files that already exist at dest.
   * @param opts.delete - Remove repo files under dest not in source.
   * @param opts.ignoreErrors - Collect errors instead of aborting.
   * @param opts.checksum - Compare by content hash (default true).
   * @returns New FS with `.changes` set.
   * @throws {PermissionError} If this snapshot is read-only.
   */
  async copyIn(
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
      exclude?: import('./exclude.js').ExcludeFilter;
      parents?: FS[];
    } = {},
  ): Promise<FS> {
    const { copyIn } = await import('./copy.js');
    return copyIn(this, sources, dest, opts);
  }

  /**
   * Copy repo files to local disk.
   *
   * Sources must be literal repo paths; use `glob()` to expand patterns
   * before calling.
   *
   * @param sources - Repo path(s). Trailing `/` copies contents; `/./` is a pivot marker.
   * @param dest - Local destination directory.
   * @param opts - Copy-out options.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.ignoreExisting - Skip files that already exist at dest.
   * @param opts.delete - Remove local files under dest not in source.
   * @param opts.ignoreErrors - Collect errors instead of aborting.
   * @param opts.checksum - Compare by content hash (default true).
   * @returns This FS with `.changes` set.
   */
  async copyOut(
    sources: string | string[],
    dest: string,
    opts: {
      dryRun?: boolean;
      ignoreExisting?: boolean;
      delete?: boolean;
      ignoreErrors?: boolean;
      checksum?: boolean;
    } = {},
  ): Promise<FS> {
    const { copyOut } = await import('./copy.js');
    return copyOut(this, sources, dest, opts);
  }

  /**
   * Make repoPath identical to localPath (including deletes).
   *
   * @param localPath - Local directory to sync from.
   * @param repoPath - Repo directory to sync to.
   * @param opts - Sync-in options.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @param opts.ignoreErrors - Collect errors instead of aborting.
   * @param opts.checksum - Compare by content hash (default true).
   * @returns New FS with `.changes` set.
   * @throws {PermissionError} If this snapshot is read-only.
   */
  async syncIn(
    localPath: string,
    repoPath: string,
    opts: {
      dryRun?: boolean;
      message?: string;
      ignoreErrors?: boolean;
      checksum?: boolean;
      exclude?: import('./exclude.js').ExcludeFilter;
      parents?: FS[];
    } = {},
  ): Promise<FS> {
    const { syncIn } = await import('./copy.js');
    return syncIn(this, localPath, repoPath, opts);
  }

  /**
   * Make localPath identical to repoPath (including deletes).
   *
   * @param repoPath - Repo directory to sync from.
   * @param localPath - Local directory to sync to.
   * @param opts - Sync-out options.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.ignoreErrors - Collect errors instead of aborting.
   * @param opts.checksum - Compare by content hash (default true).
   * @returns This FS with `.changes` set.
   */
  async syncOut(
    repoPath: string,
    localPath: string,
    opts: {
      dryRun?: boolean;
      ignoreErrors?: boolean;
      checksum?: boolean;
    } = {},
  ): Promise<FS> {
    const { syncOut } = await import('./copy.js');
    return syncOut(this, repoPath, localPath, opts);
  }

  /**
   * Remove files from the repo.
   *
   * Sources must be literal paths; use `glob()` to expand patterns before calling.
   *
   * @param sources - Repo path(s) to remove.
   * @param opts - Remove options.
   * @param opts.recursive - Allow removing directories.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @returns New FS with `.changes` set.
   * @throws {PermissionError} If this snapshot is read-only.
   * @throws {FileNotFoundError} If no source paths match.
   */
  async remove(
    sources: string | string[],
    opts: { recursive?: boolean; dryRun?: boolean; message?: string; parents?: FS[] } = {},
  ): Promise<FS> {
    const { remove } = await import('./copy.js');
    return remove(this, sources, opts);
  }

  /**
   * Move or rename files within the repo.
   *
   * Sources must be literal paths; use `glob()` to expand patterns before calling.
   *
   * @param sources - Repo path(s) to move.
   * @param dest - Destination path in the repo.
   * @param opts - Move options.
   * @param opts.recursive - Allow moving directories.
   * @param opts.dryRun - Preview only; returned FS has `.changes` set.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @returns New FS with `.changes` set.
   * @throws {PermissionError} If this snapshot is read-only.
   */
  async move(
    sources: string | string[],
    dest: string,
    opts: { recursive?: boolean; dryRun?: boolean; message?: string; parents?: FS[] } = {},
  ): Promise<FS> {
    const { move } = await import('./copy.js');
    return move(this, sources, dest, opts);
  }

  /**
   * Copy files from source FS into this branch in a single atomic commit.
   *
   * Follows the same rsync trailing-slash conventions as `copyIn`/`copyOut`:
   *
   * - `"config"` → directory mode — copies `config/` *as* `config/` under dest.
   * - `"config/"` → contents mode — pours the *contents* of `config/` into dest.
   * - `"file.txt"` → file mode — copies the single file into dest.
   * - `""` or `"/"` → root contents mode — copies everything.
   *
   * Since both snapshots share the same object store, blobs are referenced
   * by OID — no data is read into memory regardless of file size.
   *
   * @param source - Any FS (branch, tag, detached commit). Read-only; not modified.
   * @param sources - Source path(s) in source. Accepts a single string or array. Defaults to `""` (root).
   * @param dest - Destination path in this branch. Defaults to `""` (root).
   * @param opts - Copy-ref options.
   * @param opts.delete - Remove dest files under the target that aren't in source.
   * @param opts.dryRun - Compute changes but don't commit. Returned FS has `.changes` set.
   * @param opts.message - Commit message (auto-generated if omitted).
   * @returns New FS for the dest branch with the commit applied.
   * @throws {Error} If source belongs to a different repo.
   * @throws {PermissionError} If this FS is read-only.
   */
  async copyFromRef(
    source: FS | string,
    sources?: string | string[],
    dest?: string,
    opts?: { delete?: boolean; dryRun?: boolean; message?: string; parents?: FS[] },
  ): Promise<FS> {
    if (!this._writable) throw this._readonlyError('write to');

    // Resolve string to FS
    if (typeof source === 'string') {
      const name = source;
      try {
        source = await this._store.branches.get(name);
      } catch {
        try {
          source = await this._store.tags.get(name);
        } catch {
          throw new Error(`Cannot resolve '${name}': not a branch or tag`);
        }
      }
    }

    // Validate same repo
    const selfPath = this._fsModule.realpathSync(this._gitdir);
    const srcFsPath = this._fsModule.realpathSync(source._gitdir);
    if (selfPath !== srcFsPath) {
      throw new Error('source must belong to the same repo as self');
    }

    // Normalize sources to list
    const sourcesList: string[] = sources === undefined || sources === null
      ? ['']
      : typeof sources === 'string' ? [sources] : sources;

    // Normalize dest
    const destNorm = dest !== undefined && dest !== null && dest !== ''
      ? (isRootPath(dest) ? '' : normalizePath(dest))
      : '';

    const { resolveRepoSources, walkRepo } = await import('./copy.js');

    // Resolve sources using rsync conventions
    const resolved = await resolveRepoSources(source, sourcesList);

    // Enumerate source files → Map<dest_path, {oid, mode}>
    const srcMapped = new Map<string, { oid: string; mode: string }>();

    for (const { repoPath, mode, prefix } of resolved) {
      const _dest = [destNorm, prefix].filter(Boolean).join('/');

      if (mode === 'file') {
        const name = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
        const destFile = _dest ? `${_dest}/${name}` : name;
        const entry = await entryAtPath(this._fsModule, this._gitdir, source._treeOid!, repoPath);
        if (entry) {
          srcMapped.set(normalizePath(destFile), { oid: entry.oid, mode: entry.mode });
        }
      } else if (mode === 'dir') {
        const dirName = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
        const target = _dest ? `${_dest}/${dirName}` : dirName;
        for await (const [dirpath, , files] of source.walk(repoPath)) {
          for (const fe of files) {
            const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
            const rel = repoPath && storePath.startsWith(repoPath + '/')
              ? storePath.slice(repoPath.length + 1)
              : storePath;
            srcMapped.set(normalizePath(`${target}/${rel}`), { oid: fe.oid, mode: fe.mode });
          }
        }
      } else {
        // contents
        const walkPath = repoPath || null;
        for await (const [dirpath, , files] of source.walk(walkPath)) {
          for (const fe of files) {
            const storePath = dirpath ? `${dirpath}/${fe.name}` : fe.name;
            const rel = repoPath && storePath.startsWith(repoPath + '/')
              ? storePath.slice(repoPath.length + 1)
              : storePath;
            const destFile = _dest ? `${_dest}/${rel}` : rel;
            srcMapped.set(normalizePath(destFile), { oid: fe.oid, mode: fe.mode });
          }
        }
      }
    }

    // Determine dest subtree(s) to walk for diff/delete
    const destPrefixes = new Set<string>();
    for (const { repoPath, mode, prefix } of resolved) {
      const _dest = [destNorm, prefix].filter(Boolean).join('/');
      if (mode === 'dir') {
        const dirName = repoPath.includes('/') ? repoPath.split('/').pop()! : repoPath;
        destPrefixes.add(_dest ? `${_dest}/${dirName}` : dirName);
      } else {
        destPrefixes.add(_dest);
      }
    }

    const destFiles = new Map<string, { oid: string; mode: string }>();
    for (const dp of destPrefixes) {
      const walked = await walkRepo(this, dp);
      for (const [rel, entry] of walked) {
        const full = dp ? `${dp}/${rel}` : rel;
        destFiles.set(full, entry);
      }
    }

    // Build writes and removes
    const writes = new Map<string, TreeWrite>();
    const removes = new Set<string>();

    for (const [destPath, srcEntry] of srcMapped) {
      const destEntry = destFiles.get(destPath);
      if (!destEntry || destEntry.oid !== srcEntry.oid || destEntry.mode !== srcEntry.mode) {
        writes.set(destPath, { oid: srcEntry.oid, mode: srcEntry.mode });
      }
    }

    if (opts?.delete) {
      for (const full of destFiles.keys()) {
        if (!srcMapped.has(full)) {
          removes.add(full);
        }
      }
    }

    if (opts?.dryRun) {
      const changes = await this._buildChanges(writes, removes);
      this._changes = finalizeChanges(changes);
      return this;
    }

    return this._commitChanges(writes, removes, opts?.message, 'cp', opts?.parents);
  }

  // ---------------------------------------------------------------------------
  // History
  // ---------------------------------------------------------------------------

  /**
   * The parent snapshot, or `null` for the initial commit.
   *
   * @returns The parent FS, or `null` if this is the initial commit.
   */
  async getParent(): Promise<FS | null> {
    const { commit } = await git.readCommit({
      fs: this._fsModule,
      gitdir: this._gitdir,
      oid: this._commitOid,
    });
    if (!commit.parent || commit.parent.length === 0) return null;
    return FS._fromCommit(this._store, commit.parent[0], this._refName, this._writable);
  }

  /**
   * Return the FS at the n-th ancestor commit.
   *
   * @param n - Number of commits to go back (default 1).
   * @returns FS at the ancestor commit.
   * @throws {Error} If n < 0 or history is too short.
   */
  async back(n = 1): Promise<FS> {
    if (n < 0) throw new Error(`back() requires n >= 0, got ${n}`);
    let fs: FS = this;
    for (let i = 0; i < n; i++) {
      const p = await fs.getParent();
      if (p === null) throw new Error(`Cannot go back ${n} commits - history too short`);
      fs = p;
    }
    return fs;
  }

  /**
   * Move branch back N commits.
   *
   * Walks back through parent commits and updates the branch pointer.
   * Automatically writes a reflog entry.
   *
   * @param steps - Number of commits to undo (default 1).
   * @returns New FS snapshot at the ancestor commit.
   * @throws {PermissionError} If called on a read-only snapshot (tag).
   * @throws {Error} If not enough history exists.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async undo(steps = 1): Promise<FS> {
    if (steps < 1) throw new Error(`steps must be >= 1, got ${steps}`);
    if (!this._writable) throw this._readonlyError('undo');

    let current: FS = this;
    for (let i = 0; i < steps; i++) {
      const parent = await current.getParent();
      if (parent === null) {
        throw new Error(`Cannot undo ${steps} steps - only ${i} commit(s) in history`);
      }
      current = parent;
    }

    const refName = `refs/heads/${this._refName}`;
    const sig = this._store._signature;
    const committerStr = `${sig.name} <${sig.email}>`;
    const myOid = this._commitOid;

    await withRepoLock(this._fsModule, this._gitdir, async () => {
      const currentOid = await git.resolveRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
      });
      if (currentOid !== myOid) {
        throw new StaleSnapshotError(
          `Branch '${this._refName}' has advanced since this snapshot`,
        );
      }
      await git.writeRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
        value: current._commitOid,
        force: true,
      });
      await writeReflogEntry(
        this._fsModule,
        this._gitdir,
        refName,
        myOid,
        current._commitOid,
        committerStr,
        'undo: move back',
      );
    });

    return current;
  }

  /**
   * Move branch forward N steps using reflog.
   *
   * Reads the reflog to find where the branch was before the last N movements.
   * This can resurrect "orphaned" commits after undo.
   *
   * @param steps - Number of reflog entries to go back (default 1).
   * @returns New FS snapshot at the target position.
   * @throws {PermissionError} If called on a read-only snapshot (tag).
   * @throws {Error} If not enough redo history exists.
   * @throws {StaleSnapshotError} If the branch has advanced since this snapshot.
   */
  async redo(steps = 1): Promise<FS> {
    if (steps < 1) throw new Error(`steps must be >= 1, got ${steps}`);
    if (!this._writable) throw this._readonlyError('redo');

    const refName = `refs/heads/${this._refName}`;

    // Read reflog
    const entries = await readReflog(this._fsModule, this._gitdir, this._refName!);
    if (entries.length === 0) throw new Error('Reflog is empty');

    // Find current position in reflog
    let currentIndex: number | null = null;
    for (let i = entries.length - 1; i >= 0; i--) {
      if (entries[i].newSha === this._commitOid) {
        currentIndex = i;
        break;
      }
    }
    if (currentIndex === null) {
      throw new Error('Cannot redo - current commit not in reflog');
    }

    // Walk back through reflog entries to find target
    let targetSha = this._commitOid;
    let index = currentIndex;
    for (let step = 0; step < steps; step++) {
      if (index < 0) {
        throw new Error(`Cannot redo ${steps} steps - only ${step} step(s) available`);
      }
      targetSha = entries[index].oldSha;
      if (targetSha === ZERO_SHA) {
        throw new Error(
          `Cannot redo ${steps} step(s) - reaches branch creation point`,
        );
      }
      index--;
    }

    const targetFs = await FS._fromCommit(this._store, targetSha, this._refName, this._writable);
    const sig = this._store._signature;
    const committerStr = `${sig.name} <${sig.email}>`;
    const myOid = this._commitOid;

    await withRepoLock(this._fsModule, this._gitdir, async () => {
      const currentOid = await git.resolveRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
      });
      if (currentOid !== myOid) {
        throw new StaleSnapshotError(
          `Branch '${this._refName}' has advanced since this snapshot`,
        );
      }
      await git.writeRef({
        fs: this._fsModule,
        gitdir: this._gitdir,
        ref: refName,
        value: targetSha,
        force: true,
      });
      await writeReflogEntry(
        this._fsModule,
        this._gitdir,
        refName,
        myOid,
        targetSha,
        committerStr,
        'redo: move forward',
      );
    });

    return targetFs;
  }

  /**
   * Walk the commit history, yielding ancestor FS snapshots.
   *
   * All filters are optional and combine with AND.
   *
   * @param opts - Log filter options.
   * @param opts.path - Only yield commits that changed this file.
   * @param opts.match - Message pattern (`*`/`?` wildcards).
   * @param opts.before - Only yield commits on or before this time.
   */
  async *log(opts?: {
    path?: string;
    match?: string;
    before?: Date;
  }): AsyncGenerator<FS> {
    const filterPath = opts?.path ? normalizePath(opts.path) : null;
    const match = opts?.match ?? null;
    const before = opts?.before ?? null;
    let pastCutoff = false;
    let current: FS | null = this;

    while (current !== null) {
      if (!pastCutoff && before !== null) {
        const time = await current.getTime();
        if (time > before) {
          current = await current.getParent();
          continue;
        }
        pastCutoff = true;
      }

      if (filterPath !== null) {
        const currentEntry = await entryAtPath(
          this._fsModule,
          this._gitdir,
          current._treeOid,
          filterPath,
        );
        const parent = await current.getParent();
        const parentEntry = parent
          ? await entryAtPath(this._fsModule, this._gitdir, parent._treeOid, filterPath)
          : null;
        if (
          currentEntry?.oid === parentEntry?.oid &&
          currentEntry?.mode === parentEntry?.mode
        ) {
          current = parent;
          continue;
        }
      }

      if (match !== null) {
        const msg = await current.getMessage();
        if (!globMatch(match, msg)) {
          current = await current.getParent();
          continue;
        }
      }

      yield current;
      current = await current.getParent();
    }
  }

  // ---------------------------------------------------------------------------
  // Squash
  // ---------------------------------------------------------------------------

  /**
   * Create a new commit with this snapshot's tree but a fresh history.
   *
   * The returned FS is detached (not bound to any ref). To persist it,
   * assign it to a branch via `store.branches.set()`.
   *
   * @param opts - Optional squash options.
   * @param opts.parent - If provided, the squashed commit's parent will be this FS's commit.
   * @param opts.message - Commit message (default `"squash"`).
   * @returns A new detached FS with the squashed commit.
   */
  async squash(opts?: { parent?: FS; message?: string }): Promise<FS> {
    const msg = opts?.message ?? 'squash';
    const parents: string[] = [];
    if (opts?.parent) {
      if (!opts.parent._commitOid) throw new Error('parent has no commit');
      parents.push(opts.parent._commitOid);
    }

    const sig = this._store._signature;
    const now = Math.floor(Date.now() / 1000);

    const newOid = await git.writeCommit({
      fs: this._fsModule,
      gitdir: this._gitdir,
      commit: {
        message: msg.endsWith('\n') ? msg : msg + '\n',
        tree: this._treeOid,
        parent: parents,
        author: { name: sig.name, email: sig.email, timestamp: now, timezoneOffset: 0 },
        committer: { name: sig.name, email: sig.email, timestamp: now, timezoneOffset: 0 },
      },
    });

    return FS._fromCommit(this._store, newOid, null, false);
  }
}

// ---------------------------------------------------------------------------
// Standalone helpers
// ---------------------------------------------------------------------------

import { modeFromDisk } from './tree.js';

/**
 * Resolve a mode that may be a FileType name ('blob', 'executable', 'link')
 * or a git mode string ('100644', '100755', '120000').
 */
function resolveMode(mode: FileType | string): string {
  // Git mode strings are 6-digit octal like '100644'
  if (typeof mode === 'string' && /^\d{6}$/.test(mode)) return mode;
  return fileModeFromType(mode as FileType);
}

/**
 * Write data to a branch with automatic retry on concurrent modification.
 *
 * Re-fetches the branch FS on each attempt. Uses exponential backoff
 * with jitter (base 10ms, factor 2x, cap 200ms) to avoid thundering-herd.
 *
 * @param store - The GitStore instance.
 * @param branch - Branch name to write to.
 * @param path - Destination path in the repo.
 * @param data - Raw bytes to write.
 * @param opts - Optional retry-write options.
 * @param opts.message - Commit message (auto-generated if omitted).
 * @param opts.mode - File mode override (e.g. `'executable'`).
 * @param opts.retries - Maximum number of attempts (default 5).
 * @returns New FS snapshot with the write committed.
 * @throws {StaleSnapshotError} If all attempts are exhausted.
 * @throws {Error} If the branch does not exist.
 */
export async function retryWrite(
  store: GitStore,
  branch: string,
  path: string,
  data: Uint8Array,
  opts?: { message?: string; mode?: FileType | string; retries?: number },
): Promise<FS> {
  const retries = opts?.retries ?? 5;
  for (let attempt = 0; attempt < retries; attempt++) {
    const fs = await store.branches.get(branch);
    try {
      return await fs.write(path, data, opts);
    } catch (err) {
      if (err instanceof StaleSnapshotError) {
        if (attempt === retries - 1) throw err;
        const delay = Math.min(10 * 2 ** attempt, 200);
        await new Promise((r) => setTimeout(r, Math.random() * delay));
        continue;
      }
      throw err;
    }
  }
  throw new Error('unreachable');
}
