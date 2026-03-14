import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { freshStore, toBytes, rmTmpDir } from './helpers.js';
import { GitStore, FS } from '../src/index.js';

let store: GitStore;
let tmpDir: string;

beforeEach(async () => {
  const res = await freshStore();
  store = res.store;
  tmpDir = res.tmpDir;
});

afterEach(() => rmTmpDir(tmpDir));

describe('squash', () => {
  it('creates a root commit (no parents)', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('hello'));
    fs = await fs.write('b.txt', toBytes('world'));

    const squashed = await fs.squash();
    // Root commit has no parent
    const parent = await squashed.getParent();
    expect(parent).toBeNull();
  });

  it('preserves tree hash', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('hello'));
    fs = await fs.write('b.txt', toBytes('world'));

    const squashed = await fs.squash();
    expect(squashed.treeHash).toBe(fs.treeHash);

    // Content is identical
    expect(await squashed.readText('a.txt')).toBe('hello');
    expect(await squashed.readText('b.txt')).toBe('world');
  });

  it('squash with parent', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('v1'));

    const parentFs = await fs.squash({ message: 'base' });

    fs = await fs.write('b.txt', toBytes('v2'));
    const squashed = await fs.squash({ parent: parentFs });

    const p = await squashed.getParent();
    expect(p).not.toBeNull();
    expect(p!.commitHash).toBe(parentFs.commitHash);

    // Grandparent should be null (parentFs is a root commit)
    const gp = await p!.getParent();
    expect(gp).toBeNull();
  });

  it('squash with custom message', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('data'));

    const squashed = await fs.squash({ message: 'custom squash message' });
    const msg = await squashed.getMessage();
    expect(msg).toBe('custom squash message');
  });

  it('default message is "squash"', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('data'));

    const squashed = await fs.squash();
    const msg = await squashed.getMessage();
    expect(msg).toBe('squash');
  });

  it('squashed commit can be assigned to a branch', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('hello'));
    fs = await fs.write('b.txt', toBytes('world'));

    const squashed = await fs.squash({ message: 'squashed history' });

    // Assign to a new branch
    await store.branches.set('squashed', squashed);
    const branchFs = await store.branches.get('squashed');

    expect(branchFs.treeHash).toBe(squashed.treeHash);
    expect(await branchFs.readText('a.txt')).toBe('hello');
    expect(await branchFs.readText('b.txt')).toBe('world');

    // The branch should have the squashed commit (root, no parent)
    const parent = await branchFs.getParent();
    expect(parent).toBeNull();
  });

  it('squashed FS is not writable (detached)', async () => {
    let fs = await store.branches.get('main');
    fs = await fs.write('a.txt', toBytes('data'));

    const squashed = await fs.squash();
    expect(squashed.writable).toBe(false);
    expect(squashed.refName).toBeNull();
  });
});
