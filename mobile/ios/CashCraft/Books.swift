import CryptoKit
import Foundation

struct Account: Identifiable, Hashable {
    let id: Int
    let name: String
    let currency: String
    let balance: Decimal
}

struct Movement: Identifiable {
    let id: Int
    let account: String
    let date: String
    let amount: Decimal
    let payee: String
    let memo: String
    let category: String
    /// Only a plain movement is edited on the phone; a transfer or a split is seen and deleted.
    let plain: Bool
}

struct BudgetLine: Identifiable {
    var id: String { category }
    let category: String
    let budget: Decimal
    let spent: Decimal
    let remaining: Decimal
}

struct EnvelopeLine: Identifiable {
    let id: Int
    let name: String
    let assigned: Decimal
    let spent: Decimal
    let available: Decimal
}

/// The month of the envelopes, when the file's budget is made of them.
struct Envelopes {
    let toBeAssigned: Decimal
    let lines: [EnvelopeLine]
}

/// What the screens show.
struct Shown {
    var accounts: [Account] = []
    var worth: Decimal?
    var movements: [Movement] = []
    var categories: [String] = []
    var budget: [BudgetLine] = []
    var envelopes: Envelopes?
    /// The shared file could not be reached: changes wait on the phone.
    var offline = false
    /// A computer has the file open: the phone only reads.
    var inUse = false
    /// A copy written because both the phone and a computer had changed the file.
    var notice: String?
}

/// The shared data file and its private copy on this phone. File work runs on a queue of its own,
/// one task at a time.
@MainActor
final class Books: ObservableObject {
    @Published private(set) var ready = false
    @Published private(set) var open = false
    @Published private(set) var busy = false
    @Published private(set) var shown = Shown()
    /// Data files of the folder just picked, when there is not exactly one.
    @Published var picked: [String]?
    @Published var notice: String?
    @Published var error: String?

    private let work = DispatchQueue(label: "cashcraft.file")
    private let worker = Worker()

    init() {
        let language = Locale.preferredLanguages.first ?? "en"
        run { w in
            try Core.ask("lang", ["lang": language])
            if try w.restore() {
                try w.sync()
                try w.show()
            }
        }
    }

    /// Looks for the data files of a folder the person picked: one is opened, of more the person chooses.
    func choose(_ folder: URL) {
        guard folder.startAccessingSecurityScopedResource() else {
            error = String(localized: "The folder cannot be opened.")
            return
        }
        var names: [String] = []
        run { w in
            names = try w.look(folder)
            if names.count == 1 { try w.open(names[0]) }
        } done: {
            if names.count != 1 { $0.picked = names }
        }
    }

    func pick(_ name: String) { run { w in try w.open(name) } }

    func create() { run { w in try w.create() } }

    func reload() { run { w in try w.refresh() } }

    func search(_ text: String) { run { w in try w.find(text) } }

    /// A change: an operation of the engine.
    func change(_ tool: String, _ args: [String: Any]) { run { w in try w.change(tool, args) } }

    private func run(_ body: @escaping (Worker) throws -> Void, done: @escaping (Books) -> Void = { _ in }) {
        busy = true
        let worker = self.worker
        work.async {
            var failure: String?
            do { try body(worker) } catch { failure = error.localizedDescription }
            let (shown, open) = (worker.shown, worker.isOpen)
            worker.shown.notice = nil
            Task { @MainActor in
                self.busy = false
                self.shown = shown
                self.open = open
                if let notice = shown.notice { self.notice = notice }
                if let failure { self.error = failure } else { done(self) }
                self.ready = true
            }
        }
    }
}

/// The file work, done on one queue.
final class Worker: @unchecked Sendable {
    var shown = Shown()
    private var folder: URL?
    private var looked: URL?
    private var name = ""
    private var query = ""
    private let defaults = UserDefaults.standard
    private let manager = FileManager.default
    private static let extensions = ["db", "sqlite", "cashcraft"]

    var isOpen: Bool { folder != nil }

    private var dirty: Bool {
        get { defaults.bool(forKey: "dirty") }
        set { defaults.set(newValue, forKey: "dirty") }
    }

    /// The file opened before, if the phone still lets the app use its folder.
    func restore() throws -> Bool {
        guard let saved = defaults.data(forKey: "folder"), let name = defaults.string(forKey: "name") else { return false }
        var stale = false
        let url = try URL(resolvingBookmarkData: saved, bookmarkDataIsStale: &stale)
        guard url.startAccessingSecurityScopedResource() else { throw CoreError(message: "the folder cannot be opened") }
        if stale, let fresh = try? url.bookmarkData() { defaults.set(fresh, forKey: "folder") }
        folder = url
        self.name = name
        return true
    }

