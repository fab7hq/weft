#!/bin/sh
# Install or upgrade Weft and RingFrame, then initialize the global
# configuration.
#
# There is no toolchain here. CI builds a binary per platform and publishes it
# with a checksum; this detects the platform, downloads, verifies and installs.
set -eu

REPO=fab7hq/weft
BIN_DIR=${WEFT_BIN_DIR:-$HOME/.local/bin}

fail() {
    printf '%s\n' "$1" >&2
    exit 1
}

case "$(uname -s)" in
    Darwin) platform=apple-darwin ;;
    Linux) platform=unknown-linux-musl ;;
    *) fail "Unsupported system: $(uname -s). Weft builds for macOS and Linux." ;;
esac

case "$(uname -m)" in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) fail "Unsupported architecture: $(uname -m)." ;;
esac

target="$arch-$platform"

command -v curl >/dev/null 2>&1 || fail 'curl is required.'
command -v tar >/dev/null 2>&1 || fail 'tar is required.'

if [ -n "${WEFT_VERSION:-}" ]; then
    tag=$WEFT_VERSION
else
    # The latest release, without needing a token or jq.
    tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
        sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$tag" ] || fail "Could not find the latest release of $REPO."
fi

archive="fab7-$tag-$target.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

printf 'Downloading %s %s for %s\n' "$REPO" "$tag" "$target"
curl -fsSL -o "$work/$archive" "$base/$archive" ||
    fail "No build for $target in $tag."
curl -fsSL -o "$work/$archive.sha256" "$base/$archive.sha256" ||
    fail "$tag publishes no checksum for $target; refusing to install."

# Verify before anything is unpacked, with whichever tool this system has.
expected=$(cut -d' ' -f1 <"$work/$archive.sha256")
if command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$work/$archive" | cut -d' ' -f1)
elif command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$work/$archive" | cut -d' ' -f1)
else
    fail 'Neither shasum nor sha256sum is available; cannot verify the download.'
fi
[ "$expected" = "$actual" ] ||
    fail "Checksum mismatch for $archive. Expected $expected, got $actual."

tar -xzf "$work/$archive" -C "$work"
mkdir -p "$BIN_DIR"
for name in weft ringframe; do
    [ -f "$work/$name" ] || fail "$archive does not contain $name."
    # Replaced rather than written through, so a running binary is undisturbed.
    mv "$work/$name" "$BIN_DIR/$name.new"
    chmod 755 "$BIN_DIR/$name.new"
    mv "$BIN_DIR/$name.new" "$BIN_DIR/$name"
done

printf 'Installed weft and ringframe %s to %s\n' "$tag" "$BIN_DIR"

# The binaries are in place whatever happens next, so a configuration that
# cannot be read is worth saying plainly rather than aborting on.
if ! "$BIN_DIR/ringframe" init --global; then
    printf '\n%s\n' "The binaries are installed, but the configuration could not be."
    printf '%s\n' "Run \`$BIN_DIR/ringframe init --global\` again once that is sorted."
fi

# Weft's harness files say how to start, resume and set up each harness. They
# come with fab7 at the tag the configuration above came from, and replace
# whatever was there: they are never edited.
harnesses="$HOME/.fab7/weft/harnesses"
revision=$(cat "$HOME/.fab7/rf/config/.revision" 2>/dev/null || true)
if [ -n "$revision" ] && [ "$revision" != local ] &&
    curl -fsSL -o "$work/fab7.tar.gz" "https://codeload.github.com/fab7hq/fab7/tar.gz/refs/tags/$revision" &&
    mkdir -p "$work/fab7" && tar -xzf "$work/fab7.tar.gz" -C "$work/fab7" &&
    [ -d "$work"/fab7/*/products/weft/harnesses ]; then
    mkdir -p "$HOME/.fab7/weft"
    rm -rf "$harnesses.new"
    cp -R "$work"/fab7/*/products/weft/harnesses "$harnesses.new"
    rm -rf "$harnesses"
    mv "$harnesses.new" "$harnesses"
    printf 'Installed the harness files of fab7 %s to %s\n' "$revision" "$harnesses"
else
    printf '\n%s\n' "Weft's harness files could not be installed, so Weft offers no harness yet."
    printf '%s\n' 'Run `weft update` again once the configuration is in place.'
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf '\n%s\n' "Add $BIN_DIR to your PATH." ;;
esac

printf '\n%s\n' 'Now install the plugin for your harness from the Fab7 marketplace:'
printf '%s\n' '  Claude Code:  claude plugin marketplace add fab7hq/fab7 && claude plugin install rf@fab7'
printf '%s\n' '  Codex:        codex plugin marketplace add fab7hq/fab7 && codex plugin add rf@fab7'
