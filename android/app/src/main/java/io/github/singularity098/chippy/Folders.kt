package io.github.singularity098.chippy

import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract

/**
 * Between the system's folder picker, which speaks in document URIs, and
 * the engine, which reads folders by path (Chippy has "All files access").
 * Only folders on the device's own storage or an SD card have a path;
 * cloud drives and the like do not.
 */
object Folders {
    private const val STORAGE_DOCUMENTS = "com.android.externalstorage.documents"

    /** The file system path of a folder the picker returned, or null. */
    fun pathFor(tree: Uri): String? {
        val id = try {
            DocumentsContract.getTreeDocumentId(tree)
        } catch (e: IllegalArgumentException) {
            return null
        }
        if (tree.authority == "com.android.providers.downloads.documents") {
            return when {
                id.startsWith("raw:") -> id.substring(4)
                id == "downloads" -> Environment.getExternalStoragePublicDirectory(
                    Environment.DIRECTORY_DOWNLOADS
                ).absolutePath
                else -> null
            }
        }
        if (tree.authority != STORAGE_DOCUMENTS) return null
        val colon = id.indexOf(':')
        val volume = if (colon < 0) id else id.substring(0, colon)
        val rest = if (colon < 0) "" else id.substring(colon + 1)
        val root = when (volume) {
            "primary" -> Environment.getExternalStorageDirectory().absolutePath
            "home" -> Environment.getExternalStoragePublicDirectory(
                Environment.DIRECTORY_DOCUMENTS
            ).absolutePath
            else -> "/storage/$volume"
        }
        return if (rest.isEmpty()) root else "$root/$rest"
    }

    /** The picker's document for a path on the device's storage, if any. */
    fun documentUriFor(path: String?): Uri? {
        if (path.isNullOrEmpty()) return null
        val primary = Environment.getExternalStorageDirectory().absolutePath
        val id = when {
            path == primary -> "primary:"
            path.startsWith("$primary/") -> "primary:" + path.substring(primary.length + 1)
            path.startsWith("/storage/") -> {
                val rest = path.substring("/storage/".length)
                val slash = rest.indexOf('/')
                if (slash < 0) "$rest:" else rest.substring(0, slash) + ":" + rest.substring(slash + 1)
            }
            else -> return null
        }
        return DocumentsContract.buildDocumentUri(STORAGE_DOCUMENTS, id)
    }
}
