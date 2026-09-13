#!/bin/sh
# Install imsg-server and register it as a per-user macOS LaunchAgent.
#
# Works in three situations:
#   * from a source checkout (scripts/install-server.sh): builds with cargo first
#   * from an unpacked release package (imsg-server and imsg next to this script)
#   * as the installed helper ~/.local/bin/imsg-service (status, restart, token, ...)
set -eu
umask 077
LABEL=org.thescruggs.imsg-server
RELEASE_REPO=${IMSG_REPO:-thescruggs/imessage-cli}
[ -n "${HOME:-}" ] || { echo "install-server: HOME is not set" >&2; exit 1; }
BIN_DIR=$HOME/.local/bin
AGENT_DIR=$HOME/Library/LaunchAgents
CONFIG_DIR=$HOME/.config/imsg
PLIST=$AGENT_DIR/$LABEL.plist
SERVER=$BIN_DIR/imsg-server
CLI=$BIN_DIR/imsg
HELPER=$BIN_DIR/imsg-service
TOKEN_FILE=$CONFIG_DIR/server.toml
LOG_FILE=$CONFIG_DIR/server.log
BIND=${IMSG_BIND:-0.0.0.0:8787}
ACTION=install
NO_BUILD=0
OPEN_SETTINGS=0
BINARIES=
usage() {
    cat <<EOF
Usage: $0 [options]
Install imsg-server and start it automatically at login.

  --bind ADDRESS   Address the server listens on (default: IMSG_BIND or $BIND)
                   Use 127.0.0.1:8787 to allow only this Mac.
  --binaries DIR   Install imsg-server and imsg from DIR instead of building
  --no-build       Install existing target/release binaries from the source tree
  --open-settings  After installing, open System Settings at Full Disk Access
  --status         Show whether the service is registered and running
  --restart        Restart the service
  --token          Print the server token
  --url            Print the addresses to open in a browser (with login link)
  --open           Open the web client in the default browser, already logged in
  --update         Download and install the latest release from GitHub
  --uninstall      Stop the service and remove its registration only
  --help           Show this help
EOF
}
die() { echo "install-server: $*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
    case $1 in
        --bind)
            [ "$#" -ge 2 ] || die "--bind requires ADDRESS"
            case $2 in --*) die "--bind requires ADDRESS" ;; esac
            BIND=$2; shift 2 ;;
        --bind=*) BIND=${1#--bind=}; shift ;;
        --binaries)
            [ "$#" -ge 2 ] || die "--binaries requires DIR"
            BINARIES=$2; NO_BUILD=1; shift 2 ;;
        --binaries=*) BINARIES=${1#--binaries=}; NO_BUILD=1; shift ;;
        --no-build) NO_BUILD=1; shift ;;
        --open-settings) OPEN_SETTINGS=1; shift ;;
        --uninstall) ACTION=uninstall; shift ;;
        --status) ACTION=status; shift ;;
        --restart) ACTION=restart; shift ;;
        --token) ACTION=token; shift ;;
        --url) ACTION=url; shift ;;
        --open) ACTION=open; shift ;;
        --update) ACTION=update; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done
