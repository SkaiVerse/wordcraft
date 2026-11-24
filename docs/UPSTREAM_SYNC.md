# Upstream Synchronization & Maintenance Guide for WordCraft iPadOS

This document provides instructions for maintaining and synchronizing the iPadOS port of WordCraft with future updates from the upstream WordCraft repository.

---

## 1. Architecture & Isolation Strategy

To ensure effortless merges with upstream commits, all iPadOS code adheres to strict modular isolation:

```
wordcraft/
├── crates/
│   ├── wordcraft-ios/          # [ISOLATED] Dedicated Rust staticlib + C FFI bridge
│   │   ├── Cargo.toml
│   │   ├── include/
│   │   │   └── wordcraft_ios.h # C FFI header exported to Swift
│   │   └── src/
│   │       ├── lib.rs          # Host orchestration & Metal surface lifecycle
│   │       ├── ffi.rs          # Safe C-compatible export declarations
│   │       ├── bridge.rs       # Bridge routing into Session and CommandRegistry
│   │       └── input.rs        # InputModeTracker, Pencil/Palm rejection & gestures
│   └── ui-egui/
│       └── src/canvas/
│           └── touch_editing.rs # [NEW] Touch selection handles, loupe & quick format bar
├── ios/                        # [ISOLATED] Swift / UIKit host application & XcodeGen spec
│   ├── Sources/
│   │   ├── AppDelegate.swift
│   │   ├── SceneDelegate.swift
│   │   ├── ViewController.swift
│   │   ├── MetalView.swift
│   │   ├── KeyCommands.swift
│   │   ├── DocumentPicker.swift
│   │   ├── ScribbleAndPencil.swift
│   │   └── WordCraft-Bridging-Header.h
│   ├── Assets.xcassets/
│   ├── Info.plist
│   ├── project.yml
│   └── build_unsigned_ipa.sh
├── .github/workflows/
│   └── build-ios-ipa.yml       # [ISOLATED] GitHub Actions CI for unsigned IPA
├── codemagic.yaml              # [ISOLATED] Codemagic CI configuration
└── docs/
    └── UPSTREAM_SYNC.md        # This guide
```

### Upstream Modifications Map

Modifications to existing upstream Rust files have been kept surgical and strictly gated:

| File | Modification & Isolation Rationale |
|---|---|
| `crates/ui-egui/Cargo.toml` | Gated `craft` features `["dialogs", "update"]` behind `all(not(target_arch = "wasm32"), not(target_os = "ios"))` to avoid unsupported `rfd` and desktop self-updater dependencies on iOS. |
| `crates/ui-egui/src/lib.rs` | Gated desktop updater command `UPDATE_APP` behind `all(not(target_arch = "wasm32"), not(target_os = "ios"))`. |
| `crates/ui-egui/src/file_dialogs.rs` | Added `FileDialogResult::Unavailable` fallback on iOS (native `.docx` file picking is handled via UIKit `UIDocumentPickerViewController`). |
| `crates/ui-egui/src/app.rs` | Enabled Mac/iPad `command_catalog_mac` on `target_os = "ios"`; applied touch-friendly hit sizes (`APP_SIZES_TOUCH`) when in `Touch` mode. |
| `crates/ui-egui/src/ribbon.rs` | Added responsive horizontal scroll wrapper when viewport width < 800pt or in `Touch` mode. |
| `crates/ui-egui/src/panes.rs` | Added auto-collapsing slide-over drawer with backdrop dismiss for side panes when viewport width < 800pt. |
| `crates/ui-egui/src/canvas.rs` | Connected touch selection handles, loupe, floating contextual toolbar, and virtual keyboard height offset (`keyboard_inset_bottom`). |
| `crates/ui-egui/src/objects.rs` | Made object selection border grab tolerance adaptive (18pt in `Touch` mode vs 5pt in `Pointer` mode). |
| `crates/ui-egui/src/table_resize.rs` | Made table column/row border reach adaptive (22pt in `Touch` mode vs 4pt in `Pointer` mode). |
| `crates/engine/src/cmd.rs`, `para.rs`, `text.rs` | Updated `mac_or` shortcut macros to include `target_os = "ios"` (enabling `Cmd` shortcuts and standard iOS line navigation parity). |

---

## 2. Syncing with Upstream: Step-by-Step

### Step 1: Configure Git Remotes
If not already configured, ensure you have both `origin` (your iPad branch/fork) and `upstream` (official WordCraft repo):

```bash
git remote add upstream https://github.com/storytold/wordcraft.git
git fetch upstream main
```

### Step 2: Rebase `ipad-port` on Upstream
Rebase your iPad work on top of the latest upstream `main`:

```bash
git checkout ipad-port
git rebase upstream/main
```

### Step 3: Resolving Conflicts (if any)
Because almost all iPad-specific code is isolated in `crates/wordcraft-ios/` and `ios/`, merge conflicts will rarely occur. If an upstream change affects one of the few hooked upstream files (such as `crates/ui-egui/src/canvas.rs` or `panes.rs`):
1. Keep the upstream changes.
2. Ensure the `#[cfg(target_os = "ios")]` or `input_mode == InputMode::Touch` hook remains intact.
3. Mark resolved with `git add <file>` and continue with `git rebase --continue`.

### Step 4: Verification After Rebase
Always run verification checks on both desktop and iOS targets after rebasing:

```bash
# 1. Verify desktop build (must remain 100% green)
cargo check -p wordcraft
cargo test -p wordcraft

# 2. Verify iOS crate logic and unit tests
cargo check -p wordcraft-ios
cargo test -p wordcraft-ios --lib

# 3. Verify cross-compilation for iOS device target
cargo check -p wordcraft-ios --target aarch64-apple-ios
```

### Step 5: Build and Test Unsigned IPA
On a macOS host or in CI:

```bash
./ios/build_unsigned_ipa.sh
```

---

## 3. Sideloading Instructions

The resulting artifact `build/WordCraft-unsigned.ipa` can be sideloaded onto an iPad using any of the following methods:

### AltStore / SideStore
1. Transfer `WordCraft-unsigned.ipa` to your iPad via AirDrop, iCloud Drive, or the Files app.
2. Open **AltStore** or **SideStore** on your iPad.
3. Go to **My Apps** -> tap **+** -> select `WordCraft-unsigned.ipa`.
4. AltStore/SideStore will re-sign the app with your free or paid Apple Developer ID and install it.

### Sideloadly (macOS / Windows)
1. Connect your iPad to your Mac or PC via USB or Wi-Fi.
2. Launch **Sideloadly**.
3. Drag `WordCraft-unsigned.ipa` into Sideloadly.
4. Enter your Apple ID and click **Start**.

### TrollStore (iOS 15.0 - 16.6.1, 17.0 where supported)
1. Share `WordCraft-unsigned.ipa` to **TrollStore**.
2. TrollStore will install WordCraft with persistent entitlements (no 7-day expiration).

> [!NOTE]
> On iOS 16.0+, ensure **Developer Mode** is enabled on your iPad under **Settings -> Privacy & Security -> Developer Mode**.
