#!/bin/sh
# Installs the cntrl agent on Linux (systemd) or macOS (launchd) and enrolls it
# with Console (D14, D21). Console serves this script with each build's
# download and SHA-256 filled in at the top: with a single-use token for Add
# device's command, or without one for any machine, which enrolls afterwards.
#
#   curl -fsSL https://gw.cntrl.pw/install/<token> | sudo sh
#   curl -fsSL https://cntrl.pw/install.sh | sudo sh [-s -- --token <token>]
#
# On a machine that's in another account already, a token moves it there after
# asking on the terminal; `sh -s -- --move` moves it without asking (D23).
#
# By hand, from a release archive or from a build in this repo:
#
#   sudo sh install.sh --archive cntrl-agent-<version>-<target>.tar.gz [--token <token>]
#   sudo sh packaging/install.sh --binary target/release/cntrl-agent [--token <token>]
#
# Options: --console URL (where enrollment goes, default https://gw.cntrl.pw),
# --gateway URL (a gateway to use in place of the one enrollment returns),
# --move (move the machine from another account without asking), --force
# (install Console's release even when it, or a newer agent, is installed).
# The config is written only when there is none. Running Console's command
# again updates the agent when the release is newer, and otherwise only makes
# sure it runs; either way the agent keeps its identity. Everything runs from
# main(), called on the last line, so a download cut short runs nothing.

set -eu

: "${CNTRL_TOKEN:=}"
: "${CNTRL_CONSOLE:=https://gw.cntrl.pw}"
: "${CNTRL_GATEWAY:=}"
: "${CNTRL_VERSION:=}"
# One line per build: <target> <url> <size> <sha256>.
: "${CNTRL_ARTIFACTS:=}"

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'cntrl install: %s\n' "$*" >&2
    exit 1
}

# The Rust target this machine needs.
target() {
    case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) fail "this CPU ($(uname -m)) isn't supported yet" ;;
    esac
    case $(uname -s) in
    Linux) echo "$arch-unknown-linux-musl" ;;
    Darwin) echo "$arch-apple-darwin" ;;
    *) fail "this OS ($(uname -s)) isn't supported yet" ;;
    esac
}

# The version of an agent binary, such as 0.1.2; nothing when there's none.
version_of() {
    if [ -x "$1" ]; then
        "$1" --version 2>/dev/null | awk '{ print $2 }'
    fi
}

