#!/bin/sh
# DataTired installer for macOS on Apple Silicon.
#
#   curl -fsSL https://raw.githubusercontent.com/kholidss/datatired/main/install.sh | sh
#
# Run it again to update. Optional environment variables:
#   DATATIRED_VERSION=v0.1.0       install that release instead of the latest
#   DATATIRED_REPO=owner/repo      GitHub repository hosting the releases (default kholidss/datatired)
#   DATATIRED_BASE_URL=https://... download from another host instead of GitHub Releases
#   DATATIRED_DIR=~/Applications   install location (default /Applications, or ~/Applications if not writable)
#   DATATIRED_FORCE=1              reinstall even if that version is already installed
set -eu

REPO="${DATATIRED_REPO:-kholidss/datatired}"
APP="DataTired.app"

say() { printf '%s\n' "$*"; }
fail() {
	printf 'datatired-install: %s\n' "$*" >&2
	exit 1
}

[ "$(uname -s)" = Darwin ] || fail "this installer supports macOS only"
[ "$(uname -m)" = arm64 ] || fail "this build is for Apple Silicon (arm64); this Mac is $(uname -m)"

TAG="${DATATIRED_VERSION:-}"
if [ -z "${TAG}" ]; then
	TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
		sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) || true
	[ -n "${TAG}" ] || fail "could not find the latest release of ${REPO}"
fi
VERSION="${TAG#v}"
BASE="${DATATIRED_BASE_URL:-https://github.com/${REPO}/releases/download/${TAG}}"
ZIP="DataTired-${VERSION}-macos-arm64.zip"

DIR="${DATATIRED_DIR:-/Applications}"
if [ -z "${DATATIRED_DIR:-}" ] && [ ! -w "${DIR}" ]; then
	DIR="${HOME}/Applications"
fi
mkdir -p "${DIR}"

PLIST="${DIR}/${APP}/Contents/Info.plist"
if [ -f "${PLIST}" ] && [ -z "${DATATIRED_FORCE:-}" ]; then
	CURRENT=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${PLIST}" 2>/dev/null || true)
	if [ "${CURRENT}" = "${VERSION}" ]; then
		say "DataTired ${VERSION} is already installed in ${DIR}"
		exit 0
	fi
fi

TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT INT TERM

say "Downloading DataTired ${VERSION}..."
curl -fSL --progress-bar "${BASE}/${ZIP}" -o "${TMP}/${ZIP}"
curl -fsSL "${BASE}/${ZIP}.sha256" -o "${TMP}/${ZIP}.sha256"
EXPECTED=$(awk '{print $1}' "${TMP}/${ZIP}.sha256")
ACTUAL=$(shasum -a 256 "${TMP}/${ZIP}" | awk '{print $1}')
[ "${EXPECTED}" = "${ACTUAL}" ] || fail "checksum mismatch for ${ZIP} (expected ${EXPECTED}, got ${ACTUAL})"

ditto -x -k "${TMP}/${ZIP}" "${TMP}/unpacked"
[ -d "${TMP}/unpacked/${APP}" ] || fail "${ZIP} does not contain ${APP}"

if pgrep -xq DataTired; then
	say "Note: DataTired is running; restart it to use ${VERSION}."
fi
rm -rf "${DIR:?}/${APP}"
mv "${TMP}/unpacked/${APP}" "${DIR}/${APP}"
say "Installed DataTired ${VERSION} to ${DIR}/${APP}"
say "Open it from Launchpad or Spotlight, or run: open -a DataTired"