    func look(_ url: URL) throws -> [String] {
        looked = nil
        // A shared file that cannot be reached must not keep the person from choosing another.
        if dirty { try? sync() }
        let names = try manager.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)
            .map { Self.visible($0.lastPathComponent) }
            .filter { Self.extensions.contains(($0 as NSString).pathExtension) }
            .sorted()
        if dirty {
            // Changes that cannot reach their file are kept, whole, in the folder just chosen.
            let copy = besides(defaults.string(forKey: "name") ?? "cashcraft.db")
            try Core.ask("settle", ["path": try work().path])
            try write(try Data(contentsOf: try work()), url.appendingPathComponent(copy), replacing: nil)
            // With no base: if the person stays with the old file, it is read again as it is.
            defaults.set("", forKey: "base")
            dirty = false
            shown.notice = copy
        }
        looked = url
        return names
    }

    private func besides(_ name: String) -> String {
        let stamp = DateFormatter()
        stamp.locale = Locale(identifier: "en_US_POSIX")
        stamp.dateFormat = "yyyyMMdd-HHmmss"
        let parts = name as NSString
        return "\(parts.deletingPathExtension) (phone \(stamp.string(from: Date()))).\(parts.pathExtension)"
    }

    /// The name of a file; one that iCloud has not downloaded is `.name.icloud`.
    private static func visible(_ n: String) -> String {
        n.hasPrefix(".") && n.hasSuffix(".icloud") ? String(n.dropFirst().dropLast(7)) : n
    }

    func open(_ name: String) throws {
        guard let url = looked else { return }
        let bytes = try read(url.appendingPathComponent(name))
        let bookmark = try url.bookmarkData()
        try check(bytes)
        // First whose the copy is, with no base: if what follows stops, the file is read again.
        defaults.set("", forKey: "base")
        defaults.set(bookmark, forKey: "folder")
        defaults.set(name, forKey: "name")
        dirty = false
        defaults.synchronize()
        folder = url
        self.name = name
        // What the other file held is never shown as this one's.
        try? manager.removeItem(at: try work())
        try adopt(bytes)
        defaults.set(hash(bytes), forKey: "base")
        let notice = shown.notice
        shown = Shown()
        shown.notice = notice
        try refresh()
    }

    func create() throws {
        guard let url = looked else { return }
        let file = try support("new.db")
        try? manager.removeItem(at: file)
        try Core.ask("new", ["path": file.path])
        let bytes = try Data(contentsOf: file)
        try manager.removeItem(at: file)
        try write(bytes, url.appendingPathComponent("cashcraft.db"), replacing: nil)
        try open("cashcraft.db")
    }

    func refresh() throws {
        guard isOpen else { return }
        try sync()
        try show()
    }

    func find(_ text: String) throws {
        query = text
        if isOpen { try show() }
    }

    func change(_ tool: String, _ args: [String: Any]) throws {
        try sync()
        if shown.inUse { throw CoreError(message: String(localized: "The file is open on a computer. Close CashCraft there to write from the phone.")) }
        let before = dirty
        // Said before it happens: a change that stops halfway is carried or kept, never forgotten.
        dirty = true
        defaults.synchronize()
        do {
            _ = try call(tool, args)
        } catch {
            // Refused: the copy is as it was.
            if !before, let now = try? Data(contentsOf: try work()), hash(now) == defaults.string(forKey: "base") { dirty = false }
            throw error
        }
        try refresh()
    }

    /// Brings the shared file here, or carries the copy there.
    func sync() throws {
        guard let folder else { return }
        let shared = folder.appendingPathComponent(name)
        let copy = try work()
        let here = manager.fileExists(atPath: copy.path)
        if here { try Core.ask("settle", ["path": copy.path]) }
        let bytes: Data
        do {
            shown.inUse = ["-wal", "-journal"].contains { suffix in
                manager.fileExists(atPath: folder.appendingPathComponent(name + suffix).path)
            }
            bytes = try read(shared)
        } catch {
            if !here { throw error }
            shown.offline = true
            return
        }
        let now = hash(bytes)
        let changedThere = now != defaults.string(forKey: "base")
        if !dirty {
            if changedThere || !here {
                try adopt(bytes)
                defaults.set(now, forKey: "base")
            }
        } else if changedThere {
            let ours = try Data(contentsOf: copy)
            if ours != bytes {
                // Both changed: the phone's goes beside, whole, and the shared one is taken. A shared
                // file that cannot be taken is refused before a copy is made.
                try check(bytes)
                let beside = besides(name)
                try write(ours, folder.appendingPathComponent(beside), replacing: nil)
                try adopt(bytes)
                shown.notice = beside
            }
            defaults.set(now, forKey: "base")
            dirty = false
        } else if !shown.inUse {
            let ours = try Data(contentsOf: copy)
            do {
                try write(ours, shared, replacing: bytes)
            } catch is Changed {
                // Changed since it was read a moment ago: the next turn tells the two apart.
                return try sync()
            } catch {
                shown.offline = true
                return
            }
            defaults.set(hash(ours), forKey: "base")
            dirty = false
        }
        shown.offline = false
    }

    private func fetched(_ bytes: Data) throws -> URL {
        let from = try support("fetched.db")
        try bytes.write(to: from, options: .atomic)
        return from
    }

    private func check(_ bytes: Data) throws { try Core.ask("check", ["from": try fetched(bytes).path]) }

    private func adopt(_ bytes: Data) throws {
        let from = try fetched(bytes)
        try Core.ask("adopt", ["from": from.path, "path": try work().path])
    }

    private func call(_ tool: String, _ args: [String: Any] = [:]) throws -> [String: Any] {
        try Core.ask("call", ["path": try work().path, "tool": tool, "args": args]) as? [String: Any] ?? [:]
    }

    func show() throws {
        let accounts = try call("list_accounts")
        let movements = try call("list_transactions", ["limit": 300, "search": query])
        let categories = try call("list_categories")
        let budget = try call("budget_status")
        let envelopes = try call("list_envelopes")
        shown.worth = money(accounts["net_worth"])
        shown.accounts = list(accounts["accounts"]).compactMap { a in
            guard let id = a["id"] as? Int else { return nil }
            return Account(id: id, name: string(a["name"]), currency: string(a["currency"]), balance: money(a["balance"]))
        }
        shown.movements = list(movements["transactions"]).compactMap { m in
            guard let id = m["id"] as? Int else { return nil }
            return Movement(
                id: id, account: string(m["account"]), date: string(m["date"]), amount: money(m["amount"]),
                payee: string(m["payee"]), memo: string(m["memo"]), category: string(m["category"]),
                plain: ["category", "uncategorized"].contains(string(m["kind"]))
            )
        }
        shown.categories = list(categories["categories"]).map { string($0["path"]) }
        shown.budget = list(budget["lines"]).map {
            BudgetLine(category: string($0["category"]), budget: money($0["budget"]), spent: money($0["spent"]), remaining: money($0["remaining"]))
        }
        shown.envelopes = string(envelopes["mode"]) != "envelopes" ? nil : Envelopes(
            toBeAssigned: money(envelopes["to_be_assigned"]),
            lines: list(envelopes["envelopes"]).compactMap { e in
                guard let id = e["id"] as? Int else { return nil }
                return EnvelopeLine(id: id, name: string(e["name"]), assigned: money(e["assigned"]), spent: money(e["spent"]), available: money(e["available"]))
            }
        )
    }

    private func list(_ v: Any?) -> [[String: Any]] { v as? [[String: Any]] ?? [] }
    private func string(_ v: Any?) -> String { v as? String ?? "" }

    /// The engine counts whole cents.
    private func money(_ v: Any?) -> Decimal { Decimal((v as? NSNumber)?.int64Value ?? 0) / 100 }

    private func read(_ url: URL) throws -> Data {
        var result: Result<Data, Error> = .failure(CoreError(message: "not read"))
        var coordination: NSError?
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &coordination) { at in
            result = Result { try Data(contentsOf: at) }
        }
        if let coordination { throw coordination }
        return try result.get()
    }

    /// The file is not the one that was read any more.
    private struct Changed: Error {}

    /// Replaces a file that still holds `old`, or makes a new one when there is no `old`.
    private func write(_ bytes: Data, _ url: URL, replacing old: Data?) throws {
        var failure: Error?
        var coordination: NSError?
        NSFileCoordinator().coordinate(writingItemAt: url, options: old == nil ? [] : .forReplacing, error: &coordination) { at in
            do {
                if let old, try Data(contentsOf: at) != old { throw Changed() }
                try bytes.write(to: at, options: old == nil ? .withoutOverwriting : .atomic)
            } catch { failure = error }
        }
        if let coordination { throw coordination }
        if let failure { throw failure }
        guard try read(url) == bytes else { throw CoreError(message: "the file was not written whole") }
    }

    private func hash(_ bytes: Data) -> String { SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }

    private func support(_ file: String) throws -> URL {
        try manager.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent(file)
    }

    private func work() throws -> URL { try support("work.db") }
}
