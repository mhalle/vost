"""Tests for fsspec filesystem adapter."""

import pytest

fsspec = pytest.importorskip("fsspec")

from vost import GitStore
from vost._fsspec import VostFileSystem


@pytest.fixture
def repo_with_history(tmp_path):
    """Create a repo with a branch, a tag, and 2 commits of history."""
    repo = GitStore.open(tmp_path / "test.git")
    fs = repo.branches["main"]
    fs = fs.write("data.csv", b"a,b\n1,2\n")
    fs = fs.write("src/app.py", b"print('hello')")
    fs = fs.write("src/lib/util.py", b"# util")
    # fs is now at commit 4 (init + 3 writes)
    # Write again to create history
    fs = fs.write("data.csv", b"a,b\n1,2\n3,4\n")
    # Tag the previous commit
    repo.tags["v1.0"] = fs.back(1)
    return repo, fs, tmp_path / "test.git"


class TestReadCore:
    def test_ls_root(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        names = vfs.ls("/")
        assert "/data.csv" in names
        assert "/src" in names

    def test_ls_subdir(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        names = vfs.ls("/src")
        assert "/src/app.py" in names
        assert "/src/lib" in names

    def test_ls_detail(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        entries = vfs.ls("/", detail=True)
        by_name = {e["name"]: e for e in entries}
        assert by_name["/data.csv"]["type"] == "file"
        assert by_name["/data.csv"]["size"] > 0
        assert by_name["/src"]["type"] == "directory"
        assert by_name["/src"]["size"] == 0

    def test_info_file(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        info = vfs.info("/data.csv")
        assert info["type"] == "file"
        assert info["size"] == len(b"a,b\n1,2\n3,4\n")
        assert "sha" in info

    def test_info_dir(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        info = vfs.info("/src")
        assert info["type"] == "directory"

    def test_info_root(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        info = vfs.info("/")
        assert info["type"] == "directory"
        assert info["name"] == "/"

    def test_cat(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        data = vfs.cat("/data.csv")
        assert data == b"a,b\n1,2\n3,4\n"

    def test_cat_file_partial(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        data = vfs.cat_file("/data.csv", start=0, end=3)
        assert data == b"a,b"

    def test_cat_file_offset(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        data = vfs.cat_file("/data.csv", start=4, end=7)
        assert data == b"1,2"

    def test_open_read(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        with vfs.open("/data.csv", "rb") as f:
            assert f.read() == b"a,b\n1,2\n3,4\n"

    def test_exists(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        assert vfs.exists("/data.csv")
        assert vfs.exists("/src")
        assert not vfs.exists("/nope.txt")

    def test_isdir(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        assert vfs.isdir("/src")
        assert not vfs.isdir("/data.csv")

    def test_isfile(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        assert vfs.isfile("/data.csv")
        assert not vfs.isfile("/src")

    def test_walk(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        walked = list(vfs.walk("/"))
        # Should contain root and subdirectories
        dirs = [d for d, _, _ in walked]
        # Root is "" (empty string) in fsspec walk output
        assert "" in dirs
        assert any("src" in d for d in dirs)

    def test_glob(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        matches = vfs.glob("/src/*.py")
        assert "/src/app.py" in matches

    def test_glob_recursive(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        matches = vfs.glob("/src/**/*.py")
        assert "/src/app.py" in matches
        assert "/src/lib/util.py" in matches


class TestWrite:
    def test_pipe_file(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        vfs.pipe_file("/new.txt", b"new content")
        assert vfs.cat("/new.txt") == b"new content"

    def test_open_write(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        with vfs.open("/written.txt", "wb") as f:
            f.write(b"hello ")
            f.write(b"world")
        assert vfs.cat("/written.txt") == b"hello world"

    def test_rm(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        assert vfs.exists("/data.csv")
        vfs.rm("/data.csv")
        assert not vfs.exists("/data.csv")

    def test_rm_recursive(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        vfs.rm("/src", recursive=True)
        assert not vfs.exists("/src")

    def test_mkdir_noop(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        vfs.mkdir("/newdir")  # should not raise


class TestNavigation:
    def test_ref_branch(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n3,4\n"

    def test_ref_tag_readonly(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="v1.0")
        # Tag points to the commit before the last write
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n"

    def test_tag_write_raises(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="v1.0")
        with pytest.raises(PermissionError):
            vfs.pipe_file("/x.txt", b"nope")

    def test_tag_open_write_raises(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="v1.0")
        with pytest.raises(PermissionError):
            vfs.open("/x.txt", "wb")

    def test_ref_commit_hash(self, repo_with_history):
        _, fs, path = repo_with_history
        sha = fs.commit_hash
        vfs = VostFileSystem(repo=str(path), ref=sha)
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n3,4\n"

    def test_back(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main", back=1)
        # back=1 from latest commit → previous data.csv content
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n"

    def test_default_ref(self, repo_with_history):
        _, _, path = repo_with_history
        # No ref → uses current (HEAD) branch
        vfs = VostFileSystem(repo=str(path))
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n3,4\n"


class TestErrors:
    def test_missing_file(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main")
        with pytest.raises(FileNotFoundError):
            vfs.cat("/nope.txt")

    def test_bad_ref(self, tmp_path):
        repo = GitStore.open(tmp_path / "test.git")
        with pytest.raises(ValueError, match="ref not found"):
            VostFileSystem(repo=str(tmp_path / "test.git"), ref="nonexistent")

    def test_repo_not_found(self, tmp_path):
        with pytest.raises(FileNotFoundError):
            VostFileSystem(repo=str(tmp_path / "missing.git"))

    def test_rm_readonly_raises(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="v1.0")
        with pytest.raises(PermissionError):
            vfs.rm("/data.csv")


class TestReadonly:
    def test_readonly_blocks_pipe(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main", readonly=True)
        with pytest.raises(PermissionError, match="readonly=True"):
            vfs.pipe_file("/x.txt", b"nope")

    def test_readonly_blocks_open_write(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main", readonly=True)
        with pytest.raises(PermissionError, match="readonly=True"):
            vfs.open("/x.txt", "wb")

    def test_readonly_blocks_rm(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main", readonly=True)
        with pytest.raises(PermissionError, match="readonly=True"):
            vfs.rm("/data.csv")

    def test_readonly_allows_reads(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = VostFileSystem(repo=str(path), ref="main", readonly=True)
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n3,4\n"
        assert vfs.exists("/data.csv")
        assert vfs.ls("/")


class TestEntryPoint:
    def test_filesystem_factory(self, repo_with_history):
        _, _, path = repo_with_history
        vfs = fsspec.filesystem("vost", repo=str(path), ref="main")
        assert isinstance(vfs, VostFileSystem)
        assert vfs.cat("/data.csv") == b"a,b\n1,2\n3,4\n"
