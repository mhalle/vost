package vost

import org.eclipse.jgit.lib.Constants
import org.eclipse.jgit.lib.FileMode
import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption

/**
 * Internal copy/sync operations between disk and repo.
 */
internal object CopyOps {

    // ── Helpers ──────────────────────────────────────────────────────

    /**
     * Walk local files recursively, returning relative paths.
     */
    private fun walkLocalPaths(dir: String, exclude: ExcludeFilter? = null): Set<String> {
        val base = File(dir)
        if (!base.isDirectory) return emptySet()
        val result = mutableSetOf<String>()
        walkLocalRecursive(base, base, result, exclude)
        return result
    }

    private fun walkLocalRecursive(
        base: File,
        current: File,
        result: MutableSet<String>,
        exclude: ExcludeFilter? = null,
    ) {
        val entries = current.listFiles() ?: return
        for (f in entries) {
            val rel = f.relativeTo(base).path.replace(File.separatorChar, '/')
            if (f.isDirectory) {
                if (exclude != null && exclude.active && exclude.isExcluded(rel, isDir = true)) continue
                walkLocalRecursive(base, f, result, exclude)
            } else {
                if (exclude != null && exclude.active && exclude.isExcluded(rel, isDir = false)) continue
                result.add(rel)
            }
        }
    }

    /**
     * Walk repo files at a path, returning {relative_path: (oid_hex, mode)}.
     */
    private fun walkRepoFiles(fs: Fs, repoPath: String): Map<String, Pair<String, Int>> {
        val result = mutableMapOf<String, Pair<String, Int>>()
        val walkPath = repoPath.ifEmpty { null }
        try {
            for (entry in fs.walk(walkPath)) {
                for (file in entry.files) {
                    val storePath = if (entry.dirpath.isEmpty()) file.name else "${entry.dirpath}/${file.name}"
                    val rel = if (repoPath.isNotEmpty() && storePath.startsWith("$repoPath/")) {
                        storePath.removePrefix("$repoPath/")
                    } else {
                        storePath
                    }
                    result[rel] = Pair(file.oid, file.mode)
                }
            }
        } catch (_: Exception) {
            // Path doesn't exist
        }
        return result
    }

    /**
     * Resolve source paths with trailing slash conventions.
     * Returns list of (local_abs_path, repo_dest_path) pairs.
     */
    private fun resolveDiskToRepo(
        sources: List<String>,
        dest: String,
        exclude: ExcludeFilter? = null,
    ): List<Pair<String, String>> {
        val destNorm = if (dest.isNotEmpty()) normalizePath(dest.trimEnd('/')) else ""
        val pairs = mutableListOf<Pair<String, String>>()

        for (src in sources) {
            val isContents = src.endsWith("/")
            val srcPath = File(src.trimEnd('/'))
            if (!srcPath.exists()) throw java.io.FileNotFoundException(src)

            if (srcPath.isFile) {
                val repoPath = if (destNorm.isEmpty()) srcPath.name else "$destNorm/${srcPath.name}"
                pairs.add(Pair(srcPath.absolutePath, repoPath))
            } else if (srcPath.isDirectory) {
                if (isContents) {
                    // Contents mode: pour contents into dest
                    walkLocalRecursiveFiles(srcPath, exclude) { relPath ->
                        val repoPath = if (destNorm.isEmpty()) relPath else "$destNorm/$relPath"
                        pairs.add(Pair(File(srcPath, relPath).absolutePath, repoPath))
                    }
                } else {
                    // Directory mode: place dir inside dest
                    val dirName = srcPath.name
                    val targetBase = if (destNorm.isEmpty()) dirName else "$destNorm/$dirName"
                    walkLocalRecursiveFiles(srcPath, exclude) { relPath ->
                        pairs.add(Pair(File(srcPath, relPath).absolutePath, "$targetBase/$relPath"))
                    }
                }
            }
        }
        return pairs
    }

    private fun walkLocalRecursiveFiles(
        dir: File,
        exclude: ExcludeFilter? = null,
        handler: (String) -> Unit,
    ) {
        walkLocalFilesInner(dir, dir, exclude, handler)
    }

    private fun walkLocalFilesInner(
        base: File,
        current: File,
        exclude: ExcludeFilter? = null,
        handler: (String) -> Unit,
    ) {
        val entries = current.listFiles()?.sortedBy { it.name } ?: return
        for (f in entries) {
            val rel = f.relativeTo(base).path.replace(File.separatorChar, '/')
            if (f.isDirectory) {
                if (exclude != null && exclude.active && exclude.isExcluded(rel, isDir = true)) continue
                walkLocalFilesInner(base, f, exclude, handler)
            } else {
                if (exclude != null && exclude.active && exclude.isExcluded(rel, isDir = false)) continue
                handler(rel)
            }
        }
    }

    // ── Copy In ──────────────────────────────────────────────────────

