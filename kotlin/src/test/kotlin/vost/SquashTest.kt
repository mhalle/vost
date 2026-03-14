package vost

import org.junit.jupiter.api.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class SquashTest {

    @Test
    fun `squash creates root commit`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())

            val squashed = fs.squash()
            assertEquals("a", String(squashed.read("a.txt")))
            assertEquals("b", String(squashed.read("b.txt")))
            assertNull(squashed.parent)
            assertFalse(squashed.writable)
        }
    }

    @Test
    fun `squash preserves tree hash`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("data.txt", "hello".toByteArray())

            val squashed = fs.squash()
            assertEquals(fs.treeHash, squashed.treeHash)
        }
    }

    @Test
    fun `squash with parent`() {
        val store = createStore()
        store.use {
            val tip = it.branches["main"]
            var fs = tip.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())

            val squashed = fs.squash(parent = tip)
            val parent = squashed.parent
            assertNotNull(parent)
            assertEquals(tip.commitHash, parent.commitHash)
            assertEquals("b", String(squashed.read("b.txt")))
        }
    }

    @Test
    fun `squash with custom message`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())

            val squashed = fs.squash(message = "Custom squash message")
            assertTrue(squashed.message.startsWith("Custom squash message"))
        }
    }

    @Test
    fun `squash default message`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())

            val squashed = fs.squash()
            assertTrue("squash" in squashed.message.lowercase())
        }
    }

    @Test
    fun `squash assign to branch`() {
        val store = createStore()
        store.use {
            var fs = it.branches["main"]
            fs = fs.write("a.txt", "a".toByteArray())
            fs = fs.write("b.txt", "b".toByteArray())

            val squashed = fs.squash()
            it.branches["squashed"] = squashed

            val result = it.branches["squashed"]
            assertEquals("a", String(result.read("a.txt")))
            assertNull(result.parent)
        }
    }
}
