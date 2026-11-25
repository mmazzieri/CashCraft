# The phone apps

`mobile/android` (Kotlin, Jetpack Compose) and `mobile/ios` (SwiftUI) are thin apps on the operations of `cashcraft_engine::ops`. Their first version shows accounts and net worth, lists and searches transactions, adds, edits and deletes them, records transfers, makes accounts, and shows and sets the budget of the month. Not yet (issue #19): importing statements, merging two changed files, a lock with the device's biometrics, editing transfers and split transactions, a signed APK in the releases.

## The data file

The file is the desktop's, with the same `SCHEMA_VERSION`; a newer one is refused. The person chooses its folder, and the phone works on a private copy, checked before it takes the place of the one it had.

- The copy is carried whole: the engine closes it after every operation, so no log lies beside it.
- The shared file is replaced only when it is still the one that was read. If a computer changed it too, the phone's file goes beside it as `name (phone date).db` and the shared one is taken. Neither is lost; they are not merged.
- When a log (`-wal`, `-journal`) lies beside the shared file, a computer has it open: the phone only reads.
- **Android** writes in place, without an atomic rename: what is being written is kept, and a write that stopped is done again. **iOS** replaces the file atomically.

**Limit:** the desktop still opens the file where it is, and a sync app may not carry the log that says so. Close CashCraft on the computer, and let the folder sync, before using the same file on the phone. The change of the desktop is part of issue #19.

## The bridge

`crates/ffi` builds a shared library for Android (`Core.call` through JNI) and a static one for iOS (`cashcraft_call`, `cashcraft_free`). One request in JSON, one answer: `{"ok": …}` or `{"error": "…"}`.

| `op` | Fields | What it does |
|---|---|---|
| `lang` | `lang` | language of messages and of amounts typed |
| `new` | `path` | makes a data file |
| `check` | `from` | checks a fetched file and brings it up to date |
| `adopt` | `from`, `path` | checks a fetched file and puts it in the place of the copy |
| `settle` | `path` | ends what a call that stopped left open |
| `call` | `path`, `tool`, `args` | one of the operations in `ALLOWED`; amounts come out in whole cents |

## Building

Android, with the SDK and NDK: `rustup target add aarch64-linux-android x86_64-linux-android`, `cargo install cargo-ndk`, then `cd mobile/android && ./gradlew assembleDebug`.

iOS, on a Mac with Xcode: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`, `brew install xcodegen`, then `cd mobile/ios && xcodegen generate && open CashCraft.xcodeproj`.

The `Mobile` workflow builds both; the Android build is kept as an artifact.
