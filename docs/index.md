# vost Documentation

vost is a versioned key-value filesystem backed by bare Git repositories. Every write produces a new commit, writes of multiple files can be batched, and old snapshots remain accessible forever. The API is refreshingly simple, looking more like a filesystem interface than git. The Python CLI provides Unix-like utility with syntax borrowed from familiar programs like ls, cat, cp, and rsync. Under the hood, though, vost uses bare git repositories fully compatible with other git toolchains.

## Quick install

```
pip install vost            # core library
pip install "vost[cli]"    # adds the vost command-line tool
```

Requires Python 3.10+ and [dulwich](https://www.dulwich.io/).

## Hello world (Python)

```python
from vost import GitStore

repo = GitStore.open("data.git")
fs = repo.branches["main"]
fs = fs.write("hello.txt", b"Hello, world!")
print(fs.read("hello.txt"))  # b'Hello, world!'
```

## Hello world (CLI)

```bash
export VOST_REPO=data.git
echo "Hello, world!" | vost write hello.txt
vost cat hello.txt                # Hello, world!
vost ls                           # hello.txt
vost cp local-dir/ :backup        # copy a directory into the repo
vost log                          # commit history
```

## Reference docs

- [Python API Reference](api.md) -- classes, methods, and data types
- [CLI Reference](cli.md) -- the `vost` command-line tool
- [CLI Tutorial](cli-tutorial.md) -- learn the CLI step by step
- [Path Syntax](paths.md) -- how `ref:path` works across commands
- [fsspec Integration](fsspec.md) -- use vost with pandas, xarray, dask

## More

- [GitHub Repository](https://github.com/mhalle/vost) -- source code, issues, and releases
- [README](https://github.com/mhalle/vost#readme) -- core concepts, concurrency safety, error handling, and development setup
- [PyPI](https://pypi.org/project/vost/) -- `pip install vost`
