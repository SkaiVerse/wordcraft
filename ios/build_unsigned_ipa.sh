#!/usr/bin/env bash
set -euo pipefail

# Determine repository root
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

LOG_DIR="build/logs"
rm -rf "$LOG_DIR" build/build-logs.zip
mkdir -p "$LOG_DIR"

package_logs() {
    local exit_code=$?
    set +e
    echo "==> Packaging build logs into build/build-logs.zip (exit_code=$exit_code)..."
    mkdir -p "$LOG_DIR/src"
    cp -R ios/Sources "$LOG_DIR/src/" 2>/dev/null || true
    cp ios/project.yml ios/Info.plist "$LOG_DIR/src/" 2>/dev/null || true

    APP_BIN=$(find build/DerivedData -path "*/WordCraft.app/WordCraft" -type f 2>/dev/null | head -n 1)
    if [ -n "${APP_BIN:-}" ] && [ -f "$APP_BIN" ]; then
        otool -L "$APP_BIN" > "$LOG_DIR/native-libs.txt" 2>&1 || true
    else
        grep -E "sdk:|framework:" ios/project.yml > "$LOG_DIR/native-libs.txt" 2>/dev/null || true
    fi

    (cd build && zip -qr build-logs.zip logs) || true
    echo "==> Saved build logs to build/build-logs.zip"
    exit "$exit_code"
}
trap package_logs EXIT

