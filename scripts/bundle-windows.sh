#!/bin/sh
# Cross-builds DataTired.exe for Windows x64 from macOS / Linux (MinGW), plus the release zip and its
# checksum in dist/:
#   dist/DataTired-<version>-windows-x64.exe, dist/DataTired-<version>-windows-x64.zip (+ .sha256)
# Needs: rustup target add x86_64-pc-windows-gnu; brew install mingw-w64 (or apt install mingw-w64).
set -e
cd "$(dirname "$0")/.."
TARGET=x86_64-pc-windows-gnu
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release --target "$TARGET"
mkdir -p dist
NAME="DataTired-$VERSION-windows-x64"
x86_64-w64-mingw32-strip -o "dist/$NAME.exe" "target/$TARGET/release/DataTired.exe"
rm -rf "dist/$NAME" "dist/$NAME.zip" "dist/$NAME.zip.sha256"
mkdir "dist/$NAME"
cp "dist/$NAME.exe" "dist/$NAME/DataTired.exe"
(cd dist && zip -qr "$NAME.zip" "$NAME" && rm -rf "$NAME" && shasum -a 256 "$NAME.zip" > "$NAME.zip.sha256")
echo "dist/$NAME.exe"
echo "dist/$NAME.zip"
echo "dist/$NAME.zip.sha256"
