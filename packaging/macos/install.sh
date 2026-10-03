#!/bin/sh
# Installs the cntrl agent on macOS from a build of cntrl-agent (D20): the
# _cntrl user, the binary, its directories and config, and the two launchd
# jobs. Signed packages come later; this is for development and early users.
#
# Usage: sudo packaging/macos/install.sh [--console URL] [--gateway URL] [BINARY]
#   BINARY     the cntrl-agent build to install (default target/release/cntrl-agent)
#   --console  where enrollment goes (default https://gw.cntrl.pw); written to
#              /etc/cntrl/agent.toml only when that file doesn't exist yet
#   --gateway  a gateway URL to use instead of the one enrollment returns
# Run it again to update the agent. Then enroll with the command from
# Console's Add device dialog.

set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BINARY=target/release/cntrl-agent
CONSOLE=https://gw.cntrl.pw
GATEWAY=
AGENT_USER=_cntrl
SUPPORT="/Library/Application Support/cntrl"
LABELS="pw.cntrl.privd pw.cntrl.agent"

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case $1 in
    --console) CONSOLE=${2:?--console needs a URL}; shift 2 ;;
    --gateway) GATEWAY=${2:?--gateway needs a URL}; shift 2 ;;
    -h | --help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) fail "unknown option $1" ;;
    *) BINARY=$1; shift ;;
    esac
done

[ "$(uname -s)" = Darwin ] || fail "this installs on macOS"
[ "$(id -u)" -eq 0 ] || fail "run it with sudo"
[ -x "$BINARY" ] || fail "no agent build at $BINARY; run cargo build --release -p cntrl-agent first"
if [ -e /usr/local/bin/cntrl ] && [ "$(readlink /usr/local/bin/cntrl)" != cntrl-agent ]; then
    fail "/usr/local/bin/cntrl is something else; move it before installing"
fi

# A free ID below 500, where macOS keeps service accounts, unused by any user
# or group.
free_id() {
    used=$( (dscl . -list /Users UniqueID && dscl . -list /Groups PrimaryGroupID) | awk '{ print $2 }')
    id=300
    while [ "$id" -lt 500 ]; do
        if ! echo "$used" | grep -qx "$id"; then
            echo "$id"
            return
        fi
        id=$((id + 1))
    done
    fail "no free ID below 500 for $AGENT_USER"
}

if ! dscl . -read "/Users/$AGENT_USER" >/dev/null 2>&1; then
    id=$(free_id)
    echo "Creating the $AGENT_USER user and group ($id)"
    dscl . -create "/Groups/$AGENT_USER"
    dscl . -create "/Groups/$AGENT_USER" PrimaryGroupID "$id"
    dscl . -create "/Groups/$AGENT_USER" RealName "cntrl agent"
    dscl . -create "/Groups/$AGENT_USER" Password "*"
    dscl . -create "/Users/$AGENT_USER"
    dscl . -create "/Users/$AGENT_USER" UniqueID "$id"
    dscl . -create "/Users/$AGENT_USER" PrimaryGroupID "$id"
    dscl . -create "/Users/$AGENT_USER" RealName "cntrl agent"
    dscl . -create "/Users/$AGENT_USER" UserShell /usr/bin/false
    dscl . -create "/Users/$AGENT_USER" NFSHomeDirectory /var/empty
    dscl . -create "/Users/$AGENT_USER" Password "*"
    dscl . -create "/Users/$AGENT_USER" IsHidden 1
fi
GID=$(dscl . -read "/Users/$AGENT_USER" PrimaryGroupID | awk '{ print $2 }')

# Stop both jobs before replacing the binary under them.
for label in $LABELS; do
    launchctl bootout "system/$label" 2>/dev/null || true
done

echo "Installing $BINARY as /usr/local/bin/cntrl-agent, also called cntrl"
install -d -m 0755 /usr/local/bin
install -m 0755 "$BINARY" /usr/local/bin/cntrl-agent
ln -sf cntrl-agent /usr/local/bin/cntrl

install -d -m 0755 -o root -g wheel /etc/cntrl "$SUPPORT" /var/log/cntrl
install -d -m 0700 -o "$AGENT_USER" -g "$AGENT_USER" "$SUPPORT/agent"
install -d -m 0700 -o root -g wheel "$SUPPORT/privd"
touch /var/log/cntrl/agent.log /var/log/cntrl/privd.log
chown "$AGENT_USER:$AGENT_USER" /var/log/cntrl/agent.log
chmod 0640 /var/log/cntrl/agent.log
chmod 0600 /var/log/cntrl/privd.log

if [ -e /etc/cntrl/agent.toml ]; then
    echo "Keeping /etc/cntrl/agent.toml"
else
    {
        echo "# The cntrl agent's config; \`cntrl config check\` validates it."
        echo "[console]"
        echo "url = \"$CONSOLE\""
        if [ -n "$GATEWAY" ]; then echo "gateway_url = \"$GATEWAY\""; fi
    } >/etc/cntrl/agent.toml
    chmod 0644 /etc/cntrl/agent.toml
fi

for label in $LABELS; do
    plist="/Library/LaunchDaemons/$label.plist"
    sed "s/@GID@/$GID/" "$HERE/$label.plist" >"$plist"
    chown root:wheel "$plist"
    chmod 0644 "$plist"
    plutil -lint -s "$plist"
    launchctl bootstrap system "$plist"
done

echo
echo "Installed. The agent runs as $AGENT_USER; sudo cntrl status shows how it is."
echo "To add this Mac, run the command from Console's Add device dialog:"
echo "  echo '<token>' | sudo cntrl enroll"
