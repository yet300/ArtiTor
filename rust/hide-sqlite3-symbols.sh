#!/bin/sh
# Hides the bundled libsqlite3-sys symbols inside a produced Apple static library.
#
# Why: rusqlite's bundled SQLite exports the whole sqlite3 API as global symbols. When a
# consumer statically links this library together with another SQLite provider (e.g.
# SQLCipher for an encrypted app database), the linker silently binds the consumer's
# sqlite3_* calls to arti's plain SQLite — the consumer's "encrypted" database ends up
# plaintext.
#
# How: prelink the whole archive into one relocatable object (`ld -r -all_load`), which
# resolves arti's internal sqlite3 references against the bundled definitions, while
# `-unexported_symbols_list` + `-x` demote the _sqlite3* globals to local and drop them
# from the symbol table. The result cannot satisfy (or shadow) anything outside arti.
# (llvm-objcopy --redefine-syms is NOT usable here: on Mach-O it renames definitions but
# silently leaves undefined references untouched.)
#
# Requires a release-profile build: debug Rust archives contain LLVM-bitcode members from
# the prebuilt std that Xcode's ld cannot merge.
#
# Idempotent: a processed archive has no global _sqlite3* symbols, so it is skipped.
set -eu

ARCHIVE="$1"

SYSROOT="$(rustc --print sysroot)"
NM="$(find "$SYSROOT" -name llvm-nm -type f 2>/dev/null | head -1)"
if [ -z "$NM" ]; then
    echo "error: llvm-tools not found in the Rust toolchain — run: rustup component add llvm-tools" >&2
    exit 1
fi

if ! "$NM" "$ARCHIVE" 2>/dev/null | grep -q "[TDSC] _sqlite3"; then
    echo "hide-sqlite3-symbols: no global _sqlite3* in $ARCHIVE (already processed?)"
    exit 0
fi

# Derive -arch / -platform_version from the cargo target triple in the archive path.
case "$ARCHIVE" in
    *aarch64-apple-ios-sim*)  ARCH=arm64;  PLATFORM="ios-simulator 15.0 15.0" ;;
    *x86_64-apple-ios*)       ARCH=x86_64; PLATFORM="ios-simulator 15.0 15.0" ;;
    *aarch64-apple-ios*)      ARCH=arm64;  PLATFORM="ios 15.0 15.0" ;;
    *aarch64-apple-darwin*)   ARCH=arm64;  PLATFORM="macos 11.0 11.0" ;;
    *x86_64-apple-darwin*)    ARCH=x86_64; PLATFORM="macos 11.0 11.0" ;;
    *) echo "error: cannot infer arch/platform from path: $ARCHIVE" >&2; exit 1 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

printf '_sqlite3*\n' > "$WORK/hide.exp"

# shellcheck disable=SC2086  # PLATFORM is intentionally three words
xcrun ld -r -arch "$ARCH" -platform_version $PLATFORM \
    -all_load "$ARCHIVE" \
    -x -unexported_symbols_list "$WORK/hide.exp" \
    -o "$WORK/merged.o"

if "$NM" "$WORK/merged.o" 2>/dev/null | grep -q "_sqlite3"; then
    echo "error: _sqlite3* symbols survived the prelink of $ARCHIVE" >&2
    exit 1
fi

xcrun libtool -static "$WORK/merged.o" -o "$ARCHIVE"
echo "hide-sqlite3-symbols: localized bundled sqlite3 in $ARCHIVE"
