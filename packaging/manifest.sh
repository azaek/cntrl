#!/bin/sh
# Writes a release manifest (D21): the version, its channel, when it was
# published, and each archive's target, URL, size and SHA-256. CI signs the
# exact bytes with the release key; Console verifies them before it imports
# the release.
#
# Usage: packaging/manifest.sh <version> <channel> <base-url> <archive>...

set -eu

version=${1:?version}
channel=${2:?channel}
base=${3:?base-url}
shift 3

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

printf '{\n  "version": "%s",\n  "channel": "%s",\n  "published_at": "%s",\n  "artifacts": {' \
    "$version" "$channel" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
sep=
for archive in "$@"; do
    file=$(basename "$archive")
    target=${file#"cntrl-agent-$version-"}
    target=${target%.tar.gz}
    target=${target%.zip}
    size=$(wc -c <"$archive" | tr -d ' ')
    printf '%s\n    "%s": { "url": "%s/%s", "size": %s, "sha256": "%s" }' \
        "$sep" "$target" "$base" "$file" "$size" "$(sha256 "$archive")"
    sep=,
done
printf '\n  }\n}\n'
