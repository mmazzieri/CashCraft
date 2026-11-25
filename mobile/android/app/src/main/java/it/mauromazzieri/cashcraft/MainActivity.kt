package it.mauromazzieri.cashcraft

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import java.math.BigDecimal
import java.text.NumberFormat
import java.time.LocalDate
import java.util.Locale
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        Books.start(this)
        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                Surface(Modifier.fillMaxSize()) { App() }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        Books.reload()
    }
}

/** Two decimals, as amounts are typed in the language in use (Italian or English). */
private fun show(amount: BigDecimal): String {
    val locale = if (Locale.getDefault().language == "it") Locale.ITALY else Locale.UK
    return NumberFormat.getNumberInstance(locale).apply { minimumFractionDigits = 2; maximumFractionDigits = 2 }.format(amount)
}

private enum class Tab { Accounts, Movements, Budget }

private sealed interface Form {
    data class OfMovement(val movement: Movement?) : Form
    data object OfTransfer : Form
    data object OfAccount : Form
    data class OfBudget(val line: BudgetLine?) : Form
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun App() {
    val state by Books.state.collectAsStateWithLifecycle()
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { if (it != null) Books.choose(it) }
    if (!state.ready) return
    var tab by rememberSaveable { mutableStateOf(Tab.Accounts) }
    var form by remember { mutableStateOf<Form?>(null) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.app_name)) },
                actions = {
                    if (state.open) {
                        val (label, next) = when (tab) {
                            Tab.Accounts -> R.string.new_account to Form.OfAccount
                            Tab.Movements -> R.string.new_movement to Form.OfMovement(null)
                            Tab.Budget -> R.string.new_budget to Form.OfBudget(null)
                        }
                        if (tab == Tab.Movements) {
                            TextButton(onClick = { form = Form.OfTransfer }) { Text(stringResource(R.string.transfer)) }
                        }
                        IconButton(onClick = { form = next }) { Icon(Icons.Filled.Add, stringResource(label)) }
                        IconButton(onClick = { Books.reload() }) { Icon(Icons.Filled.Refresh, stringResource(R.string.reload)) }
                        TextButton(onClick = { pick.launch(null) }) { Text(stringResource(R.string.open)) }
                    }
                },
            )
        },
        bottomBar = {
            if (state.open) {
                NavigationBar {
                    for (t in Tab.entries) {
                        val label = when (t) {
                            Tab.Accounts -> R.string.accounts
                            Tab.Movements -> R.string.movements
                            Tab.Budget -> R.string.budget
                        }
                        NavigationBarItem(tab == t, { tab = t }, icon = {}, label = { Text(stringResource(label)) })
                    }
                }
            }
        },
    ) { inner ->
        Column(Modifier.padding(inner).fillMaxSize()) {
            if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            Messages(state)
            when {
                !state.open -> Welcome { pick.launch(null) }
                tab == Tab.Accounts -> Accounts(state)
                tab == Tab.Movements -> Movements(state) { form = Form.OfMovement(it) }
                else -> Budget(state) { form = Form.OfBudget(it) }
            }
        }
    }
    state.picked?.let { PickDialog(it) }
    val close = { form = null }
    when (val f = form) {
        null -> {}
        is Form.OfMovement -> MovementDialog(f.movement, state, close)
        Form.OfTransfer -> TransferDialog(state, close)
        Form.OfAccount -> AccountDialog(close)
        is Form.OfBudget -> BudgetDialog(f.line, state, close)
    }
}

@Composable
private fun Messages(state: BooksState) {
    val message = state.error ?: state.notice?.let { stringResource(R.string.conflict, it) }
    if (message != null) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(message, Modifier.weight(1f), color = MaterialTheme.colorScheme.error)
            TextButton(onClick = Books::dismiss) { Text(stringResource(R.string.close)) }
        }
    }
    val note = if (state.inUse) R.string.in_use else if (state.offline) R.string.offline else null
    if (note != null) Text(stringResource(note), Modifier.padding(16.dp, 4.dp), style = MaterialTheme.typography.bodySmall)
}

@Composable
private fun Welcome(choose: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(stringResource(R.string.welcome), modifier = Modifier.padding(vertical = 16.dp))
        Button(onClick = choose) { Text(stringResource(R.string.choose_folder)) }
    }
}

