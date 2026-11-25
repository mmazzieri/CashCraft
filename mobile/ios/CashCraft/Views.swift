import SwiftUI
import UniformTypeIdentifiers

/// Two decimals, as amounts are typed in the language in use (Italian or English).
private func show(_ amount: Decimal) -> String {
    let f = NumberFormatter()
    f.numberStyle = .decimal
    f.locale = Locale(identifier: (Locale.preferredLanguages.first ?? "en").hasPrefix("it") ? "it_IT" : "en_GB")
    f.minimumFractionDigits = 2
    f.maximumFractionDigits = 2
    return f.string(from: amount as NSDecimalNumber) ?? "\(amount)"
}

private func today() -> String {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.dateFormat = "yyyy-MM-dd"
    return f.string(from: Date())
}

private func blank(_ s: String) -> Bool { s.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

struct RootView: View {
    @EnvironmentObject var books: Books
    @State private var picking = false

    var body: some View {
        Group {
            if !books.ready {
                ProgressView()
            } else if !books.open {
                VStack(spacing: 16) {
                    Text("CashCraft").font(.largeTitle)
                    Text("Choose the folder that holds your CashCraft data file, or where to make a new one. Close CashCraft on the computer before using the same file on the phone.")
                        .multilineTextAlignment(.center)
                    Button("Choose the folder") { picking = true }.buttonStyle(.borderedProminent)
                }
                .padding(24)
            } else {
                TabView {
                    NavigationStack { AccountsView(pick: { picking = true }) }.tabItem { Label("Accounts", systemImage: "building.columns") }
                    NavigationStack { MovementsView() }.tabItem { Label("Transactions", systemImage: "list.bullet") }
                    NavigationStack { BudgetView() }.tabItem { Label("Budget", systemImage: "chart.bar") }
                }
            }
        }
        .fileImporter(isPresented: $picking, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { books.choose(url) }
        }
        .confirmationDialog(
            "Which data file?",
            isPresented: Binding(get: { books.picked != nil }, set: { if !$0 { books.picked = nil } }),
            titleVisibility: .visible
        ) {
            ForEach(books.picked ?? [], id: \.self) { name in Button(name) { books.pick(name) } }
            Button("New data file") { books.create() }
        }
        .alert(
            "Something went wrong",
            isPresented: Binding(get: { books.error != nil }, set: { if !$0 { books.error = nil } }),
            actions: { Button("Close") { books.error = nil } },
            message: { Text(books.error ?? "") }
        )
        .alert(
            "The file was changed elsewhere too",
            isPresented: Binding(get: { books.notice != nil }, set: { if !$0 { books.notice = nil } }),
            actions: { Button("Close") { books.notice = nil } },
            message: { Text("What was on the phone is in \(books.notice ?? "").") }
        )
    }
}

struct Status: View {
    @EnvironmentObject var books: Books

    var body: some View {
        if books.shown.inUse {
            Text("The file is open on a computer. Close CashCraft there to write from the phone.").font(.footnote)
        } else if books.shown.offline {
            Text("The shared file could not be reached. Changes are kept on the phone and written at the next try.").font(.footnote)
        }
    }
}

struct Line: View {
    let title: String
    let detail: String
    let amount: Decimal

    var body: some View {
        HStack {
            VStack(alignment: .leading) {
                Text(title).lineLimit(1)
                if !detail.isEmpty { Text(detail).font(.footnote).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer()
            Text(show(amount)).monospacedDigit()
        }
    }
}

struct AccountsView: View {
    @EnvironmentObject var books: Books
    let pick: () -> Void
    @State private var adding = false

    var body: some View {
        List {
            Status()
            if let worth = books.shown.worth { Line(title: String(localized: "Net worth"), detail: "", amount: worth) }
            ForEach(books.shown.accounts) { Line(title: $0.name, detail: $0.currency == "EUR" ? "" : $0.currency, amount: $0.balance) }
        }
        .navigationTitle("Accounts")
        .refreshable { books.reload() }
        .toolbar {
            ToolbarItem(placement: .navigationBarLeading) { Button("Open", action: pick) }
            ToolbarItemGroup(placement: .navigationBarTrailing) {
                if books.busy { ProgressView() }
                Button { adding = true } label: { Label("New account", systemImage: "plus") }
            }
        }
        .sheet(isPresented: $adding) { AccountForm() }
    }
}

struct MovementsView: View {
    @EnvironmentObject var books: Books
    @State private var query = ""
    @State private var editing: Movement?
    @State private var adding = false
    @State private var transfer = false

    var body: some View {
        List {
            Status()
            ForEach(books.shown.movements) { m in
                Button { editing = m } label: {
                    Line(title: m.payee, detail: [m.date, m.account, m.category].filter { !$0.isEmpty }.joined(separator: " · "), amount: m.amount)
                }
                .buttonStyle(.plain)
            }
        }
        .navigationTitle("Transactions")
        .searchable(text: $query)
        .task(id: query) {
            try? await Task.sleep(nanoseconds: 400_000_000)
            if !Task.isCancelled { books.search(query) }
        }
        .refreshable { books.reload() }
        .toolbar {
            ToolbarItem(placement: .navigationBarLeading) { Button("Transfer") { transfer = true } }
            ToolbarItem(placement: .navigationBarTrailing) {
                Button { adding = true } label: { Label("New transaction", systemImage: "plus") }
            }
        }
        .sheet(item: $editing) { MovementForm(movement: $0) }
        .sheet(isPresented: $adding) { MovementForm(movement: nil) }
        .sheet(isPresented: $transfer) { TransferForm() }
    }
}

struct BudgetView: View {
    @EnvironmentObject var books: Books
    @State private var editing: BudgetLine?
    @State private var adding = false

    var body: some View {
        List {
            Status()
            ForEach(books.shown.budget) { b in
                Button { editing = b } label: {
                    Line(title: b.category, detail: String(localized: "\(show(b.spent)) spent of \(show(b.budget))"), amount: b.remaining)
                }
                .buttonStyle(.plain)
            }
        }
        .navigationTitle("Budget")
        .refreshable { books.reload() }
        .toolbar { Button { adding = true } label: { Label("Monthly budget", systemImage: "plus") } }
        .sheet(item: $editing) { BudgetForm(line: $0) }
        .sheet(isPresented: $adding) { BudgetForm(line: nil) }
    }
}

/// A form with Close and Save.
struct Sheet<Fields: View>: View {
    @Environment(\.dismiss) private var dismiss
    let enabled: Bool
    let save: () -> Void
    @ViewBuilder let fields: () -> Fields

    var body: some View {
        NavigationStack {
            Form { fields() }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            save()
                            dismiss()
                        }
                        .disabled(!enabled)
                    }
                }
        }
    }
}

