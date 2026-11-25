package it.mauromazzieri.cashcraft

import android.content.Context
import android.content.Intent
import android.net.Uri
import java.io.File
import java.io.IOException
import java.math.BigDecimal
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

data class Account(val id: Long, val name: String, val currency: String, val balance: BigDecimal)

data class Movement(
    val id: Long,
    val account: String,
    val date: String,
    val amount: BigDecimal,
    val payee: String,
    val memo: String,
    val category: String,
    /** Only a plain movement is edited on the phone; a transfer or a split is seen and deleted. */
    val plain: Boolean,
)

data class BudgetLine(val category: String, val budget: BigDecimal, val spent: BigDecimal, val remaining: BigDecimal)

/** A folder the person picked, and the data files in it. */
data class Picked(val tree: Uri, val names: List<String>)

data class BooksState(
    val ready: Boolean = false,
    val open: Boolean = false,
    val busy: Boolean = false,
    val picked: Picked? = null,
    val accounts: List<Account> = emptyList(),
    val worth: BigDecimal? = null,
    val movements: List<Movement> = emptyList(),
    val categories: List<String> = emptyList(),
    val budget: List<BudgetLine> = emptyList(),
    /** The shared file could not be reached: changes wait on the phone. */
    val offline: Boolean = false,
    /** A computer has the file open: the phone only reads. */
    val inUse: Boolean = false,
    /** A copy written because both the phone and a computer had changed the file. */
    val notice: String? = null,
    val error: String? = null,
)

/**
 * The shared data file and its private copy on this phone. The engine works on the copy, and whole
 * files are carried: the shared one is replaced only when it is still the one that was read.
 * All work runs on the IO dispatcher, one task at a time.
 */
object Books {
    private const val PREFS = "cashcraft"
    private const val NEW_NAME = "cashcraft.db"
    private val lock = Mutex()
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutableState = MutableStateFlow(BooksState())
    val state: StateFlow<BooksState> = mutableState

    private lateinit var app: Context
    private var place: Place? = null
    private var search = ""

    fun start(context: Context) {
        if (::app.isInitialized) return
        app = context.applicationContext
        scope.launch {
            lock.withLock {
                val prefs = prefs()
                val tree = prefs.getString("tree", null)?.let(Uri::parse)
                val name = prefs.getString("name", null)
                if (tree != null && name != null) place = Place(app.contentResolver, tree, name)
                runCatching {
                    Core.ask("lang") { put("lang", Locale.getDefault().language) }
                    if (place != null) {
                        sync()
                        show()
                    }
                }.onFailure(::fail)
                mutableState.update { it.copy(ready = true, open = place != null) }
            }
        }
    }

    /** Looks for the data files of a folder the person picked: one is opened, of more the person chooses. */
    fun choose(tree: Uri) = task {
        if (dirty()) sync()
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        app.contentResolver.takePersistableUriPermission(tree, flags)
        val names = Place.dataFiles(app.contentResolver, tree)
        if (dirty()) rescue(tree)
        if (names.size == 1) open(Place(app.contentResolver, tree, names.first()))
        else mutableState.update { it.copy(picked = Picked(tree, names)) }
    }

    fun pick(name: String) = task { mutableState.value.picked?.let { open(Place(app.contentResolver, it.tree, name)) } }

    /** Makes a data file in the folder just picked. */
    fun create() = task {
        val tree = mutableState.value.picked?.tree ?: return@task
        val file = File(app.filesDir, "new.db")
        file.delete()
        Core.ask("new") { put("path", file.path) }
        val bytes = file.readBytes()
        file.delete()
        val made = Place(app.contentResolver, tree, NEW_NAME)
        made.make(bytes)
        open(made)
    }

    fun cancel() {
        mutableState.update { it.copy(picked = null) }
    }