bump_ios_plist_version() {
    local plist="ios/Info.plist"
    local current_short current_build major minor _ next_short next_build
    current_short=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$plist" 2>/dev/null || echo "0.1")
    current_build=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$plist" 2>/dev/null || echo "1")
    IFS='.' read -r major minor _ <<< "$current_short"
    case "$major" in ''|*[!0-9]*) major=0 ;; esac
    case "$minor" in ''|*[!0-9]*) minor=0 ;; esac
    case "$current_build" in ''|*[!0-9]*) current_build=0 ;; esac
    minor=$((10#$minor + 1))
    if [ "$minor" -ge 10 ]; then
        major=$((10#$major + 1))
        minor=0
    else
        major=$((10#$major))
    fi
    next_short="${major}.${minor}"
    next_build=$((10#$current_build + 1))
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $next_short" "$plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $next_build" "$plist"
    echo "==> Bumped iOS version: $current_short ($current_build) -> $next_short ($next_build)"
}

# Local builds auto-bump by +0.1; CI builds keep the committed version unless FORCE_VERSION_BUMP=1.
if { [ "${GITHUB_ACTIONS:-}" != "true" ] || [ "${FORCE_VERSION_BUMP:-0}" = "1" ]; } && [ "${SKIP_VERSION_BUMP:-0}" != "1" ]; then
    bump_ios_plist_version
fi

# Capture environment metadata
{
    echo "Timestamp: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    echo "Git commit: $(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
    echo "Git branch: $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo 'unknown')"
    echo "Rustc: $(rustc --version 2>/dev/null || echo 'missing')"
    echo "Cargo: $(cargo --version 2>/dev/null || echo 'missing')"
    echo "XcodeGen: $(xcodegen --version 2>/dev/null || echo 'missing')"
    echo "=== Xcode Version ==="
    xcodebuild -version 2>&1 || true
    echo "=== Available SDKs ==="
    xcodebuild -showsdks 2>&1 || true
    echo "=== App Version (Info.plist) ==="
    /usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" ios/Info.plist 2>/dev/null || true
    /usr/libexec/PlistBuddy -c "Print :CFBundleVersion" ios/Info.plist 2>/dev/null || true
} > "$LOG_DIR/env.log"

echo "=================================================="
echo " WordCraft iPadOS: Unsigned .ipa Packaging Script "
echo "=================================================="

# 1. Ensure aarch64-apple-ios target is installed
echo "==> Step 1/6: Checking Rust target aarch64-apple-ios..."
if ! rustup target list --installed | grep -q "aarch64-apple-ios"; then
    echo "Installing aarch64-apple-ios target..."
    rustup target add aarch64-apple-ios
else
    echo "aarch64-apple-ios target is already installed."
fi

# 2. Compile wordcraft-ios in release mode
echo "==> Step 2/6: Compiling wordcraft-ios (release, aarch64-apple-ios)..."
cargo build -p wordcraft-ios --target aarch64-apple-ios --release 2>&1 | tee "$LOG_DIR/rust-build.log"

LIB_PATH="target/aarch64-apple-ios/release/libwordcraft.a"
if [ ! -f "$LIB_PATH" ]; then
    echo "Error: Static library not found at $LIB_PATH"
    exit 1
fi
echo "Static library built successfully: $LIB_PATH ($(du -h "$LIB_PATH" | cut -f1))"

# 3. Generate Xcode project via XcodeGen
echo "==> Step 3/6: Generating Xcode project via XcodeGen..."
if ! command -v xcodegen &> /dev/null; then
    echo "Error: 'xcodegen' command not found."
    echo "Please install XcodeGen via Homebrew: brew install xcodegen"
    exit 1
fi
xcodegen generate --spec ios/project.yml 2>&1 | tee "$LOG_DIR/xcodegen.log"

if ! grep -q "UIApplicationSceneManifest" ios/Info.plist; then
    echo "Error: ios/Info.plist is missing UIApplicationSceneManifest (was it overwritten by XcodeGen?)"
    exit 1
fi

# 4. Build WordCraft.app with xcodebuild (unsigned)
echo "==> Step 4/6: Building WordCraft.app via xcodebuild..."
mkdir -p build/DerivedData
xcodebuild -project ios/WordCraft.xcodeproj \
    -scheme WordCraft \
    -configuration Release \
    -sdk iphoneos \
    -destination 'generic/platform=iOS' \
    -derivedDataPath build/DerivedData \
    CODE_SIGN_IDENTITY="" \
    CODE_SIGNING_REQUIRED=NO \
    CODE_SIGNING_ALLOWED=NO \
    build 2>&1 | tee "$LOG_DIR/xcodebuild.log"

APP_PATH="build/DerivedData/Build/Products/Release-iphoneos/WordCraft.app"
if [ ! -d "$APP_PATH" ]; then
    APP_PATH=$(find build/DerivedData -name "WordCraft.app" -type d | head -n 1)
fi

if [ -z "$APP_PATH" ] || [ ! -d "$APP_PATH" ]; then
    echo "Error: Could not locate built WordCraft.app in build/DerivedData!"
    exit 1
fi
echo "Found built app at: $APP_PATH"

# 5. Stage Payload and package unsigned .ipa
echo "==> Step 5/6: Staging and creating unsigned .ipa..."
rm -rf build/ipa_stage build/WordCraft-unsigned.ipa
mkdir -p build/ipa_stage/Payload
cp -R "$APP_PATH" build/ipa_stage/Payload/WordCraft.app

# Strip any stray code signature so sideloaders (AltStore/SideStore/TrollStore) sign cleanly
rm -rf build/ipa_stage/Payload/WordCraft.app/_CodeSignature

(cd build/ipa_stage && zip -qr -y ../WordCraft-unsigned.ipa Payload)

# 6. Verify .ipa contents and output size
echo "==> Step 6/6: Verifying .ipa package..."
unzip -Z1 build/WordCraft-unsigned.ipa > "$LOG_DIR/ipa-contents.txt"
if ! grep -Fxq "Payload/WordCraft.app/Info.plist" "$LOG_DIR/ipa-contents.txt"; then
    echo "Error: IPA verification failed! Payload/WordCraft.app/Info.plist missing."
    exit 1
fi
if ! grep -Fxq "Payload/WordCraft.app/WordCraft" "$LOG_DIR/ipa-contents.txt"; then
    echo "Error: IPA verification failed! Payload/WordCraft.app/WordCraft executable missing."
    exit 1
fi

IPA_SIZE=$(du -h build/WordCraft-unsigned.ipa | cut -f1)
echo "=================================================="
echo " BUILD SUCCESSFUL!"
echo " Artifact: build/WordCraft-unsigned.ipa"
echo " Size:     $IPA_SIZE"
echo "=================================================="
