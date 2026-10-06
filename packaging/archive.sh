#!/bin/sh
# Packs one target's release archive: the agent, its licence and the service
# files its OS needs, under cntrl-agent-<version>-<target>/ (D21). install.sh
# installs from it. Windows' is a zip of cntrl-agent.exe, which installs
# itself (D58), made with Windows' own tar.
#
# Usage: packaging/archive.sh <version> <target> <binary> <out-dir>

set -eu

version=${1:?version}
target=${2:?target}
binary=${3:?binary}
out=${4:?out-dir}
here=$(cd "$(dirname "$0")" && pwd)
name="cntrl-agent-$version-$target"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$out"
case $target in
*-windows-*)
    mkdir -p "$stage/$name"
    cp "$binary" "$stage/$name/cntrl-agent.exe"
    cp "$here/../LICENSE" "$stage/$name/LICENSE"
    /c/Windows/System32/tar.exe -a -cf "$out/$name.zip" -C "$stage" "$name"
    echo "$out/$name.zip"
    exit 0
    ;;
esac
mkdir -p "$stage/$name/packaging"
cp "$binary" "$stage/$name/cntrl-agent"
chmod 0755 "$stage/$name/cntrl-agent"
cp "$here/../LICENSE" "$stage/$name/LICENSE"
# The installer travels with the release, so `cntrl update` runs the one that
# knows this release's steps, checked with the archive (D41).
cp "$here/install.sh" "$stage/$name/packaging/install.sh"
case $target in
*-linux-*) cp -R "$here/systemd" "$stage/$name/packaging/" ;;
*-darwin)
    mkdir -p "$stage/$name/packaging/macos"
    cp "$here"/macos/*.plist "$stage/$name/packaging/macos/"
    ;;
*) echo "archive.sh: no service files for $target" >&2; exit 1 ;;
esac
# No extended attributes or AppleDouble files from a Mac: Linux's tar would
# warn about each.
if tar --version 2>/dev/null | grep -q bsdtar; then
    COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf "$out/$name.tar.gz" -C "$stage" "$name"
else
    tar --no-xattrs -czf "$out/$name.tar.gz" -C "$stage" "$name"
fi
echo "$out/$name.tar.gz"