    /** Changes that cannot reach their file are kept, whole, in the folder the person has just chosen. */
    private fun rescue(tree: Uri) {
        val copy = besides(prefs().getString("name", null) ?: NEW_NAME)
        Core.ask("settle") { put("path", work().path) }
        Place(app.contentResolver, tree, copy).make(work().readBytes())
        prefs().edit().putBoolean("dirty", false).commit()
        mutableState.update { it.copy(notice = copy) }
    }

    private fun besides(name: String): String {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.ROOT).format(Date())
        val dot = name.lastIndexOf('.')
        return "${name.substring(0, dot)} (phone $stamp)${name.substring(dot)}"
    }

    private fun open(chosen: Place) {
        val bytes = chosen.read()
        check(bytes)
        // First whose the copy is, with no base: if what follows stops, the file is read again.
        prefs().edit().putString("tree", chosen.tree.toString()).putString("name", chosen.name)
            .putString("base", "").putBoolean("dirty", false).commit()
        place = chosen
        pending().delete()
        adopt(bytes)
        prefs().edit().putString("base", hash(bytes)).commit()
        mutableState.update { BooksState(ready = true, open = true, notice = it.notice) }
        sync()
        show()
    }

    /** Looks at the shared file again: a computer may have changed it. */
    fun reload() = task {
        if (place != null) {
            sync()
            show()
        }
    }

    fun search(text: String) = task {
        search = text
        if (place != null) show()
    }

    /** A change: an operation of the engine. */
    fun change(tool: String, fill: JSONObject.() -> Unit) = task {
        sync()
        if (mutableState.value.inUse) throw CoreException(app.getString(R.string.in_use))
        // Said before it happens: a change that stops halfway is carried or kept, never forgotten.
        val before = dirty()
        prefs().edit().putBoolean("dirty", true).commit()
        runCatching { call(tool, JSONObject().apply(fill)) }.onFailure {
            fail(it)
            // Refused: nothing of it is in the copy.
            if (it is CoreException && !before && same()) prefs().edit().putBoolean("dirty", false).commit()
        }
        sync()
        show()
    }

    fun dismissError() {
        mutableState.update { it.copy(error = null) }
    }

    fun dismissNotice() {
        mutableState.update { it.copy(notice = null) }
    }

    private fun task(body: () -> Unit) {
        scope.launch {
            lock.withLock {
                mutableState.update { it.copy(busy = true) }
                runCatching(body).onFailure(::fail)
                mutableState.update { it.copy(busy = false) }
            }
        }
    }

    private fun fail(e: Throwable) {
        mutableState.update { it.copy(error = e.message ?: e.toString()) }
    }

    /** Whether the copy still is the shared file as it was read. */
    private fun same() = hash(work().readBytes()) == prefs().getString("base", null)

    /** Brings the shared file here, or carries the copy there. */
    private fun sync() {
        val shared = place ?: return
        if (work().exists()) Core.ask("settle") { put("path", work().path) }
        val bytes = try {
            mutableState.update { it.copy(inUse = shared.inUse()) }
            fetch(shared)
        } catch (e: SecurityException) {
            throw e
        } catch (e: IOException) {
            if (!work().exists()) throw e
            mutableState.update { it.copy(offline = true) }
            return
        }
        val now = hash(bytes)
        val base = prefs().getString("base", null)
        val changedThere = now != base
        if (!dirty()) {
            if (changedThere || !work().exists()) {
                adopt(bytes)
                prefs().edit().putString("base", now).commit()
            }
        } else if (changedThere) {
            // Both changed: the phone's goes beside, whole, and the shared one is taken. A shared file
            // that cannot be taken is refused before a copy is made.
            check(bytes)
            val copy = besides(shared.name)
            Place(app.contentResolver, shared.tree, copy).make(work().readBytes())
            adopt(bytes)
            prefs().edit().putString("base", now).putBoolean("dirty", false).commit()
            mutableState.update { it.copy(notice = copy) }
        } else if (!mutableState.value.inUse) {
            val ours = work().readBytes()
            keep(pending(), ours)
            try {
                // Changed since it was read a moment ago: the next turn tells the two apart.
                if (!shared.replace(bytes, ours)) {
                    pending().delete()
                    return sync()
                }
            } catch (e: IOException) {
                mutableState.update { it.copy(offline = true) }
                return
            }
            prefs().edit().putString("base", hash(ours)).putBoolean("dirty", false).commit()
            pending().delete()
        }
        mutableState.update { it.copy(offline = false) }
    }

    /** The shared file. A write of ours that stopped halfway is done again first. */
    private fun fetch(shared: Place): ByteArray {
        val bytes = shared.read()
        if (!pending().exists()) return bytes
        val ours = pending().readBytes()
        // A write starts by emptying the file: what stopped halfway is the beginning of ours.
        val torn = bytes.size < ours.size && ours.copyOf(bytes.size).contentEquals(bytes)
        if (torn && !shared.replace(bytes, ours)) throw IOException("the file is changing")
        val now = if (torn) ours else bytes
        if (now.contentEquals(ours)) {
            // The write had ended: what the phone has since changed is told by comparing.
            val more = !work().readBytes().contentEquals(ours)
            prefs().edit().putString("base", hash(ours)).putBoolean("dirty", more).commit()
        }
        pending().delete()
        return now
    }

    private fun fetched(bytes: ByteArray) = File(app.filesDir, "fetched.db").apply { writeBytes(bytes) }

    private fun check(bytes: ByteArray) {
        val from = fetched(bytes)
        Core.ask("check") { put("from", from.path) }
    }

    private fun adopt(bytes: ByteArray) {
        val from = fetched(bytes)
        Core.ask("adopt") { put("from", from.path); put("path", work().path) }
    }

    private fun call(tool: String, args: JSONObject = JSONObject()): JSONObject = Core.ask("call") {
        put("path", work().path)
        put("tool", tool)
        put("args", args)
    } as JSONObject

    private fun show() {
        val accounts = call("list_accounts")
        val movements = call("list_transactions", JSONObject().put("limit", 300).put("search", search))
        val categories = call("list_categories")
        val budget = call("budget_status")
        mutableState.update { state ->
            state.copy(
            accounts = objects(accounts.getJSONArray("accounts")).map {
                Account(it.getLong("id"), it.getString("name"), it.optString("currency"), money(it, "balance"))
            },
            worth = money(accounts, "net_worth"),
            movements = objects(movements.getJSONArray("transactions")).map {
                Movement(
                    it.getLong("id"), it.optString("account"), it.getString("date"), money(it, "amount"),
                    it.optString("payee"), it.optString("memo"), it.optString("category"),
                    it.optString("kind") in listOf("category", "uncategorized"),
                )
            },
            categories = objects(categories.getJSONArray("categories")).map { it.getString("path") },
            budget = objects(budget.getJSONArray("lines")).map {
                BudgetLine(it.getString("category"), money(it, "budget"), money(it, "spent"), money(it, "remaining"))
            },
        ) }
    }

    /** The engine counts whole cents. */
    private fun money(o: JSONObject, key: String): BigDecimal = BigDecimal.valueOf(o.getLong(key), 2)

    private fun objects(a: JSONArray) = (0 until a.length()).map { a.getJSONObject(it) }

    private fun hash(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    private fun keep(file: File, bytes: ByteArray) {
        val part = File(file.parentFile, "${file.name}.part")
        part.outputStream().use {
            it.write(bytes)
            it.fd.sync()
        }
        if (!part.renameTo(file)) throw IOException("the phone's copy could not be written")
    }

    private fun dirty() = prefs().getBoolean("dirty", false)

    private fun prefs() = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun work() = File(app.filesDir, "work.db")

    private fun pending() = File(app.filesDir, "pending.db")
}
