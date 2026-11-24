#!/usr/bin/env bash
set -euo pipefail

# Determine repository root
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

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
cargo build -p wordcraft-ios --target aarch64-apple-ios --release

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
xcodegen generate --spec ios/project.yml

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
    build

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
if ! unzip -l build/WordCraft-unsigned.ipa | grep -q "Payload/WordCraft.app/Info.plist"; then
    echo "Error: IPA verification failed! Payload/WordCraft.app/Info.plist missing."
    exit 1
fi

IPA_SIZE=$(du -h build/WordCraft-unsigned.ipa | cut -f1)
echo "=================================================="
echo " BUILD SUCCESSFUL!"
echo " Artifact: build/WordCraft-unsigned.ipa"
echo " Size:     $IPA_SIZE"
echo "=================================================="