struct MovementForm: View {
    @EnvironmentObject var books: Books
    @Environment(\.dismiss) private var dismiss
    let movement: Movement?
    @State private var account = 0
    @State private var amount = ""
    @State private var payee = ""
    @State private var date = today()
    @State private var category = ""
    @State private var memo = ""
    @State private var deleting = false

    private var plain: Bool { movement?.plain ?? true }

    var body: some View {
        Sheet(enabled: plain && !blank(amount) && !blank(payee) && (movement != nil || account != 0), save: save) {
            if let movement {
                Text(movement.account)
            } else {
                Picker("Account", selection: $account) { ForEach(books.shown.accounts) { Text($0.name).tag($0.id) } }
            }
            TextField("Amount (negative: money out)", text: $amount).keyboardType(.numbersAndPunctuation)
            TextField("Payee", text: $payee)
            TextField("Date", text: $date)
            if plain {
                Picker("Category", selection: $category) {
                    Text("—").tag("")
                    ForEach(books.shown.categories, id: \.self) { Text($0).tag($0) }
                }
            } else {
                Text("A transfer or a split transaction is edited on the computer.").font(.footnote)
            }
            TextField("Notes", text: $memo)
            if let movement {
                Button("Delete", role: .destructive) { deleting = true }
                    .confirmationDialog("Delete for good", isPresented: $deleting, titleVisibility: .visible) {
                        Button("Delete", role: .destructive) {
                            books.change("delete_transaction", ["id": movement.id])
                            dismiss()
                        }
                    }
            }
        }
        .onAppear {
            account = books.shown.accounts.first?.id ?? 0
            guard let m = movement else { return }
            (amount, payee, date, category, memo) = (show(m.amount), m.payee, m.date, m.category, m.memo)
        }
    }

    private func save() {
        guard let m = movement else {
            var add: [String: Any] = ["account": String(account), "amount": amount, "payee": payee, "date": date]
            if !category.isEmpty { add["category"] = category }
            if !memo.isEmpty { add["memo"] = memo }
            books.change("add_transaction", add)
            return
        }
        // Only what was changed.
        var edit: [String: Any] = ["id": m.id]
        if amount != show(m.amount) { edit["amount"] = amount }
        if payee != m.payee { edit["payee"] = payee }
        if date != m.date { edit["date"] = date }
        if category != m.category { edit["category"] = category }
        if memo != m.memo { edit["memo"] = memo }
        books.change("update_transaction", edit)
    }
}

struct TransferForm: View {
    @EnvironmentObject var books: Books
    @State private var from = 0
    @State private var to = 0
    @State private var amount = ""
    @State private var date = today()

    var body: some View {
        Sheet(enabled: from != to && from != 0 && to != 0 && !blank(amount)) {
            books.change("add_transfer", ["from": String(from), "to": String(to), "amount": amount, "date": date])
        } fields: {
            Picker("From", selection: $from) { ForEach(books.shown.accounts) { Text($0.name).tag($0.id) } }
            Picker("To", selection: $to) { ForEach(books.shown.accounts) { Text($0.name).tag($0.id) } }
            TextField("Amount", text: $amount).keyboardType(.decimalPad)
            TextField("Date", text: $date)
        }
        .onAppear {
            from = books.shown.accounts.first?.id ?? 0
            to = books.shown.accounts.dropFirst().first?.id ?? 0
        }
    }
}

struct AccountForm: View {
    @EnvironmentObject var books: Books
    @State private var name = ""
    @State private var balance = ""

    var body: some View {
        Sheet(enabled: !blank(name)) {
            var add: [String: Any] = ["name": name]
            if !blank(balance) { add["opening_balance"] = balance }
            books.change("create_account", add)
        } fields: {
            TextField("Name", text: $name)
            TextField("Opening balance", text: $balance).keyboardType(.numbersAndPunctuation)
        }
    }
}

struct BudgetForm: View {
    @EnvironmentObject var books: Books
    let line: BudgetLine?
    @State private var category = ""
    @State private var amount = ""

    var body: some View {
        Sheet(enabled: !category.isEmpty && !blank(amount)) {
            books.change("set_budget", ["category": category, "monthly_amount": amount])
        } fields: {
            if line == nil {
                Picker("Category", selection: $category) {
                    Text("—").tag("")
                    ForEach(books.shown.categories, id: \.self) { Text($0).tag($0) }
                }
            } else {
                Text(category)
            }
            TextField("Amount each month", text: $amount).keyboardType(.decimalPad)
        }
        .onAppear {
            guard let line else { return }
            (category, amount) = (line.category, show(line.budget))
        }
    }
}