@Composable
private fun PickDialog(picked: Picked) {
    AlertDialog(
        onDismissRequest = Books::cancel,
        title = { Text(stringResource(R.string.which)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                if (picked.names.isEmpty()) Text(stringResource(R.string.no_file))
                for (name in picked.names) TextButton(onClick = { Books.pick(name) }) { Text(name) }
            }
        },
        confirmButton = { TextButton(onClick = { Books.create() }) { Text(stringResource(R.string.new_file)) } },
        dismissButton = { TextButton(onClick = Books::cancel) { Text(stringResource(R.string.close)) } },
    )
}

@Composable
private fun Line(title: String, detail: String, amount: String, open: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().let { if (open != null) it.clickable(onClick = open) else it }.padding(16.dp, 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (detail.isNotEmpty()) Text(detail, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Text(amount, Modifier.padding(start = 12.dp))
    }
    HorizontalDivider()
}

@Composable
private fun Accounts(state: BooksState) {
    LazyColumn {
        item { state.worth?.let { Line(stringResource(R.string.net_worth), "", show(it)) } }
        items(state.accounts, key = { it.id }) { Line(it.name, if (it.currency == "EUR") "" else it.currency, show(it.balance)) }
    }
}

@Composable
private fun Movements(state: BooksState, open: (Movement) -> Unit) {
    var query by rememberSaveable { mutableStateOf("") }
    LaunchedEffect(query) {
        delay(400)
        Books.search(query)
    }
    OutlinedTextField(
        query, { query = it },
        label = { Text(stringResource(R.string.search)) },
        singleLine = true,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
    )
    LazyColumn {
        items(state.movements, key = { it.id }) { m ->
            val detail = listOf(m.date, m.account, m.category).filter { it.isNotEmpty() }.joinToString(" · ")
            Line(m.payee, detail, show(m.amount)) { open(m) }
        }
    }
}

@Composable
private fun Budget(state: BooksState, open: (BudgetLine) -> Unit) {
    LazyColumn {
        items(state.budget, key = { it.category }) { b ->
            Line(b.category, stringResource(R.string.spent_of, show(b.spent), show(b.budget)), show(b.remaining)) { open(b) }
        }
    }
}

/** A choice among names. */
@Composable
private fun Choice(label: String, value: String, options: List<String>, choose: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Column(Modifier.padding(vertical = 4.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium)
        TextButton(onClick = { open = true }) { Text(value.ifEmpty { "—" }) }
        DropdownMenu(open, { open = false }) {
            for (o in options) DropdownMenuItem(text = { Text(o.ifEmpty { "—" }) }, onClick = { choose(o); open = false })
        }
    }
}

@Composable
private fun FormDialog(title: String, enabled: Boolean, save: () -> Unit, close: () -> Unit, fields: @Composable () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        title = { Text(title, fontWeight = FontWeight.Bold) },
        text = { Column(Modifier.verticalScroll(rememberScrollState())) { fields() } },
        confirmButton = { TextButton(onClick = { save(); close() }, enabled = enabled) { Text(stringResource(R.string.save)) } },
        dismissButton = { TextButton(onClick = close) { Text(stringResource(R.string.close)) } },
    )
}

@Composable
private fun Field(label: Int, value: String, change: (String) -> Unit) {
    OutlinedTextField(value, change, label = { Text(stringResource(label)) }, singleLine = true)
}

