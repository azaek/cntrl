#!/bin/sh
# Removes the cntrl agent from macOS: its launchd jobs and the binary. With
# --purge it also removes the agent's identity, config and logs and the _cntrl
# user; the device then stays in Console until someone removes it there.
#
# Usage: sudo packaging/macos/uninstall.sh [--purge]

set -eu

fail() {
    echo "uninstall.sh: $*" >&2
    exit 1
}

PURGE=
case ${1:-} in
--purge) PURGE=1 ;;
"") ;;
*) fail "unknown option $1" ;;
esac
[ "$(id -u)" -eq 0 ] || fail "run it with sudo"

bin="/Library/Application Support/cntrl/bin/cntrl-agent"
for label in pw.cntrl.agent pw.cntrl.privd; do
    launchctl bootout "system/$label" 2>/dev/null || true
    rm -f "/Library/LaunchDaemons/$label.plist"
done
case $(readlink /usr/local/bin/cntrl 2>/dev/null || true) in
"$bin" | cntrl-agent) rm -f /usr/local/bin/cntrl ;;
esac
rm -rf "/Library/Application Support/cntrl/bin"
# Test builds before 0.1.0 installed it in /usr/local/bin.
rm -f /usr/local/bin/cntrl-agent /var/run/cntrl-agent.sock /var/run/cntrl-privd.sock

if [ -n "$PURGE" ]; then
    rm -rf "/Library/Application Support/cntrl" /etc/cntrl /var/log/cntrl
    dscl . -delete /Users/_cntrl 2>/dev/null || true
    dscl . -delete /Groups/_cntrl 2>/dev/null || true
    echo "Removed the agent, its identity, config and logs, and the _cntrl user."
else
    echo "Removed the agent. Its identity, config and logs stay; --purge removes them too."
fi
