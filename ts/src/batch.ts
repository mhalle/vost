/**
 * Batch: accumulates writes and removes, commits once on commit().
 */

import git from 'isomorphic-git';
import {
  MODE_BLOB,
  MODE_LINK,
  MODE_TREE,
  FileNotFoundError,
  IsADirectoryError,
  PermissionError,
  BatchClosedError,
  type FsModule,
} from './types.js';
import { normalizePath } from './paths.js';
import { modeFromDisk, walkTo, existsAtPath, type TreeWrite } from './tree.js';

import { BatchWriter } from './fileobj.js';
import type { FS } from './fs.js';
import type { GitStore } from './gitstore.js';

/**
 * Accumulates writes and removes, then commits all changes atomically
 * when `commit()` is called.
 *
 * Nothing is committed if the batch is not explicitly committed.
 * Use `commit()` to finalize all staged changes in a single commit.
 */
export class Batch {
  private _fs: FS;
  private _store: GitStore;
  private _fsModule: FsModule;
  private _gitdir: string;
  private _message: string | null;
  private _operation: string | null;
  private _parents: FS[] | undefined;
  private _writes = new Map<string, TreeWrite>();
  private _removes = new Set<string>();
  private _closed = false;

  /** The resulting FS snapshot after commit. Null until commit() completes. */
  fs: FS | null = null;

  constructor(fs: FS, message?: string | null, operation?: string | null, parents?: FS[]) {
    if (!fs._writable) {
      throw new PermissionError('Cannot batch on a read-only snapshot');
    }
    this._fs = fs;
    this._store = fs._store;
    this._fsModule = fs._store._fsModule;
    this._gitdir = fs._store._gitdir;
    this._message = message ?? null;
    this._operation = operation ?? null;
    this._parents = parents;
  }

  private _checkOpen(): void {
    if (this._closed) throw new BatchClosedError('Batch is closed');
  }

  /**
   * Stage a file write. Creates the blob immediately in the object store.
   *
   * @param path - Destination path in the repo.
   * @param data - Raw bytes to write.
   * @param opts.mode - File mode override (e.g. MODE_BLOB_EXEC for executable).
   */
  async write(path: string, data: Uint8Array, opts?: { mode?: string }): Promise<void> {
    this._checkOpen();
    const normalized = normalizePath(path);
    this._removes.delete(normalized);
    const blobOid = await git.writeBlob({ fs: this._fsModule, gitdir: this._gitdir, blob: data });
    this._writes.set(normalized, {
      oid: blobOid,
      mode: opts?.mode ?? MODE_BLOB,
    });
  }

  /**
   * Stage a write from a local file. Reads the file and creates the blob.
   *
   * Executable permission is auto-detected from disk unless `opts.mode` is set.
   *
   * @param path - Destination path in the repo.
   * @param localPath - Path to the local file.
   * @param opts.mode - File mode override (e.g. MODE_BLOB_EXEC for executable).
   */
  async writeFromFile(
    path: string,
    localPath: string,
    opts?: { mode?: string },
  ): Promise<void> {
    this._checkOpen();
    const normalized = normalizePath(path);
    this._removes.delete(normalized);

    const detectedMode = await modeFromDisk(this._fsModule, localPath);
    const mode = opts?.mode ?? detectedMode;
    const data = (await this._fsModule.promises.readFile(localPath)) as Uint8Array;
    const blobOid = await git.writeBlob({ fs: this._fsModule, gitdir: this._gitdir, blob: data });
    this._writes.set(normalized, { oid: blobOid, mode });
  }

  /**
   * Stage a symbolic link entry.
   *
   * @param path - Symlink path in the repo.
   * @param target - The symlink target string.
   */
  async writeSymlink(path: string, target: string): Promise<void> {
    this._checkOpen();
    const normalized = normalizePath(path);
    this._removes.delete(normalized);
    const data = new TextEncoder().encode(target);
    const blobOid = await git.writeBlob({ fs: this._fsModule, gitdir: this._gitdir, blob: data });
    this._writes.set(normalized, { oid: blobOid, mode: MODE_LINK });
  }

  /**
   * Stage a file removal.
   *
   * @param path - Path to remove from the repo.
   * @throws {FileNotFoundError} If the path does not exist in the repo or pending writes.
   * @throws {IsADirectoryError} If the path is a directory.
   */
  async remove(path: string): Promise<void> {
    this._checkOpen();
    const normalized = normalizePath(path);
    const pendingWrite = this._writes.has(normalized);
    const existsInBase = await existsAtPath(
      this._fsModule,
      this._gitdir,
      this._fs._treeOid,
      normalized,
    );

    if (!pendingWrite && !existsInBase) {
      throw new FileNotFoundError(normalized);
    }

    // Don't allow removing directories
    if (existsInBase) {
      const entry = await walkTo(this._fsModule, this._gitdir, this._fs._treeOid, normalized);
      if (entry.mode === MODE_TREE) {
        throw new IsADirectoryError(normalized);
      }
    }

    this._writes.delete(normalized);
    if (existsInBase) {
      this._removes.add(normalized);
    }
  }

  /**
   * Return a buffered writer that stages to this batch on close.
   *
   * Accepts `Uint8Array` or `string` via `write()`. Strings are UTF-8 encoded.
   *
   * @param path - Destination path in the repo.
   * @returns A BatchWriter instance. Call `close()` to flush and stage.
   */
  writer(path: string): BatchWriter {
    this._checkOpen();
    return new BatchWriter(this, path);
  }

  /**
   * Commit all accumulated changes atomically.
   *
   * After calling this the batch is closed and no further writes are allowed.
   *
   * @returns The resulting FS snapshot.
   * @throws {StaleSnapshotError} If the branch has advanced since the snapshot.
   */
  async commit(): Promise<FS> {
    if (this._closed) throw new BatchClosedError('Batch is already committed');
    this._closed = true;

    if (this._writes.size === 0 && this._removes.size === 0) {
      this.fs = this._fs;
      return this._fs;
    }

    this.fs = await this._fs._commitChanges(
      this._writes,
      this._removes,
      this._message,
      this._operation,
      this._parents,
    );
    return this.fs;
  }
}