@Composable
private fun MovementDialog(movement: Movement?, state: BooksState, close: () -> Unit) {
    var account by remember { mutableStateOf(movement?.account ?: state.accounts.firstOrNull()?.name ?: "") }
    var amount by remember { mutableStateOf(movement?.let { show(it.amount) } ?: "") }
    var payee by remember { mutableStateOf(movement?.payee ?: "") }
    var date by remember { mutableStateOf(movement?.date ?: LocalDate.now().toString()) }
    var category by remember { mutableStateOf(movement?.category ?: "") }
    var memo by remember { mutableStateOf(movement?.memo ?: "") }
    var deleting by remember { mutableStateOf(false) }
    val plain = movement?.plain ?: true
    // By its id, because a name may be a number.
    val id = state.accounts.firstOrNull { it.name == account }?.id
    FormDialog(
        stringResource(if (movement == null) R.string.new_movement else R.string.movement),
        enabled = plain && id != null && amount.isNotBlank() && payee.isNotBlank(),
        save = {
            if (movement == null) {
                Books.change("add_transaction") {
                    put("account", id.toString())
                    put("amount", amount)
                    put("payee", payee)
                    put("date", date)
                    if (category.isNotEmpty()) put("category", category)
                    if (memo.isNotEmpty()) put("memo", memo)
                }
            } else {
                // Only what was changed.
                Books.change("update_transaction") {
                    put("id", movement.id)
                    if (amount != show(movement.amount)) put("amount", amount)
                    if (payee != movement.payee) put("payee", payee)
                    if (date != movement.date) put("date", date)
                    if (category != movement.category) put("category", category)
                    if (memo != movement.memo) put("memo", memo)
                }
            }
        },
        close,
    ) {
        if (movement == null) Choice(stringResource(R.string.account), account, state.accounts.map { it.name }) { account = it }
        else Text(movement.account)
        Field(R.string.amount, amount) { amount = it }
        Field(R.string.payee, payee) { payee = it }
        Field(R.string.date, date) { date = it }
        if (plain) Choice(stringResource(R.string.category), category, listOf("") + state.categories) { category = it }
        Field(R.string.memo, memo) { memo = it }
        if (!plain) Text(stringResource(R.string.not_plain), style = MaterialTheme.typography.bodySmall)
        if (movement != null) {
            TextButton(onClick = {
                if (deleting) {
                    Books.change("delete_transaction") { put("id", movement.id) }
                    close()
                } else {
                    deleting = true
                }
            }) { Text(stringResource(if (deleting) R.string.delete_sure else R.string.delete)) }
        }
    }
}

@Composable
private fun TransferDialog(state: BooksState, close: () -> Unit) {
    var from by remember { mutableStateOf(state.accounts.getOrNull(0)?.name ?: "") }
    var to by remember { mutableStateOf(state.accounts.getOrNull(1)?.name ?: "") }
    var amount by remember { mutableStateOf("") }
    var date by remember { mutableStateOf(LocalDate.now().toString()) }
    val a = state.accounts.firstOrNull { it.name == from }?.id
    val b = state.accounts.firstOrNull { it.name == to }?.id
    FormDialog(
        stringResource(R.string.transfer),
        enabled = a != null && b != null && a != b && amount.isNotBlank(),
        save = {
            Books.change("add_transfer") {
                put("from", a.toString())
                put("to", b.toString())
                put("amount", amount)
                put("date", date)
            }
        },
        close,
    ) {
        val names = state.accounts.map { it.name }
        Choice(stringResource(R.string.from), from, names) { from = it }
        Choice(stringResource(R.string.to), to, names) { to = it }
        Field(R.string.amount, amount) { amount = it }
        Field(R.string.date, date) { date = it }
    }
}

@Composable
private fun AccountDialog(close: () -> Unit) {
    var name by remember { mutableStateOf("") }
    var balance by remember { mutableStateOf("") }
    FormDialog(
        stringResource(R.string.new_account),
        enabled = name.isNotBlank(),
        save = {
            Books.change("create_account") {
                put("name", name)
                if (balance.isNotBlank()) put("opening_balance", balance)
            }
        },
        close,
    ) {
        Field(R.string.name, name) { name = it }
        Field(R.string.opening_balance, balance) { balance = it }
    }
}

@Composable
private fun BudgetDialog(line: BudgetLine?, state: BooksState, close: () -> Unit) {
    var category by remember { mutableStateOf(line?.category ?: "") }
    var amount by remember { mutableStateOf(line?.let { show(it.budget) } ?: "") }
    FormDialog(
        stringResource(R.string.new_budget),
        enabled = category.isNotEmpty() && amount.isNotBlank(),
        save = { Books.change("set_budget") { put("category", category); put("monthly_amount", amount) } },
        close,
    ) {
        if (line == null) Choice(stringResource(R.string.category), category, state.categories) { category = it } else Text(category)
        Field(R.string.monthly_amount, amount) { amount = it }
    }
}
