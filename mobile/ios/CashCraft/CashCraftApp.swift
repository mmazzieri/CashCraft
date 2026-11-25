import SwiftUI

@main
struct CashCraftApp: App {
    @StateObject private var books = Books()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(books)
        }
        .onChange(of: phase) { now in
            if now == .active { books.reload() }
        }
    }
}