[ "$(uname -s)" = Darwin ] || die "this installer supports macOS only"
[ "$(id -u)" -ne 0 ] || die "do not run as root or with sudo; this is a per-user service"
[ -d "$HOME" ] || die "HOME does not point to a directory"
[ -n "$BIND" ] || die "bind address cannot be empty"
case $BIND in *[[:cntrl:]]*) die "bind address contains control characters" ;; esac
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd -P)
USER_ID=$(id -u)
DOMAIN=gui/$USER_ID
command_exists() { command -v "$1" >/dev/null 2>&1; }
command_exists launchctl || die "launchctl was not found"
command_exists plutil || die "plutil was not found"
service_exists() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }
domain_exists() { launchctl print "$DOMAIN" >/dev/null 2>&1; }
stop_service() {
    if service_exists; then
        echo "Stopping $LABEL"
        launchctl bootout "$DOMAIN/$LABEL"
    fi
}
read_token() {
    [ -f "$TOKEN_FILE" ] || return 1
    sed -n 's/^[[:space:]]*token[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$TOKEN_FILE" | head -n 1 | grep .
}
port_of() {
    case $BIND in
        *:*) printf '%s\n' "${BIND##*:}" ;;
        *) echo 8787 ;;
    esac
}
installed_port() {
    # Read the port back from the installed plist so --url matches what is running.
    if [ -f "$PLIST" ] && command_exists plutil; then
        addr=$(plutil -extract ProgramArguments.2 raw -o - "$PLIST" 2>/dev/null || true)
        case $addr in *:*) printf '%s\n' "${addr##*:}"; return ;; esac
    fi
    port_of
}
lan_ip() {
    for iface in en0 en1 en2 en3; do
        ip=$(ipconfig getifaddr "$iface" 2>/dev/null || true)
        if [ -n "$ip" ]; then printf '%s\n' "$ip"; return; fi
    done
    return 1
}
print_urls() {
    port=$(installed_port)
    token=$(read_token || true)
    suffix=
    if [ -n "$token" ]; then suffix="#token=$token"; fi
    echo "On this Mac:            http://localhost:$port/$suffix"
    host=$(scutil --get LocalHostName 2>/dev/null || true)
    if [ -n "$host" ]; then echo "From another device:    http://$host.local:$port/$suffix"; fi
    ip=$(lan_ip || true)
    if [ -n "$ip" ]; then echo "  or by IP address:     http://$ip:$port/$suffix"; fi
    if [ -z "$token" ]; then
        echo "(no token yet: the server has not started; run --restart and try again)"
    else
        echo "The part after # logs the browser in automatically. Keep these links private."
    fi
}
case $ACTION in
    status)
        if service_exists; then
            echo "$LABEL is registered in $DOMAIN"
            launchctl print "$DOMAIN/$LABEL"
            exit 0
        fi
        echo "$LABEL is not registered in $DOMAIN"
        exit 1
        ;;
    uninstall)
        stop_service
        if [ -e "$PLIST" ]; then rm -f "$PLIST"; echo "Removed $PLIST"; fi
        echo "Uninstalled $LABEL (binaries and data were left in place)."
        exit 0
        ;;
    restart)
        [ -f "$PLIST" ] || die "no installed LaunchAgent; run the installer first"
        launchctl enable "$DOMAIN/$LABEL"
        if service_exists; then
            launchctl kickstart -k "$DOMAIN/$LABEL"
        else
            launchctl bootstrap "$DOMAIN" "$PLIST"
        fi
        echo "$LABEL restarted in $DOMAIN"
        exit 0
        ;;
    token)
        read_token || die "no token yet; the server has not started (see $LOG_FILE)"
        exit 0
        ;;
    url)
        print_urls
        exit 0
        ;;
    open)
        token=$(read_token) || die "no token yet; the server has not started (see $LOG_FILE)"
        open "http://localhost:$(installed_port)/#token=$token"
        exit 0
        ;;
    update)
        command_exists curl || die "curl was not found"
        echo "Downloading the latest release of imsg from github.com/$RELEASE_REPO"
        curl -fsSL --proto '=https' "https://github.com/$RELEASE_REPO/releases/latest/download/install.sh" | IMSG_BIND=$BIND sh
        exit 0
        ;;
esac

# ---- locate the binaries to install ----
if [ -n "$BINARIES" ]; then
    SOURCE_DIR=$BINARIES
elif [ -x "$SCRIPT_DIR/imsg-server" ] && [ -x "$SCRIPT_DIR/imsg" ]; then
    # Unpacked release package (or the installed helper re-registering itself).
    SOURCE_DIR=$SCRIPT_DIR
    NO_BUILD=1
else
    REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd -P)
    if [ "$NO_BUILD" -eq 0 ]; then
        CARGO=
        if command_exists cargo; then CARGO=$(command -v cargo)
        else
            for candidate in "$HOME/.cargo/bin/cargo" /opt/homebrew/opt/rustup/bin/cargo /usr/local/opt/rustup/bin/cargo; do
                if [ -x "$candidate" ]; then CARGO=$candidate; break; fi
            done
        fi
        [ -n "$CARGO" ] || die "cargo was not found in PATH or standard rustup locations"
        echo "Building release binaries"
        "$CARGO" build --release --locked --target-dir "$REPO_ROOT/target" --manifest-path "$REPO_ROOT/Cargo.toml"
    fi
    SOURCE_DIR=$REPO_ROOT/target/release