    fun copyIn(
        fs: Fs,
        sources: List<String>,
        dest: String,
        message: String? = null,
        delete: Boolean = false,
        exclude: ExcludeFilter? = null,
        parents: List<Fs> = emptyList(),
    ): Fs {
        val pairs = resolveDiskToRepo(sources, dest, exclude)

        if (delete) {
            val destNorm = if (dest.isNotEmpty()) normalizePath(dest.trimEnd('/')) else ""
            val repoFiles = walkRepoFiles(fs, destNorm)
            val sourceRels = mutableSetOf<String>()
            for ((_, repoPath) in pairs) {
                val rel = if (destNorm.isNotEmpty() && repoPath.startsWith("$destNorm/")) {
                    repoPath.removePrefix("$destNorm/")
                } else {
                    repoPath
                }
                sourceRels.add(rel)
            }

            val batch = fs.batch(message = message, operation = "cp", parents = parents)

            // Write new/updated files
            for ((localPath, repoPath) in pairs) {
                val data = File(localPath).readBytes()
                batch.write(repoPath, data)
            }

            // Delete files not in source
            for (rel in repoFiles.keys) {
                if (rel !in sourceRels) {
                    val fullPath = if (destNorm.isEmpty()) rel else "$destNorm/$rel"
                    try {
                        batch.remove(fullPath)
                    } catch (_: Exception) {
                        // Ignore
                    }
                }
            }

            return batch.commit()
        } else {
            if (pairs.isEmpty()) return fs

            val batch = fs.batch(message = message, operation = "cp", parents = parents)
            for ((localPath, repoPath) in pairs) {
                val data = File(localPath).readBytes()
                batch.write(repoPath, data)
            }
            return batch.commit()
        }
    }

    // ── Copy Out ─────────────────────────────────────────────────────

    fun copyOut(
        fs: Fs,
        sources: List<String>,
        dest: String,
        delete: Boolean = false,
    ): Fs {
        val destDir = File(dest)
        destDir.mkdirs()

        val destNorm = dest.trimEnd('/')

        // Resolve repo sources to (repo_path, local_path) pairs
        val pairs = mutableListOf<Pair<String, String>>()
        for (src in sources) {
            val isContents = src.endsWith("/") || src.isEmpty()
            val stripped = src.trim('/')

            if (stripped.isEmpty()) {
                // Root contents: copy everything
                for (entry in fs.walk()) {
                    for (file in entry.files) {
                        val storePath = if (entry.dirpath.isEmpty()) file.name else "${entry.dirpath}/${file.name}"
                        pairs.add(Pair(storePath, "$destNorm/$storePath"))
                    }
                }
            } else if (isContents) {
                // Contents mode
                val srcNorm = normalizePath(stripped)
                if (!fs.exists(srcNorm)) throw java.io.FileNotFoundException(srcNorm)
                if (fs.isDir(srcNorm)) {
                    for (entry in fs.walk(srcNorm)) {
                        for (file in entry.files) {
                            val storePath = if (entry.dirpath.isEmpty()) file.name else "${entry.dirpath}/${file.name}"
                            val rel = storePath.removePrefix("$srcNorm/")
                            pairs.add(Pair(storePath, "$destNorm/$rel"))
                        }
                    }
                } else {
                    pairs.add(Pair(srcNorm, "$destNorm/${srcNorm.substringAfterLast('/')}"))
                }
            } else {
                val srcNorm = normalizePath(stripped)
                if (!fs.exists(srcNorm)) throw java.io.FileNotFoundException(srcNorm)
                if (fs.isDir(srcNorm)) {
                    val dirName = srcNorm.substringAfterLast('/')
                    for (entry in fs.walk(srcNorm)) {
                        for (file in entry.files) {
                            val storePath = if (entry.dirpath.isEmpty()) file.name else "${entry.dirpath}/${file.name}"
                            val rel = storePath.removePrefix("$srcNorm/")
                            pairs.add(Pair(storePath, "$destNorm/$dirName/$rel"))
                        }
                    }
                } else {
                    pairs.add(Pair(srcNorm, "$destNorm/${srcNorm.substringAfterLast('/')}"))
                }
            }
        }

        if (delete) {
            val localPaths = walkLocalPaths(dest)
            val sourceRels = mutableSetOf<String>()
            for ((_, localPath) in pairs) {
                val rel = File(localPath).relativeTo(destDir).path.replace(File.separatorChar, '/')
                sourceRels.add(rel)
            }

            // Delete local files not in source
            for (rel in localPaths) {
                if (rel !in sourceRels) {
                    val f = File(dest, rel)
                    if (f.exists()) f.delete()
                }
            }

            // Prune empty directories
            pruneEmptyDirs(destDir)
        }

        // Write files to disk
        for ((repoPath, localPath) in pairs) {
            val data = fs.read(repoPath)
            val outFile = File(localPath)
            outFile.parentFile?.mkdirs()
            outFile.writeBytes(data)

            // Preserve executable bit
            val mode = fs.fileType(repoPath)
            if (mode == FileType.EXECUTABLE) {
                outFile.setExecutable(true)
            }
        }

        return fs
    }

    // ── Sync In ──────────────────────────────────────────────────────

    fun syncIn(
        fs: Fs,
        localPath: String,
        repoPath: String,
        message: String? = null,
        exclude: ExcludeFilter? = null,
        parents: List<Fs> = emptyList(),
    ): Fs {
        val src = if (localPath.endsWith("/")) localPath else "$localPath/"
        return copyIn(fs, listOf(src), repoPath, message = message, delete = true, exclude = exclude, parents = parents)
    }

    // ── Sync Out ─────────────────────────────────────────────────────

    fun syncOut(
        fs: Fs,
        repoPath: String,
        localPath: String,
    ): Fs {
        val src = if (repoPath.endsWith("/")) repoPath else if (repoPath.isEmpty()) "" else "$repoPath/"
        return copyOut(fs, listOf(src), localPath, delete = true)
    }

    // ── Helpers ──────────────────────────────────────────────────────

    private fun pruneEmptyDirs(dir: File) {
        val entries = dir.listFiles() ?: return
        for (f in entries) {
            if (f.isDirectory) {
                pruneEmptyDirs(f)
                if (f.listFiles()?.isEmpty() == true) {
                    f.delete()
                }
            }
        }
    }
}
