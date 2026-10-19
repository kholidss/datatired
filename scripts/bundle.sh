#!/bin/sh
# Builds target/release/bundle/DataTired.app (icon + Info.plist) from the release binary, plus the
# release zip and its checksum in dist/:
#   dist/DataTired-<version>-macos-arm64.zip  and  dist/DataTired-<version>-macos-arm64.zip.sha256
# Upload both to a GitHub release tagged v<version>; install.sh downloads them.
set -e
cd "$(dirname "$0")/.."
cargo build --release
cargo run --release --example icon -q
iconutil -c icns target/DataTired.iconset -o assets/DataTired.icns
APP=target/release/bundle/DataTired.app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/DataTired "$APP/Contents/MacOS/DataTired"
cp assets/DataTired.icns "$APP/Contents/Resources/DataTired.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DataTired</string>
  <key>CFBundleDisplayName</key><string>DataTired</string>
  <key>CFBundleIdentifier</key><string>dev.datatired.DataTired</string>
  <key>CFBundleExecutable</key><string>DataTired</string>
  <key>CFBundleIconFile</key><string>DataTired</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc signature: Apple Silicon refuses to run unsigned code. (Not notarized — install.sh downloads
# with curl, which does not quarantine files, so Gatekeeper does not block the app.)
codesign --force --deep --sign - "$APP"

mkdir -p dist
ZIP="DataTired-$VERSION-macos-arm64.zip"
rm -f "dist/$ZIP" "dist/$ZIP.sha256"
ditto -c -k --keepParent "$APP" "dist/$ZIP"
(cd dist && shasum -a 256 "$ZIP" > "$ZIP.sha256")
echo "$APP"
echo "dist/$ZIP"
echo "dist/$ZIP.sha256"
