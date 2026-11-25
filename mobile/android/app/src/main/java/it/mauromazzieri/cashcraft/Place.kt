package it.mauromazzieri.cashcraft

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import java.io.FileNotFoundException
import java.io.IOException

/**
 * A data file by its name in a folder the person chose. The folder, and not the file, because the
 * access to a file is lost when a sync app replaces it, and copies are written beside it.
 */
class Place(private val resolver: ContentResolver, val tree: Uri, val name: String) {
    companion object {
        private val EXTENSIONS = listOf(".db", ".sqlite", ".cashcraft")

        private fun uri(tree: Uri, id: String): Uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)

        /** The files of the folder: name and document id. */
        private fun files(resolver: ContentResolver, tree: Uri): Map<String, String> {
            val list = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
            val columns = arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME)
            val cursor = resolver.query(list, columns, null, null, null) ?: throw IOException("the folder could not be listed")
            return cursor.use { c ->
                val out = HashMap<String, String>()
                while (c.moveToNext()) {
                    val id = c.getString(0) ?: continue
                    val name = c.getString(1) ?: continue
                    out[name] = id
                }
                out
            }
        }

        fun dataFiles(resolver: ContentResolver, tree: Uri): List<String> =
            files(resolver, tree).keys.filter { n -> EXTENSIONS.any { n.endsWith(it) } }.sorted()
    }

    private fun all() = files(resolver, tree)

    fun read(): ByteArray {
        val id = all()[name] ?: throw FileNotFoundException("$name is not in the folder")
        return resolver.openInputStream(uri(tree, id))?.use { it.readBytes() } ?: throw IOException("$name could not be opened")
    }

    /** A log beside the file: a computer has it open, and the file alone may not hold everything. */
    fun inUse(): Boolean = all().let { f -> listOf("-wal", "-journal").any { name + it in f } }

    /** Replaces the content, if it still is [expected], and reads it back. False: it changed. */
    fun replace(expected: ByteArray, bytes: ByteArray): Boolean {
        val id = all()[name] ?: throw FileNotFoundException("$name is not in the folder")
        if (!read().contentEquals(expected)) return false
        put(id, bytes)
        if (!read().contentEquals(bytes)) throw IOException("$name was not written whole")
        return true
    }

    /** Makes a new file, which must not be there. */
    fun make(bytes: ByteArray) {
        if (name in all()) throw IOException("$name is there already")
        val root = uri(tree, DocumentsContract.getTreeDocumentId(tree))
        val made = DocumentsContract.createDocument(resolver, root, "application/octet-stream", name)
            ?: throw IOException("$name could not be made")
        val id = DocumentsContract.getDocumentId(made)
        if (all()[name] != id) {
            DocumentsContract.deleteDocument(resolver, made)
            throw IOException("$name could not be made with this name")
        }
        try {
            put(id, bytes)
            if (!read().contentEquals(bytes)) throw IOException("$name was not written whole")
        } catch (e: IOException) {
            // So that the next try finds the name free.
            runCatching { DocumentsContract.deleteDocument(resolver, made) }
            throw e
        }
    }

    private fun put(id: String, bytes: ByteArray) {
        resolver.openOutputStream(uri(tree, id), "wt")?.use { it.write(bytes) } ?: throw IOException("$name could not be written")
    }
}
