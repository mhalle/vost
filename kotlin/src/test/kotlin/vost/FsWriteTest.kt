package vost

import org.eclipse.jgit.revwalk.RevWalk
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import java.io.FileNotFoundException
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class FsWriteTest {

    @Test
    fun `write creates file`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("hello.txt", "hello".toByteArray())
            assertEquals("hello", fs.readText("hello.txt"))
        }
    }

    @Test
    fun `write overwrites existing`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "v1".toByteArray())
            fs = fs.write("file.txt", "v2".toByteArray())
            assertEquals("v2", fs.readText("file.txt"))
        }
    }

    @Test
    fun `write preserves other files`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())
            assertEquals("a", fs.readText("a.txt"))
            assertEquals("b", fs.readText("b.txt"))
        }
    }

    @Test
    fun `writeText convenience`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.writeText("msg.txt", "Kotlin port")
            assertEquals("Kotlin port", fs.readText("msg.txt"))
        }
    }

    @Test
    fun `write with executable mode`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("script.sh", "#!/bin/bash".toByteArray(), mode = FileType.EXECUTABLE)
            assertEquals(FileType.EXECUTABLE, fs.fileType("script.sh"))
        }
    }

    @Test
    fun `writeSymlink creates symlink entry`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.writeSymlink("link", "target.txt")
            assertEquals(FileType.LINK, fs.fileType("link"))
            assertEquals("target.txt", fs.readlink("link"))
        }
    }

    @Test
    fun `write in nested directory`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a/b/c.txt", "nested".toByteArray())
            assertEquals("nested", fs.readText("a/b/c.txt"))
            assertTrue(fs.isDir("a"))
            assertTrue(fs.isDir("a/b"))
        }
    }

    @Test
    fun `write on readonly snapshot throws`() {
        val store = createStore()
        store.use {
            val fs = it.branches["main"]
            it.tags["v1"] = fs
            val tagFs = it.tags["v1"]
            assertThrows<PermissionError> {
                tagFs.write("file.txt", "data".toByteArray())
            }
        }
    }

    @Test
    fun `remove deletes file`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())
            fs = fs.remove(listOf("a.txt"))
            assertFalse(fs.exists("a.txt"))
            assertTrue(fs.exists("b.txt"))
        }
    }

    @Test
    fun `remove last file in directory removes directory`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("dir/only.txt", "data".toByteArray())
            fs = fs.remove(listOf("dir/only.txt"))
            assertFalse(fs.exists("dir"))
        }
    }

    @Test
    fun `apply with writes and removes`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())

            fs = fs.apply(
                writes = mapOf("c.txt" to "c".toByteArray()),
                removes = listOf("a.txt"),
            )
            assertFalse(fs.exists("a.txt"))
            assertTrue(fs.exists("b.txt"))
            assertEquals("c", fs.readText("c.txt"))
        }
    }

    @Test
    fun `apply with string values`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf("text.txt" to "hello" as Any))
            assertEquals("hello", fs.readText("text.txt"))
        }
    }

    @Test
    fun `write returns new commit hash`() {
        val store = createStore()
        store.use {
            val fs1 = it.branches["main"]
            val fs2 = fs1.write("file.txt", "data".toByteArray())
            assertNotEquals(fs1.commitHash, fs2.commitHash)
        }
    }

    @Test
    fun `write same content is no-op`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "data".toByteArray())
            val hash1 = fs.commitHash
            fs = fs.write("file.txt", "data".toByteArray())
            // No-op write should return same commit (tree unchanged)
            assertEquals(hash1, fs.commitHash)
        }
    }

    @Test
    fun `changes report on write`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("new.txt", "new".toByteArray())
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.add.size)
            assertEquals("new.txt", changes.add[0].path)
        }
    }

    @Test
    fun `changes report on update`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "v1".toByteArray())
            fs = fs.write("file.txt", "v2".toByteArray())
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.update.size)
        }
    }

    @Test
    fun `changes report on remove`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "data".toByteArray())
            fs = fs.remove(listOf("file.txt"))
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.delete.size)
        }
    }

    @Test
    fun `stale snapshot error`() {
        val store = createStore()
        store.use {
            val fs1 = it.branches["main"]
            val fs2 = it.branches["main"]

            // fs1 writes and advances branch
            fs1.write("a.txt", "a".toByteArray())

            // fs2 is stale now
            assertThrows<StaleSnapshotError> {
                fs2.write("b.txt", "b".toByteArray())
            }
        }
    }

    @Test
    fun `blob to tree transition`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            // Create a file at "dir"
            fs = fs.write("dir", "I am a file".toByteArray())
            assertFalse(fs.isDir("dir"))

            // Now write a file inside "dir" — this should convert it to a directory
            fs = fs.write("dir/file.txt", "inside".toByteArray())
            assertTrue(fs.isDir("dir"))
            assertEquals("inside", fs.readText("dir/file.txt"))
        }
    }

    @Test
    fun `writeFromFile reads local file into repo`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            val tmpFile = java.io.File.createTempFile("vost-test-", ".txt")
            try {
                tmpFile.writeText("hello from disk")
                fs = fs.writeFromFile("imported.txt", tmpFile.absolutePath)
                assertEquals("hello from disk", fs.readText("imported.txt"))
            } finally {
                tmpFile.delete()
            }
        }
    }

    @Test
    fun `writeFromFile auto-detects executable`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            val tmpFile = java.io.File.createTempFile("vost-test-", ".sh")
            try {
                tmpFile.writeText("#!/bin/sh\necho hello")
                tmpFile.setExecutable(true)
                fs = fs.writeFromFile("script.sh", tmpFile.absolutePath)
                assertEquals(FileType.EXECUTABLE, fs.fileType("script.sh"))
            } finally {
                tmpFile.delete()
            }
        }
    }

    @Test
    fun `writeFromFile with explicit mode`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            val tmpFile = java.io.File.createTempFile("vost-test-", ".txt")
            try {
                tmpFile.writeText("data")
                fs = fs.writeFromFile("exec.txt", tmpFile.absolutePath, mode = FileType.EXECUTABLE)
                assertEquals(FileType.EXECUTABLE, fs.fileType("exec.txt"))
            } finally {
                tmpFile.delete()
            }
        }
    }

    @Test
    fun `write symlink creates link`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("target.txt", "data".toByteArray())
            fs = fs.writeSymlink("link.txt", "target.txt")
            assertEquals(FileType.LINK, fs.fileType("link.txt"))
            assertEquals("target.txt", fs.readlink("link.txt"))
        }
    }

    @Test
    fun `write with explicit executable mode`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("script.sh", "#!/bin/sh".toByteArray(), mode = FileType.EXECUTABLE)
            assertEquals(FileType.EXECUTABLE, fs.fileType("script.sh"))
        }
    }

    @Test
    fun `remove multiple files`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())
            fs = fs.write("c.txt", "c".toByteArray())
            fs = fs.remove(listOf("a.txt", "b.txt"))
            assertFalse(fs.exists("a.txt"))
            assertFalse(fs.exists("b.txt"))
            assertTrue(fs.exists("c.txt"))
        }
    }

    // ── apply edge cases ──

    @Test
    fun `apply with bytes data`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            val data = byteArrayOf(0x00, 0xFF.toByte(), 0x42)
            fs = fs.apply(writes = mapOf("bin.dat" to data))
            val result = fs.read("bin.dat")
            assertEquals(3, result.size)
            assertEquals(0x00.toByte(), result[0])
            assertEquals(0xFF.toByte(), result[1])
            assertEquals(0x42.toByte(), result[2])
        }
    }

    @Test
    fun `apply with symlink entry`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf("target.txt" to "data",
                "link" to WriteEntry(target = "target.txt")))
            assertEquals(FileType.LINK, fs.fileType("link"))
            assertEquals("target.txt", fs.readlink("link"))
        }
    }

    @Test
    fun `apply with executable mode`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf(
                "script.sh" to WriteEntry(data = "#!/bin/sh".toByteArray(), mode = FileType.EXECUTABLE)
            ))
            assertEquals(FileType.EXECUTABLE, fs.fileType("script.sh"))
        }
    }

    @Test
    fun `apply multiple writes single commit`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf(
                "a.txt" to "aaa",
                "b.txt" to "bbb",
                "c/d.txt" to "ccc"
            ))
            assertEquals("aaa", fs.readText("a.txt"))
            assertEquals("bbb", fs.readText("b.txt"))
            assertEquals("ccc", fs.readText("c/d.txt"))
            // Single commit for all writes
            assertNotNull(fs.changes)
        }
    }

    @Test
    fun `apply removes single file`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())
            fs = fs.apply(removes = listOf("a.txt"))
            assertFalse(fs.exists("a.txt"))
            assertTrue(fs.exists("b.txt"))
        }
    }

    @Test
    fun `apply removes multiple files`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())
            fs = fs.write("c.txt", "c".toByteArray())
            fs = fs.apply(removes = listOf("a.txt", "b.txt"))
            assertFalse(fs.exists("a.txt"))
            assertFalse(fs.exists("b.txt"))
            assertTrue(fs.exists("c.txt"))
        }
    }

    @Test
    fun `apply empty is noop`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "data".toByteArray())
            val hash1 = fs.commitHash
            fs = fs.apply()
            // Empty apply should not create a new commit (same tree)
            assertEquals(hash1, fs.commitHash)
        }
    }

    @Test
    fun `apply identical write is noop`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "data".toByteArray())
            val hash1 = fs.commitHash
            fs = fs.apply(writes = mapOf("file.txt" to "data"))
            assertEquals(hash1, fs.commitHash)
        }
    }

    @Test
    fun `apply with custom message`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf("file.txt" to "data"), message = "custom apply")
            assertEquals("custom apply", fs.message)
        }
    }

    @Test
    fun `apply with operation keyword`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf(
                "a.txt" to "aaa",
                "b.txt" to "bbb"
            ), operation = "import")
            assertTrue(fs.message.startsWith("Batch import:"))
        }
    }

    @Test
    fun `apply on readonly tag throws`() {
        val store = createStore()
        store.use {
            val fs = it.branches["main"]
            it.tags["v1"] = fs
            val tagFs = it.tags["v1"]
            assertThrows<PermissionError> {
                tagFs.apply(writes = mapOf("file.txt" to "data"))
            }
        }
    }

    @Test
    fun `apply stale snapshot throws`() {
        val store = createStore()
        store.use {
            val fs1 = it.branches["main"]
            val fs2 = it.branches["main"]
            fs1.write("a.txt", "a".toByteArray())
            assertThrows<StaleSnapshotError> {
                fs2.apply(writes = mapOf("b.txt" to "b"))
            }
        }
    }

    @Test
    fun `apply changes report add`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.apply(writes = mapOf("new.txt" to "new"))
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.add.size)
            assertEquals("new.txt", changes.add[0].path)
        }
    }

    @Test
    fun `apply changes report update`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "v1".toByteArray())
            fs = fs.apply(writes = mapOf("file.txt" to "v2"))
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.update.size)
            assertEquals("file.txt", changes.update[0].path)
        }
    }

    @Test
    fun `apply changes report delete`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("file.txt", "data".toByteArray())
            fs = fs.apply(removes = listOf("file.txt"))
            val changes = fs.changes
            assertNotNull(changes)
            assertEquals(1, changes.delete.size)
            assertEquals("file.txt", changes.delete[0].path)
        }
    }

    // --- parents parameter tests ---

    private fun parentCount(store: GitStore, fs: Fs): Int {
        val revWalk = RevWalk(store.repo)
        try {
            val commit = revWalk.parseCommit(fs.commitId)
            return commit.parentCount
        } finally {
            revWalk.close()
        }
    }

    @Test
    fun `write with no parents has one parent`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "hello".toByteArray())
            assertEquals(1, parentCount(it, fs))
        }
    }

    @Test
    fun `write with parents creates merge commit`() {
        val store = createStore()
        store.use {
            // Create two branches
            var mainFs = it.branches["main"]
            mainFs = mainFs.write("a.txt", "main".toByteArray())

            it.branches["other"] = mainFs
            var otherFs = it.branches["other"]
            otherFs = otherFs.write("b.txt", "other".toByteArray())

            // Write on main with other as extra parent
            val merged = mainFs.write("c.txt", "merged".toByteArray(), parents = listOf(otherFs))
            assertEquals(2, parentCount(it, merged))

            // Verify content is accessible
            assertEquals("merged", String(merged.read("c.txt")))

            // Verify first parent is the previous main commit
            val revWalk = RevWalk(it.repo)
            try {
                val commit = revWalk.parseCommit(merged.commitId)
                assertEquals(mainFs.commitId, commit.getParent(0).id)
                assertEquals(otherFs.commitId, commit.getParent(1).id)
            } finally {
                revWalk.close()
            }
        }
    }

    @Test
    fun `writeText with parents creates merge commit`() {
        val store = createStore()
        store.use {
            var mainFs = it.branches["main"]
            mainFs = mainFs.writeText("a.txt", "main")

            it.branches["other"] = mainFs
            var otherFs = it.branches["other"]
            otherFs = otherFs.writeText("b.txt", "other")

            val merged = mainFs.writeText("c.txt", "merged", parents = listOf(otherFs))
            assertEquals(2, parentCount(it, merged))
        }
    }

    @Test
    fun `apply with parents creates merge commit`() {
        val store = createStore()
        store.use {
            var mainFs = it.branches["main"]
            mainFs = mainFs.write("a.txt", "main".toByteArray())

            it.branches["other"] = mainFs
            var otherFs = it.branches["other"]
            otherFs = otherFs.write("b.txt", "other".toByteArray())

            val merged = mainFs.apply(
                writes = mapOf("c.txt" to "merged"),
                parents = listOf(otherFs),
            )
            assertEquals(2, parentCount(it, merged))
        }
    }

    @Test
    fun `batch with parents creates merge commit`() {
        val store = createStore()
        store.use {
            var mainFs = it.branches["main"]
            mainFs = mainFs.write("a.txt", "main".toByteArray())

            it.branches["other"] = mainFs
            var otherFs = it.branches["other"]
            otherFs = otherFs.write("b.txt", "other".toByteArray())

            val batch = mainFs.batch(parents = listOf(otherFs))
            batch.write("c.txt", "merged".toByteArray())
            val merged = batch.commit()
            assertEquals(2, parentCount(it, merged))
        }
    }

    @Test
    fun `first parent lineage preserved with parents`() {
        val store = createStore()
        store.use {
            var mainFs = it.branches["main"]
            mainFs = mainFs.write("a.txt", "v1".toByteArray())
            val beforeMerge = mainFs

            it.branches["other"] = mainFs
            var otherFs = it.branches["other"]
            otherFs = otherFs.write("b.txt", "other".toByteArray())

            val merged = mainFs.write("c.txt", "merged".toByteArray(), parents = listOf(otherFs))

            // parent walks first-parent, so should give us beforeMerge
            val parent = merged.parent
            assertNotNull(parent)
            assertEquals(beforeMerge.commitId, parent!!.commitId)
        }
    }

    @Test
    fun `write with multiple extra parents`() {
        val store = createStore()
        store.use {
            var mainFs = it.branches["main"]
            mainFs = mainFs.write("a.txt", "main".toByteArray())

            it.branches["b1"] = mainFs
            var b1Fs = it.branches["b1"]
            b1Fs = b1Fs.write("b1.txt", "b1".toByteArray())

            it.branches["b2"] = mainFs
            var b2Fs = it.branches["b2"]
            b2Fs = b2Fs.write("b2.txt", "b2".toByteArray())

            val merged = mainFs.write("m.txt", "merged".toByteArray(), parents = listOf(b1Fs, b2Fs))
            assertEquals(3, parentCount(it, merged))
        }
    }
}
