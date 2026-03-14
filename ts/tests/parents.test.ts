import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import git from 'isomorphic-git';
import { freshStore, toBytes, fromBytes, rmTmpDir, fs } from './helpers.js';
import { GitStore, FS } from '../src/index.js';

let store: GitStore;
let snap: FS;
let tmpDir: string;

beforeEach(async () => {
  const res = await freshStore();
  store = res.store;
  tmpDir = res.tmpDir;
  let f = await store.branches.get('main');
  snap = await f.write('a.txt', toBytes('a'));
});

afterEach(() => rmTmpDir(tmpDir));

async function readCommitParents(gitdir: string, oid: string): Promise<string[]> {
  const { commit } = await git.readCommit({ fs, gitdir, oid });
  return commit.parent;
}

describe('parents', () => {
  it('write with parents adds extra parent', async () => {
    // Create a second branch from snap to use as parent
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.write('b.txt', toBytes('b'));

    const f2 = await snap.write('c.txt', toBytes('c'), { parents: [otherFs2] });
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(2);
    expect(parents[0]).toBe(snap.commitHash);
    expect(parents[1]).toBe(otherFs2.commitHash);
  });

  it('writeText with parents', async () => {
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.writeText('b.txt', 'b');

    const f2 = await snap.writeText('c.txt', 'c', { parents: [otherFs2] });
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(2);
    expect(parents[1]).toBe(otherFs2.commitHash);
  });

  it('apply with parents', async () => {
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.write('b.txt', toBytes('b'));

    const f2 = await snap.apply(
      { 'c.txt': toBytes('c') },
      null,
      { parents: [otherFs2] },
    );
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(2);
    expect(parents[1]).toBe(otherFs2.commitHash);
  });

  it('batch with parents', async () => {
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.write('b.txt', toBytes('b'));

    const b = snap.batch({ parents: [otherFs2] });
    await b.write('c.txt', toBytes('c'));
    const f2 = await b.commit();
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(2);
    expect(parents[0]).toBe(snap.commitHash);
    expect(parents[1]).toBe(otherFs2.commitHash);
  });

  it('multiple extra parents', async () => {
    const otherFs1 = await store.branches.setAndGet('other1', snap);
    const otherFs1b = await otherFs1.write('b.txt', toBytes('b'));
    const otherFs2 = await store.branches.setAndGet('other2', snap);
    const otherFs2b = await otherFs2.write('d.txt', toBytes('d'));

    const f2 = await snap.write('c.txt', toBytes('c'), {
      parents: [otherFs1b, otherFs2b],
    });
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(3);
    expect(parents[0]).toBe(snap.commitHash);
    expect(parents[1]).toBe(otherFs1b.commitHash);
    expect(parents[2]).toBe(otherFs2b.commitHash);
  });

  it('no parents by default gives single parent', async () => {
    const f2 = await snap.write('c.txt', toBytes('c'));
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(1);
    expect(parents[0]).toBe(snap.commitHash);
  });

  it('first-parent lineage preserved with parents', async () => {
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.write('b.txt', toBytes('b'));

    const f2 = await snap.write('c.txt', toBytes('c'), { parents: [otherFs2] });
    // getParent walks first-parent lineage
    const parent = await f2.getParent();
    expect(parent!.commitHash).toBe(snap.commitHash);
  });

  it('writeSymlink with parents', async () => {
    const otherFs = await store.branches.setAndGet('other', snap);
    const otherFs2 = await otherFs.write('b.txt', toBytes('b'));

    const f2 = await snap.writeSymlink('link', 'a.txt', { parents: [otherFs2] });
    const parents = await readCommitParents(store._gitdir, f2.commitHash);
    expect(parents.length).toBe(2);
    expect(parents[1]).toBe(otherFs2.commitHash);
  });
});