# Whether version $1 comes before $2, comparing each part as a number.
older() {
    [ "$1" != "$2" ] &&
        [ "$(printf '%s\n%s\n' "$1" "$2" | sort -t . -k 1,1n -k 2,2n -k 3,3n | head -n 1)" = "$1" ]
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

download() {
    if command -v curl >/dev/null 2>&1; then
        # A bar on a terminal, since a slow link would otherwise look like a hang.
        if [ -t 2 ]; then
            curl -fL --retry 3 --progress-bar -o "$2" "$1"
        else
            curl -fsSL --retry 3 -o "$2" "$1"
        fi
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        fail "this needs curl or wget to download the agent"
    fi
}

# Downloads this machine's build and checks it against the SHA-256 Console
# verified from the signed release manifest.
fetch() {
    line=$(printf '%s\n' "$CNTRL_ARTIFACTS" | awk -v t="$1" '$1 == t { print; exit }')
    [ -n "$line" ] || fail "release $CNTRL_VERSION has no build for $1"
    url=$(printf '%s\n' "$line" | awk '{ print $2 }')
    want=$(printf '%s\n' "$line" | awk '{ print $4 }')
    size=$(printf '%s\n' "$line" | awk '{ printf "%.1f MB", $3 / 1048576 }')
    say "Downloading cntrl agent $CNTRL_VERSION for $1 ($size)"
    download "$url" "$2"
    got=$(sha256 "$2")
    [ "$got" = "$want" ] || fail "the download doesn't match the release's SHA-256 (got $got)"
}

write_config() {
    if [ -e /etc/cntrl/agent.toml ]; then
        say "Keeping /etc/cntrl/agent.toml"
        return
    fi
    {
        echo "# The cntrl agent's config; \`cntrl config check\` validates it."
        echo "[console]"
        echo "url = \"$CNTRL_CONSOLE\""
        if [ -n "$CNTRL_GATEWAY" ]; then echo "gateway_url = \"$CNTRL_GATEWAY\""; fi
    } >/etc/cntrl/agent.toml
    chmod 0644 /etc/cntrl/agent.toml
}

install_linux() {
    command -v systemctl >/dev/null 2>&1 || fail "this needs systemd"
    if ! id -u cntrl >/dev/null 2>&1; then
        say "Creating the cntrl user"
        useradd --system --user-group --no-create-home --shell /usr/sbin/nologin cntrl
    fi
    # Members of systemd-journal read the system journal, which the agent
    # streams for logs (angle 11). Done on every install, so updates get it.
    if getent group systemd-journal >/dev/null 2>&1; then
        usermod -a -G systemd-journal cntrl
    fi
    # Stop both halves so the binary isn't replaced under them, and so privd
    # starts again from the new one.
    systemctl stop cntrl-agent.service cntrl-privd.service 2>/dev/null || true
    install -d -m 0755 /usr/local/bin /etc/cntrl
    install -m 0755 "$1/cntrl-agent" "$bin"
    ln -sf cntrl-agent /usr/local/bin/cntrl
    write_config
    for unit in "$1"/packaging/systemd/*; do
        install -m 0644 "$unit" /etc/systemd/system/
    done
    systemctl daemon-reload
    start_linux
}

start_linux() {
    # Forget earlier failures, so systemd's limit on quick restarts can't keep
    # a repaired agent from starting.
    systemctl reset-failed cntrl-agent.service cntrl-privd.service 2>/dev/null || true
    systemctl enable --now cntrl-privd.socket cntrl-agent.service
}

# A free ID below 500, where macOS keeps service accounts, unused by any user
# or group.
free_id() {
    used=$( (dscl . -list /Users UniqueID && dscl . -list /Groups PrimaryGroupID) | awk '{ print $2 }')
    id=300
    while [ "$id" -lt 500 ]; do
        if ! printf '%s\n' "$used" | grep -qx "$id"; then
            echo "$id"
            return
        fi
        id=$((id + 1))
    done
    fail "no free ID below 500 for _cntrl"
}

install_macos() {
    link=$(readlink /usr/local/bin/cntrl 2>/dev/null || true)
    if [ -e /usr/local/bin/cntrl ] && [ "$link" != "$bin" ] && [ "$link" != cntrl-agent ]; then
        fail "/usr/local/bin/cntrl is something else; move it first"
    fi
    if ! dscl . -read /Users/_cntrl >/dev/null 2>&1; then
        id=$(free_id)
        say "Creating the _cntrl user and group ($id)"
        dscl . -create /Groups/_cntrl
        dscl . -create /Groups/_cntrl PrimaryGroupID "$id"
        dscl . -create /Groups/_cntrl RealName "cntrl agent"
        dscl . -create /Groups/_cntrl Password "*"
        dscl . -create /Users/_cntrl
        dscl . -create /Users/_cntrl UniqueID "$id"
        dscl . -create /Users/_cntrl PrimaryGroupID "$id"
        dscl . -create /Users/_cntrl RealName "cntrl agent"
        dscl . -create /Users/_cntrl UserShell /usr/bin/false
        dscl . -create /Users/_cntrl NFSHomeDirectory /var/empty
        dscl . -create /Users/_cntrl Password "*"
        dscl . -create /Users/_cntrl IsHidden 1
    fi
    gid=$(dscl . -read /Users/_cntrl PrimaryGroupID | awk '{ print $2 }')
    for label in pw.cntrl.agent pw.cntrl.privd; do
        unload "$label"
    done
    support="/Library/Application Support/cntrl"
    install -d -m 0755 -o root -g wheel /etc/cntrl "$support" "$support/bin" /var/log/cntrl
    install -m 0755 -o root -g wheel "$1/cntrl-agent" "$bin"
    install -d -m 0755 /usr/local/bin
    ln -sf "$bin" /usr/local/bin/cntrl
    # Test builds before 0.1.0 installed it in /usr/local/bin.
    rm -f /usr/local/bin/cntrl-agent
    install -d -m 0700 -o _cntrl -g _cntrl "$support/agent"
    install -d -m 0700 -o root -g wheel "$support/privd"
    touch /var/log/cntrl/agent.log /var/log/cntrl/privd.log
    chown _cntrl:_cntrl /var/log/cntrl/agent.log
    chmod 0640 /var/log/cntrl/agent.log
    chmod 0600 /var/log/cntrl/privd.log
    write_config
    for label in pw.cntrl.privd pw.cntrl.agent; do
        plist="/Library/LaunchDaemons/$label.plist"
        sed "s/@GID@/$gid/" "$1/packaging/macos/$label.plist" >"$plist"
        chown root:wheel "$plist"
        chmod 0644 "$plist"
        plutil -lint -s "$plist"
        launchctl bootstrap system "$plist"
    done
}

# Stops a launchd job and waits for launchd to let it go. bootout returns
# while the job is still exiting, and loading it again before it's gone fails
# with "Bootstrap failed: 5: Input/output error".
unload() {
    launchctl bootout "system/$1" 2>/dev/null || true
    tries=0
    while launchctl print "system/$1" >/dev/null 2>&1; do
        tries=$((tries + 1))
        # launchd kills a job that hasn't exited after 20 s.
        [ "$tries" -lt 120 ] || fail "$1 didn't stop; see /var/log/cntrl"
        sleep 0.25
    done
}

# Loads the jobs that aren't loaded, as after an install that stopped halfway.
start_macos() {
    for label in pw.cntrl.privd pw.cntrl.agent; do
        if ! launchctl print "system/$label" >/dev/null 2>&1; then
            launchctl bootstrap system "/Library/LaunchDaemons/$label.plist"
        fi
    done
}

# Waits for the agent, then hands it the token, if there is one. The agent
# enrolls, says the machine is in that account already, or asks before moving
# it from another (D23).
enroll() {
    tries=0
    until "$bin" status >/dev/null 2>&1; do
        tries=$((tries + 1))
        [ "$tries" -lt 20 ] || fail "the agent didn't start; see its log"
        sleep 0.5
    done
    if [ -n "$CNTRL_TOKEN" ]; then
        printf '%s\n' "$CNTRL_TOKEN" | CNTRL_INSTALLER=1 "$bin" enroll $move
    elif "$bin" status 2>/dev/null | grep -q '^uplink: not enrolled'; then
        say "To add this machine, run the command from Console's Add device dialog."
    else
        say "This machine is enrolled already."
    fi
}

main() {
    here=$(dirname "$0")
    archive=
    binary=
    move=
    force=
    while [ $# -gt 0 ]; do
        case $1 in
        --archive) archive=${2:?--archive needs a file}; shift 2 ;;
        --binary) binary=${2:?--binary needs a file}; shift 2 ;;
        --token) CNTRL_TOKEN=${2:?--token needs a token}; shift 2 ;;
        --console) CNTRL_CONSOLE=${2:?--console needs a URL}; shift 2 ;;
        --gateway) CNTRL_GATEWAY=${2:?--gateway needs a URL}; shift 2 ;;
        --move) move=--move; shift ;;
        --force) force=1; shift ;;
        -h | --help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) fail "unknown option $1" ;;
        esac
    done
    [ "$(id -u)" -eq 0 ] || fail "run it as root, with sudo"
    tgt=$(target)
    # On macOS the binary root runs stays out of /usr/local: Homebrew on Intel
    # Macs gave the user /usr/local/bin and /usr/local/lib, so any program
    # that user ran could replace it.
    case $tgt in
    *-linux-*) bin=/usr/local/bin/cntrl-agent ;;
    *-darwin) bin="/Library/Application Support/cntrl/bin/cntrl-agent" ;;
    esac
    had=$(version_of "$bin")

    # Console's release, when it or a newer agent is installed already: keep
    # that one, make sure it runs, and go on to enrolling.
    if [ -z "$binary$archive$force" ] && [ -n "$had" ] && [ -n "$CNTRL_VERSION" ] &&
        ! older "$had" "$CNTRL_VERSION"; then
        if [ "$had" = "$CNTRL_VERSION" ]; then
            say "cntrl agent $had, the latest release, is installed already."
        else
            say "Keeping cntrl agent $had, which is newer than the latest release, $CNTRL_VERSION."
        fi
        case $tgt in
        *-linux-*) start_linux ;;
        *-darwin) start_macos ;;
        esac
        enroll
        return
    fi

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    if [ -n "$binary" ]; then
        # A build from this repo: the packaging files sit beside this script.
        mkdir -p "$work/files"
        cp "$binary" "$work/files/cntrl-agent"
        cp -R "$here" "$work/files/packaging"
    else
        if [ -z "$archive" ]; then
            [ -n "$CNTRL_ARTIFACTS" ] || fail "nothing to install: use Console's command, --archive or --binary"
            archive="$work/agent.tar.gz"
            fetch "$tgt" "$archive"
        fi
        mkdir -p "$work/files"
        tar -xzf "$archive" -C "$work/files" --strip-components 1
    fi
    [ -x "$work/files/cntrl-agent" ] || fail "the archive has no cntrl-agent"
    new=$(version_of "$work/files/cntrl-agent")

    case $tgt in
    *-linux-*) install_linux "$work/files" ;;
    *-darwin) install_macos "$work/files" ;;
    esac
    if [ -z "$had" ]; then
        say "Installed cntrl agent $new as $bin, also called cntrl."
    elif [ "$had" = "$new" ]; then
        say "Reinstalled cntrl agent $new."
    elif older "$new" "$had"; then
        say "Downgraded the cntrl agent from $had to $new."
    else
        say "Updated the cntrl agent from $had to $new."
    fi
    enroll
}

main "$@"