fi
SOURCE_SERVER=$SOURCE_DIR/imsg-server
SOURCE_CLI=$SOURCE_DIR/imsg
[ -x "$SOURCE_SERVER" ] || die "missing executable: $SOURCE_SERVER (build first or omit --no-build)"
[ -x "$SOURCE_CLI" ] || die "missing executable: $SOURCE_CLI (build first or omit --no-build)"
mkdir -p "$BIN_DIR" "$AGENT_DIR" "$CONFIG_DIR"
SERVER_TMP=
CLI_TMP=
HELPER_TMP=
temporary_plist=
cleanup() { rm -f "$SERVER_TMP" "$CLI_TMP" "$HELPER_TMP" "$temporary_plist"; }
trap cleanup 0
trap 'exit 1' 1 2 3 15
same_file() { [ "$1" -ef "$2" ]; }
stage_file() {
    # Copy $1 next to $2 under a temporary name; prints the temporary path, or
    # nothing when source and destination are already the same file.
    source=$1; destination=$2
    if same_file "$source" "$destination"; then return 0; fi
    temporary=$(mktemp "$destination.tmp.XXXXXX") || die "could not create temporary file"
    cp "$source" "$temporary" || { rm -f "$temporary"; die "could not copy $source"; }
    chmod 700 "$temporary"
    # Files downloaded through a browser carry a quarantine flag that makes
    # macOS refuse to run them; release packages are fetched with curl, but
    # clear it anyway so a manually downloaded package works too.
    xattr -d com.apple.quarantine "$temporary" >/dev/null 2>&1 || true
    printf '%s\n' "$temporary"
}
SERVER_TMP=$(stage_file "$SOURCE_SERVER" "$SERVER")
CLI_TMP=$(stage_file "$SOURCE_CLI" "$CLI")
HELPER_TMP=$(stage_file "$SCRIPT_DIR/$(basename "$0")" "$HELPER")
xml_escape() { printf '%s' "$1" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
XML_HOME=$(xml_escape "$HOME")
XML_SERVER=$(xml_escape "$SERVER")
XML_BIND=$(xml_escape "$BIND")
XML_LOG=$(xml_escape "$LOG_FILE")
temporary_plist=$(mktemp "$AGENT_DIR/.$LABEL.XXXXXX") || die "could not create temporary plist"
cat > "$temporary_plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$XML_SERVER</string><string>--bind</string><string>$XML_BIND</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>5</integer>
  <key>StandardOutPath</key><string>$XML_LOG</string>
  <key>StandardErrorPath</key><string>$XML_LOG</string>
  <key>EnvironmentVariables</key><dict><key>HOME</key><string>$XML_HOME</string></dict>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>
EOF
plutil -lint "$temporary_plist" >/dev/null || die "generated plist failed validation"
stop_service
[ -z "$SERVER_TMP" ] || mv -f "$SERVER_TMP" "$SERVER"
[ -z "$CLI_TMP" ] || mv -f "$CLI_TMP" "$CLI"
[ -z "$HELPER_TMP" ] || mv -f "$HELPER_TMP" "$HELPER"
mv -f "$temporary_plist" "$PLIST"
SERVER_TMP=; CLI_TMP=; HELPER_TMP=; temporary_plist=
trap - 0 1 2 3 15
if [ -f "$TOKEN_FILE" ]; then chmod 600 "$TOKEN_FILE"; fi
if [ -f "$LOG_FILE" ]; then chmod 600 "$LOG_FILE"; fi
# An existing token means this is an update, not a first install.
UPDATING=0
if read_token >/dev/null 2>&1; then UPDATING=1; fi
STARTED=0
if domain_exists; then
    launchctl enable "$DOMAIN/$LABEL"
    launchctl bootstrap "$DOMAIN" "$PLIST"
    STARTED=1
    echo "Installed and registered $LABEL for GUI login in $DOMAIN"
else
    echo "Installed $LABEL; GUI login domain is unavailable, so it will register at the next login."
fi
echo "Binary:  $SERVER"
echo "Helper:  $HELPER  (--status, --restart, --token, --url, --open, --update, --uninstall)"
echo "Bind:    $BIND"
echo "Log:     $LOG_FILE"
# The server writes its token before it touches the Messages database, so the
# token appears within a moment even when Full Disk Access is still missing.
if [ "$STARTED" -eq 1 ]; then
    tries=0
    while [ "$tries" -lt 20 ] && ! read_token >/dev/null 2>&1; do
        sleep 0.5
        tries=$((tries + 1))
    done
fi
echo
echo "================ NEXT STEPS ================"
echo "1. Give imsg-server permission to read your Messages:"
echo "   System Settings > Privacy & Security > Full Disk Access"
echo "   Click +, press Command-Shift-G, enter $SERVER and click Open,"
echo "   then make sure the switch next to imsg-server is turned on."
echo "   The server retries by itself every few seconds; no restart needed."
echo "2. Open the web client (this link logs you in automatically):"
print_urls | sed 's/^/   /'
echo "3. The first time you send a message, macOS asks whether imsg-server may"
echo "   control Messages. Click Allow (or OK)."
if [ "$UPDATING" -eq 1 ]; then
    echo
    echo "You just updated imsg. macOS forgets Full Disk Access when the program"
    echo "changes: if messages do not load, open the Full Disk Access list, select"
    echo "imsg-server, click - to remove it, then add it again as in step 1."
fi
echo "============================================"
if [ "$OPEN_SETTINGS" -eq 1 ] && command_exists open; then
    # Show the binary in Finder so it can be dragged into the Full Disk Access
    # list, and open that settings pane.
    open -R "$SERVER" >/dev/null 2>&1 || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles" >/dev/null 2>&1 || true
fi
