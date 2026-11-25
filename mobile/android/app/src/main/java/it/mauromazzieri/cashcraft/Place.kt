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

        /** The files of the folder: name, document id and size. */
        private fun files(resolver: ContentResolver, tree: Uri): Map<String, Pair<String, Long>> {
            val list = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
            val columns = arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_SIZE)
            val cursor = resolver.query(list, columns, null, null, null) ?: throw IOException("the folder could not be listed")
            return cursor.use { c ->
                val out = HashMap<String, Pair<String, Long>>()
                while (c.moveToNext()) {
                    val id = c.getString(0) ?: continue
                    val name = c.getString(1) ?: continue
                    out[name] = id to (if (c.isNull(2)) 0L else c.getLong(2))
                }
                out
            }
        }

        fun dataFiles(resolver: ContentResolver, tree: Uri): List<String> =
            files(resolver, tree).keys.filter { n -> EXTENSIONS.any { n.endsWith(it) } }.sorted()
    }

    private fun all() = files(resolver, tree)

    fun read(): ByteArray {
        val id = all()[name]?.first ?: throw FileNotFoundException("$name is not in the folder")
        return resolver.openInputStream(uri(tree, id))?.use { it.readBytes() } ?: throw IOException("$name could not be opened")
    }

    /** A log beside the file: a computer has it open, and the file alone may not hold everything. */
    fun inUse(): Boolean = all().let { f -> listOf("-wal", "-journal").any { (f[name + it]?.second ?: 0L) > 0L } }

    /** Replaces the content and reads it back. */
    fun write(bytes: ByteArray) {
        val id = all()[name]?.first ?: throw FileNotFoundException("$name is not in the folder")
        put(id, bytes)
        if (!read().contentEquals(bytes)) throw IOException("$name was not written whole")
    }

    /** Makes a new file, which must not be there. */
    fun make(bytes: ByteArray) {
        if (name in all()) throw IOException("$name is there already")
        val root = uri(tree, DocumentsContract.getTreeDocumentId(tree))
        val made = DocumentsContract.createDocument(resolver, root, "application/octet-stream", name)
            ?: throw IOException("$name could not be made")
        val id = DocumentsContract.getDocumentId(made)
        if (all()[name]?.first != id) {
            DocumentsContract.deleteDocument(resolver, made)
            throw IOException("$name could not be made with this name")
        }
        put(id, bytes)
        if (!read().contentEquals(bytes)) throw IOException("$name was not written whole")
    }

    private fun put(id: String, bytes: ByteArray) {
        resolver.openOutputStream(uri(tree, id), "wt")?.use { it.write(bytes) } ?: throw IOException("$name could not be written")
    }
}
