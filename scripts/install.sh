#!/usr/bin/env bash
# =============================================================================
# SoundSync Installer
# =============================================================================
#
# Installs and configures SoundSync — Bluetooth A2DP Sink with DSP EQ.
#
# Supports:
#   - Ubuntu 24.04 LTS (x86_64)
#   - Fresh install and re-install (idempotent)
#   - Custom port selection
#   - User systemd service (runs as current user, not root)
#
# Usage:
#   chmod +x install.sh
#   ./install.sh [--port PORT] [--name NAME] [--adapter ADAPTER] [--uninstall]
#
# =============================================================================

set -euo pipefail

# ── Colour output ─────────────────────────────────────────────────────────────
RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[0;33m'
BLU='\033[0;34m'
CYN='\033[0;36m'
BLD='\033[1m'
RST='\033[0m'

info()    { echo -e "${BLU}[INFO]${RST}  $*"; }
success() { echo -e "${GRN}[OK]${RST}    $*"; }
warn()    { echo -e "${YLW}[WARN]${RST}  $*"; }
error()   { echo -e "${RED}[ERROR]${RST} $*" >&2; }
header()  { echo -e "\n${BLD}${CYN}══ $* ══${RST}"; }

# ── Script metadata ───────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
APP_NAME="soundsync"
APP_VERSION="1.2.0"
INSTALL_USER="${USER:-$(whoami)}"
INSTALL_BIN="${HOME}/.local/bin"
CONFIG_DIR="${HOME}/.config/soundsync"
SERVICE_DIR="${HOME}/.config/systemd/user"
SERVICE_FILE="${SERVICE_DIR}/soundsync.service"
LOG_DIR="${HOME}/.local/share/soundsync"

# ── Defaults ──────────────────────────────────────────────────────────────────
DEFAULT_PORT=8080
PORT=${DEFAULT_PORT}
PORT_EXPLICIT=false       # set to true when user passes --port explicitly
DEVICE_NAME="SoundSync"
ADAPTER="hci0"
UNINSTALL=false
BUILD_RELEASE=true

# ── Parse arguments ───────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --port|-p)
            PORT="$2"
            PORT_EXPLICIT=true
            shift 2
            ;;
        --name|-n)
            DEVICE_NAME="$2"
            shift 2
            ;;
        --adapter|-a)
            ADAPTER="$2"
            shift 2
            ;;
        --uninstall)
            UNINSTALL=true
            shift
            ;;
        --dev)
            BUILD_RELEASE=false
            shift
            ;;
        --help|-h)
            echo "SoundSync Installer v${APP_VERSION}"
            echo ""
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --port PORT        Web UI port (default: 8080)"
            echo "  --name NAME        Bluetooth device name (default: SoundSync)"
            echo "  --adapter ADAPTER  Bluetooth adapter (default: hci0)"
            echo "  --uninstall        Remove SoundSync and all configuration"
            echo "  --dev              Build in debug mode (faster, larger binary)"
            echo "  --help             Show this help"
            exit 0
            ;;
        *)
            error "Unknown argument: $1"
            exit 1
            ;;
    esac
done

# ── Validation ────────────────────────────────────────────────────────────────

# Returns 0 (true) if $1 is available, 1 if in use.
is_port_in_use() {
    local p="$1"
    # Try ss first (iproute2), then lsof, then a /dev/tcp probe
    if command -v ss &>/dev/null; then
        ss -tlnH "sport = :${p}" 2>/dev/null | grep -q ":${p}" && return 0
    fi
    if command -v lsof &>/dev/null; then
        lsof -iTCP:"${p}" -sTCP:LISTEN -t &>/dev/null && return 0
    fi
    # Fallback: try binding (requires bash /dev/tcp support)
    (echo "" > /dev/tcp/127.0.0.1/"${p}") 2>/dev/null && return 0
    return 1  # port appears free
}

# Find the next free port starting from $1.
find_free_port() {
    local p="$1"
    while is_port_in_use "${p}"; do
        ((p++))
        if [ "${p}" -gt 65535 ]; then
            error "Could not find a free port above ${1}."
            exit 1
        fi
    done
    echo "${p}"
}

validate_port() {
    if ! [[ "${PORT}" =~ ^[0-9]+$ ]] || [ "${PORT}" -lt 1024 ] || [ "${PORT}" -gt 65535 ]; then
        error "Port must be between 1024 and 65535, got: ${PORT}"
        exit 1
    fi

    if is_port_in_use "${PORT}"; then
        warn "Port ${PORT} is already in use."
        local suggested
        suggested=$(find_free_port $((PORT + 1)))

        if [ "${PORT_EXPLICIT}" = true ]; then
            # The user specifically asked for this port — give them a choice.
            warn "You requested --port ${PORT} but it is taken."
        fi

        if [ -t 0 ]; then
            # Interactive terminal — prompt the user.
            echo -en "  ${BLD}Enter a different port${RST} [${suggested}]: "
            read -r user_port
            user_port="${user_port:-${suggested}}"
            if ! [[ "${user_port}" =~ ^[0-9]+$ ]] || [ "${user_port}" -lt 1024 ] || [ "${user_port}" -gt 65535 ]; then
                error "Invalid port: ${user_port}"
                exit 1
            fi
            if is_port_in_use "${user_port}"; then
                error "Port ${user_port} is also in use. Please free a port and retry."
                exit 1
            fi
            PORT="${user_port}"
        else
            # Non-interactive (e.g. piped script) — auto-select.
            info "Auto-selecting next free port: ${suggested}"
            PORT="${suggested}"
        fi
        success "Using port ${PORT}"
    fi
}

check_not_root() {
    if [ "$(id -u)" -eq 0 ]; then
        error "Do NOT run this installer as root."
        error "SoundSync runs as a user service. Run as your normal user:"
        error "  ./install.sh"
        exit 1
    fi
}

check_os() {
    if [ ! -f /etc/os-release ]; then
        warn "Cannot detect OS — proceeding anyway"
        return
    fi
    source /etc/os-release
    if [[ "${ID}" != "ubuntu" ]]; then
        warn "This installer is designed for Ubuntu 24.04 LTS."
        warn "Current OS: ${PRETTY_NAME}"
        warn "Proceeding anyway — some steps may need manual adjustment."
    fi
}

# ── Uninstall ─────────────────────────────────────────────────────────────────
do_uninstall() {
    header "Uninstalling SoundSync"

    # Stop and disable service
    if systemctl --user is-active --quiet soundsync 2>/dev/null; then
        info "Stopping soundsync service..."
        systemctl --user stop soundsync || true
    fi
    if systemctl --user is-enabled --quiet soundsync 2>/dev/null; then
        systemctl --user disable soundsync || true
    fi

    # Remove service file
    if [ -f "${SERVICE_FILE}" ]; then
        rm -f "${SERVICE_FILE}"
        systemctl --user daemon-reload
        success "Service file removed"
    fi

    # Remove binary
    if [ -f "${INSTALL_BIN}/soundsync" ]; then
        rm -f "${INSTALL_BIN}/soundsync"
        success "Binary removed"
    fi

    # Remove WirePlumber BT config written by the installer
    local wp_bt_conf="${HOME}/.config/wireplumber/bluetooth.lua.d/51-soundsync-a2dp.lua"
    if [ -f "${wp_bt_conf}" ]; then
        rm -f "${wp_bt_conf}"
        success "WirePlumber BT config removed"
        # Restart WirePlumber so it reverts to system defaults
        systemctl --user restart wireplumber 2>/dev/null || true
    fi

    echo ""
    echo -e "${GRN}SoundSync has been uninstalled.${RST}"
    echo "Config files in ${CONFIG_DIR} have been preserved."
    echo "To remove them: rm -rf ${CONFIG_DIR}"
    exit 0
}

# ── System packages ───────────────────────────────────────────────────────────
install_system_packages() {
    header "Installing system packages"

    # Check for apt
    if ! command -v apt-get &>/dev/null; then
        warn "apt-get not found — skipping package installation"
        warn "Please ensure these packages are installed:"
        warn "  bluez pipewire pipewire-alsa wireplumber libpipewire-0.3-dev"
        warn "  libdbus-1-dev pkg-config build-essential"
        return
    fi

    info "Updating package lists..."
    sudo apt-get update -qq

    # Core packages
    local packages=(
        # Bluetooth
        "bluez"
        "bluetooth"
        "bluez-tools"

        # PipeWire audio
        "pipewire"
        "pipewire-alsa"
        "pipewire-audio"
        "wireplumber"
        "libpipewire-0.3-dev"
        "libspa-0.2-dev"

        # D-Bus development
        "libdbus-1-dev"
        "dbus"

        # Build tools
        "pkg-config"
        "build-essential"
        "clang"
        "libclang-dev"
        "curl"

        # Audio capture and encoding
        "ffmpeg"
        "pulseaudio-utils"    # provides parec

        # Python (for bluetoothctl helper)
        "python3"
    )

    local to_install=()
    for pkg in "${packages[@]}"; do
        if ! dpkg -l "${pkg}" 2>/dev/null | grep -q '^ii'; then
            to_install+=("${pkg}")
        fi
    done

    if [ "${#to_install[@]}" -gt 0 ]; then
        info "Installing: ${to_install[*]}"
        sudo apt-get install -y -qq "${to_install[@]}"
        success "System packages installed"
    else
        success "All system packages already installed"
    fi
}

# ── Rust toolchain ────────────────────────────────────────────────────────────
install_rust() {
    header "Checking Rust toolchain"

    if command -v cargo &>/dev/null; then
        local rust_version
        rust_version=$(rustc --version 2>/dev/null | awk '{print $2}')
        success "Rust already installed: ${rust_version}"
        return
    fi

    info "Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

    # Source the cargo environment
    source "${HOME}/.cargo/env" 2>/dev/null || true
    export PATH="${HOME}/.cargo/bin:${PATH}"

    success "Rust installed: $(rustc --version)"
}

# ── BlueZ configuration ───────────────────────────────────────────────────────
configure_bluez() {
    header "Configuring BlueZ"

    # Enable and start the Bluetooth service
    if systemctl is-enabled --quiet bluetooth 2>/dev/null; then
        success "Bluetooth service already enabled"
    else
        info "Enabling Bluetooth service..."
        sudo systemctl enable bluetooth
        success "Bluetooth service enabled"
    fi

    if systemctl is-active --quiet bluetooth 2>/dev/null; then
        success "Bluetooth service already running"
    else
        info "Starting Bluetooth service..."
        sudo systemctl start bluetooth
        success "Bluetooth service started"
    fi

    # Configure BlueZ for A2DP sink operation
    local bluez_conf="/etc/bluetooth/main.conf"
    if [ -f "${bluez_conf}" ]; then
        # Enable auto-enable of adapters
        if ! grep -q "^AutoEnable=true" "${bluez_conf}"; then
            info "Configuring BlueZ auto-enable..."
            if grep -q "^AutoEnable=" "${bluez_conf}"; then
                sudo sed -i 's/^AutoEnable=.*/AutoEnable=true/' "${bluez_conf}"
            else
                # Add under [Policy] section or append
                if grep -q "^\[Policy\]" "${bluez_conf}"; then
                    sudo sed -i '/^\[Policy\]/a AutoEnable=true' "${bluez_conf}"
                else
                    echo -e "\n[Policy]\nAutoEnable=true" | sudo tee -a "${bluez_conf}" > /dev/null
                fi
            fi
            success "BlueZ AutoEnable configured"
        fi
    fi

    # Add user to bluetooth group for adapter access without sudo
    if ! groups "${INSTALL_USER}" | grep -q bluetooth; then
        info "Adding ${INSTALL_USER} to bluetooth group..."
        sudo usermod -aG bluetooth "${INSTALL_USER}"
        warn "You may need to log out and back in for group changes to take effect"
    else
        success "User ${INSTALL_USER} already in bluetooth group"
    fi
}

# ── PipeWire configuration ────────────────────────────────────────────────────
configure_pipewire() {
    header "Configuring PipeWire"

    # Ensure user PipeWire service is running
    if systemctl --user is-enabled --quiet pipewire 2>/dev/null; then
        success "PipeWire user service already enabled"
    else
        info "Enabling PipeWire user service..."
        systemctl --user enable pipewire pipewire-pulse wireplumber 2>/dev/null || true
        success "PipeWire user service enabled"
    fi

    if systemctl --user is-active --quiet pipewire 2>/dev/null; then
        success "PipeWire already running"
    else
        info "Starting PipeWire..."
        systemctl --user start pipewire pipewire-pulse wireplumber 2>/dev/null || warn "PipeWire not started — may need relogin"
    fi

    # WirePlumber Bluetooth policy configuration
    local wp_conf_dir="${HOME}/.config/wireplumber/bluetooth.lua.d"
    mkdir -p "${wp_conf_dir}"

    local wp_bt_conf="${wp_conf_dir}/51-soundsync-a2dp.lua"
    if [ ! -f "${wp_bt_conf}" ]; then
        info "Writing WirePlumber A2DP sink configuration..."
        cat > "${wp_bt_conf}" << 'LUAEOF'
-- SoundSync WirePlumber configuration
-- Enables this machine as an A2DP sink (receives audio from other devices)

bluez_monitor.enabled = true

bluez_monitor.properties = {
  ["bluez5.enable-sbc-xq"]    = true,
  ["bluez5.enable-msbc"]      = true,
  ["bluez5.enable-hw-volume"] = true,
  -- sbc_xq listed first so WirePlumber negotiates it before falling back to sbc
  ["bluez5.codecs"]           = "[ sbc_xq sbc aac ldac aptx aptx_hd ]",
  ["bluez5.roles"]            = "[ a2dp_sink hsp_hs hfp_hf ]",
}
LUAEOF
        success "WirePlumber A2DP configuration written"

        # Restart WirePlumber so it picks up the new config immediately.
        # Failure is non-fatal — the config will be loaded on next login.
        if systemctl --user is-active --quiet wireplumber 2>/dev/null; then
            info "Restarting WirePlumber to apply Bluetooth codec configuration..."
            if systemctl --user restart wireplumber 2>/dev/null; then
                success "WirePlumber restarted"
            else
                warn "WirePlumber restart failed — codec config will apply on next login"
            fi
        fi
    else
        success "WirePlumber A2DP configuration already present"
    fi
}

# ── Enable user lingering ─────────────────────────────────────────────────────
enable_linger() {
    header "Enabling user session persistence"

    # loginctl linger allows user services to run without being logged in.
    # This is required for headless operation (no X session / no interactive login).
    if loginctl show-user "${INSTALL_USER}" 2>/dev/null | grep -q "Linger=yes"; then
        success "User linger already enabled"
    else
        info "Enabling linger for ${INSTALL_USER}..."
        if sudo loginctl enable-linger "${INSTALL_USER}" 2>/dev/null; then
            success "User linger enabled — services will start at boot without login"
        else
            warn "Could not enable linger (sudo unavailable or loginctl failed)."
            warn "SoundSync will NOT start automatically after reboot on a headless system."
            warn "Fix manually with: sudo loginctl enable-linger ${INSTALL_USER}"
        fi
    fi
}

# ── Check for conflicting Bluetooth audio agents ──────────────────────────────
# Other daemons that register BlueZ MediaEndpoints will steal the A2DP transport
# from WirePlumber, causing the bluez_input node to never appear in PipeWire.
# Known culprits: bluealsa, PulseAudio (when managing BT), custom BT audio servers.
check_bt_audio_conflicts() {
    header "Checking for conflicting Bluetooth audio agents"

    local conflicts=()
    local conflict_services=()   # systemd service names to disable
    local suspicious=""

    # 1. bluealsa — most common competitor; registers its own A2DP MediaEndpoint
    if systemctl is-active --quiet bluealsa 2>/dev/null; then
        conflicts+=("bluealsa (system service is running)")
        conflict_services+=("system:bluealsa")
    fi
    if systemctl --user is-active --quiet bluealsa 2>/dev/null; then
        conflicts+=("bluealsa (user service is running)")
        conflict_services+=("user:bluealsa")
    fi
    if pgrep -x bluealsad &>/dev/null && [ "${#conflict_services[@]}" -eq 0 ]; then
        conflicts+=("bluealsad (process running, not via systemd)")
    fi

    # 2. Any system service whose unit name matches Bluetooth-audio patterns —
    #    catches auto-restarting services like bluetooth-audio.service that will
    #    respawn the process immediately after it is killed.
    local svc_matches
    svc_matches=$(systemctl list-unit-files --type=service --no-legend 2>/dev/null \
        | awk '{print $1}' \
        | grep -iE '(bluetooth.?audio|bluez.?audio|bt.?audio|a2dp)' \
        | grep -v "soundsync\|wireplumber" \
        || true)
    if [ -n "${svc_matches}" ]; then
        while IFS= read -r svc; do
            local state
            state=$(systemctl is-enabled "${svc}" 2>/dev/null || true)
            if [[ "${state}" == "enabled" || "${state}" == "static" ]]; then
                # Only flag if it is actually active or activating (including auto-restart)
                local active
                active=$(systemctl is-active "${svc}" 2>/dev/null || true)
                if [[ "${active}" != "inactive" && "${active}" != "dead" ]]; then
                    conflicts+=("${svc} (system service: ${active}, ${state})")
                    conflict_services+=("system:${svc}")
                elif [[ "${state}" == "enabled" ]]; then
                    # Enabled but not currently active — warn so it doesn't start later
                    conflicts+=("${svc} (system service: enabled but not running — will start on next boot/trigger)")
                    conflict_services+=("system:${svc}")
                fi
            fi
        done <<< "${svc_matches}"
    fi

    # 3. PulseAudio with Bluetooth modules loaded
    if pgrep -x pulseaudio &>/dev/null; then
        if pactl list modules 2>/dev/null | grep -qiE "module-bluetooth-(discover|device|policy)"; then
            conflicts+=("pulseaudio (Bluetooth modules are loaded)")
        fi
    fi

    # 4. Any running process whose args contain Bluetooth-audio patterns —
    #    catches processes not managed by a detected service unit.
    suspicious=$(ps -eo pid,args --no-headers 2>/dev/null \
        | grep -iE '(bluetooth.?audio|bluez.?audio|bt.?audio|a2dp.?sink)' \
        | grep -v "grep\|soundsync\|wireplumber\|install\.sh" \
        || true)
    if [ -n "${suspicious}" ]; then
        while IFS= read -r line; do
            local pid cmd
            pid=$(echo "${line}" | awk '{print $1}')
            cmd=$(echo "${line}" | awk '{print $2}')
            conflicts+=("PID ${pid}: ${cmd}")
        done <<< "${suspicious}"
    fi

    if [ "${#conflicts[@]}" -eq 0 ]; then
        success "No conflicting Bluetooth audio agents detected"
        return
    fi

    warn "Conflicting Bluetooth audio agent(s) detected:"
    for c in "${conflicts[@]}"; do
        warn "  • ${c}"
    done
    echo ""
    warn "These processes register their own BlueZ MediaEndpoint and will steal"
    warn "the A2DP transport from WirePlumber.  Audio will never appear as a"
    warn "PipeWire source and SoundSync will not receive audio."
    echo ""

    if [ -t 0 ]; then
        echo -en "  ${BLD}Disable and stop conflicting services/processes now?${RST} [Y/n]: "
        read -r answer
        answer="${answer:-Y}"
        if [[ "${answer}" =~ ^[Yy]$ ]]; then
            # Disable & stop conflicting systemd services so they cannot respawn
            for entry in "${conflict_services[@]}"; do
                local scope svc
                scope="${entry%%:*}"
                svc="${entry##*:}"
                if [ "${scope}" = "system" ]; then
                    sudo systemctl disable --now "${svc}" 2>/dev/null \
                        && success "Disabled and stopped system service: ${svc}" \
                        || warn "Could not disable ${svc} — try: sudo systemctl disable --now ${svc}"
                else
                    systemctl --user disable --now "${svc}" 2>/dev/null \
                        && success "Disabled and stopped user service: ${svc}" \
                        || warn "Could not disable ${svc} — try: systemctl --user disable --now ${svc}"
                fi
            done

            # Kill any remaining conflicting processes by PID
            if [ -n "${suspicious}" ]; then
                while IFS= read -r line; do
                    local pid
                    pid=$(echo "${line}" | awk '{print $1}')
                    if kill "${pid}" 2>/dev/null; then
                        success "Stopped PID ${pid}"
                    else
                        warn "Could not stop PID ${pid} — try: sudo kill ${pid}"
                    fi
                done <<< "${suspicious}"
            fi
        else
            warn "Skipped — SoundSync may not receive audio until these are stopped."
        fi
    else
        warn "Non-interactive mode — skipping automatic stop."
        warn "Disable conflicting services manually before connecting a Bluetooth source:"
        for entry in "${conflict_services[@]}"; do
            local scope svc
            scope="${entry%%:*}"
            svc="${entry##*:}"
            if [ "${scope}" = "system" ]; then
                warn "  sudo systemctl disable --now ${svc}"
            else
                warn "  systemctl --user disable --now ${svc}"
            fi
        done
        [ -n "${suspicious}" ] && warn "  kill <PID>   # for any remaining processes"
    fi
}

# ── Remove snd-aloop if previously loaded ─────────────────────────────────────
# snd-aloop creates an ALSA loopback device that WirePlumber treats as a
# real hardware sink and may prefer over the soundsync-capture null sink.
# This causes audio to be routed away from soundsync-capture.monitor, breaking
# both the browser audio stream and the spectrum visualiser.
# SoundSync uses a PipeWire null sink (soundsync-capture) instead, which is
# sufficient for headless operation.
configure_snd_aloop() {
    local modules_conf="/etc/modules-load.d/soundsync.conf"

    # Remove the persistence file if we wrote it previously
    if [ -f "${modules_conf}" ] && grep -q "snd-aloop" "${modules_conf}" 2>/dev/null; then
        info "Removing snd-aloop persistence (${modules_conf})..."
        sudo rm -f "${modules_conf}" 2>/dev/null || true
        success "snd-aloop persistence removed"
    fi

    # Remove from /etc/modules if present (another common persistence location)
    if grep -qE "^snd[-_]aloop" /etc/modules 2>/dev/null; then
        info "Removing snd-aloop from /etc/modules..."
        sudo sed -i '/^snd[-_]aloop/d' /etc/modules 2>/dev/null || warn "Could not edit /etc/modules"
        success "snd-aloop removed from /etc/modules"
    fi

    # Unload the module if it is currently loaded — it competes with soundsync-capture
    if lsmod 2>/dev/null | grep -q "^snd_aloop"; then
        info "Unloading snd-aloop kernel module (conflicts with soundsync-capture routing)..."
        sudo modprobe -r snd-aloop 2>/dev/null || warn "Could not unload snd-aloop — reboot to clear it"
    fi
}

# ── Find or build binary ───────────────────────────────────────────────────────
build_binary() {
    # Check for a pre-built binary in the repo root first
    if [ -f "${REPO_DIR}/soundsync" ]; then
        BINARY_PATH="${REPO_DIR}/soundsync"
        success "Pre-built binary found at ${BINARY_PATH} — skipping build"
        return 0
    fi

    header "Building SoundSync"

    # Ensure cargo is in PATH
    source "${HOME}/.cargo/env" 2>/dev/null || true
    export PATH="${HOME}/.cargo/bin:${PATH}"

    if ! command -v cargo &>/dev/null; then
        error "cargo not found after installing Rust. Please run: source ~/.cargo/env"
        exit 1
    fi

    # Fix stdbool.h / bindgen clang header issue
    if command -v clang &>/dev/null; then
        local clang_res
        clang_res=$(clang -print-resource-dir 2>/dev/null || true)
        if [ -d "${clang_res}/include" ]; then
            export BINDGEN_EXTRA_CLANG_ARGS="-I${clang_res}/include"
        fi
    fi

    cd "${REPO_DIR}"

    if [ "${BUILD_RELEASE}" = true ]; then
        info "Building release binary (optimised)..."
        cargo build --release 2>&1 | tail -5
        BINARY_PATH="${REPO_DIR}/target/release/soundsync"
    else
        info "Building debug binary..."
        cargo build 2>&1 | tail -5
        BINARY_PATH="${REPO_DIR}/target/debug/soundsync"
    fi

    if [ ! -f "${BINARY_PATH}" ]; then
        error "Build failed — binary not found at ${BINARY_PATH}"
        exit 1
    fi

    success "Build successful: ${BINARY_PATH}"
}

# ── Install binary ────────────────────────────────────────────────────────────
install_binary() {
    header "Installing binary"

    mkdir -p "${INSTALL_BIN}"

    cp "${BINARY_PATH}" "${INSTALL_BIN}/soundsync"
    chmod +x "${INSTALL_BIN}/soundsync"

    # Ensure ~/.local/bin is in PATH
    if ! echo "${PATH}" | grep -q "${HOME}/.local/bin"; then
        warn "${HOME}/.local/bin is not in your PATH"
        warn "Add this to ~/.bashrc or ~/.zshrc:"
        warn "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi

    success "Binary installed: ${INSTALL_BIN}/soundsync"
    info "Version: $(${INSTALL_BIN}/soundsync --version 2>/dev/null || echo 'v0.1.0')"
}

# ── Detect FFmpeg AAC encoder ─────────────────────────────────────────────────
# Sets AAC_ENCODER to "libfdk_aac" if that encoder is compiled into the
# installed ffmpeg, otherwise falls back to the built-in "aac" encoder.
# libfdk_aac is higher quality but only available in non-free ffmpeg builds.
detect_aac_encoder() {
    header "Detecting FFmpeg AAC encoder"

    AAC_ENCODER="aac"   # safe default

    if ! command -v ffmpeg &>/dev/null; then
        warn "ffmpeg not found — AAC streaming will use fallback encoder when installed"
        return
    fi

    if ffmpeg -codecs 2>/dev/null | grep -q "libfdk_aac"; then
        AAC_ENCODER="libfdk_aac"
        success "libfdk_aac encoder detected — high-quality AAC enabled"
    else
        AAC_ENCODER="aac"
        info "Using FFmpeg built-in AAC encoder"
        info "For higher quality AAC, install an ffmpeg build with libfdk_aac:"
        info "  e.g. from the 'jellyfin-ffmpeg' or 'deb-multimedia' repository"
    fi
}

# ── Write configuration ───────────────────────────────────────────────────────
write_config() {
    header "Writing configuration"

    mkdir -p "${CONFIG_DIR}"

    # Only write config if it doesn't exist (preserve user changes on reinstall)
    if [ ! -f "${CONFIG_DIR}/config.toml" ]; then
        cat > "${CONFIG_DIR}/config.toml" << TOMLEOF
# SoundSync Configuration
# Generated by installer on $(date '+%Y-%m-%d %H:%M:%S')

# Web UI port
port = ${PORT}

# Bluetooth adapter name
adapter = "${ADAPTER}"

# Bluetooth device name (as seen by other devices)
device_name = "${DEVICE_NAME}"

# Auto-accept incoming pairing requests
auto_pair = true

# Maximum number of simultaneously connected devices
max_devices = 1

# AAC encoder: libfdk_aac (best quality, requires non-free ffmpeg build)
#              or aac (FFmpeg built-in, always available)
# Detected automatically at install time — change if you switch ffmpeg builds.
aac_encoder = "${AAC_ENCODER}"

# Default browser stream quality: mp3 | aac | wav
# mp3 — MP3 128 kbps, universal browser support
# aac — AAC 192 kbps, higher quality, Safari & Chrome (recommended for LAN)
# wav — Lossless PCM ~1.4 Mbps, highest quality, LAN only
stream_quality = "mp3"
TOMLEOF
        success "Configuration written: ${CONFIG_DIR}/config.toml"
    else
        # Existing config: add new keys if they are absent (non-destructive upgrade).
        if ! grep -q "^aac_encoder" "${CONFIG_DIR}/config.toml"; then
            info "Adding aac_encoder to existing config..."
            echo "" >> "${CONFIG_DIR}/config.toml"
            echo "# AAC encoder detected by installer (${AAC_ENCODER})" >> "${CONFIG_DIR}/config.toml"
            echo "aac_encoder = \"${AAC_ENCODER}\"" >> "${CONFIG_DIR}/config.toml"
        fi
        if ! grep -q "^stream_quality" "${CONFIG_DIR}/config.toml"; then
            info "Adding stream_quality to existing config..."
            echo "stream_quality = \"mp3\"" >> "${CONFIG_DIR}/config.toml"
        fi
        success "Configuration already exists (updated with new keys): ${CONFIG_DIR}/config.toml"
    fi

    # Create log directory
    mkdir -p "${LOG_DIR}"
}

# ── Install systemd service ───────────────────────────────────────────────────
install_service() {
    header "Installing systemd user service"

    mkdir -p "${SERVICE_DIR}"

    # Stop existing service before replacing
    if systemctl --user is-active --quiet soundsync 2>/dev/null; then
        info "Stopping existing SoundSync service..."
        systemctl --user stop soundsync
    fi

    local uid
    uid=$(id -u)

    cat > "${SERVICE_FILE}" << SVCEOF
[Unit]
Description=SoundSync — Bluetooth A2DP Sink with DSP EQ
Documentation=https://github.com/tunlezah/BluetoothA2DP
After=network.target bluetooth.target pipewire.service wireplumber.service pipewire-pulse.service
Wants=pipewire.service wireplumber.service pipewire-pulse.service
Requires=dbus.socket
# Allow up to 10 restarts within 5 minutes before systemd gives up.
StartLimitBurst=10
StartLimitIntervalSec=300

[Service]
Type=simple
ExecStart=${INSTALL_BIN}/soundsync --port ${PORT} --name "${DEVICE_NAME}" --adapter ${ADAPTER}
Restart=on-failure
# 10 s between restarts gives the BT adapter time to re-enumerate after a freeze.
RestartSec=10s

# Environment
Environment=LOG_FORMAT=json
Environment=RUST_LOG=soundsync=info,tower_http=warn
# Ensure pactl/parec can find the PipeWire-PulseAudio socket even in lingering sessions.
Environment=XDG_RUNTIME_DIR=/run/user/${uid}
Environment=PULSE_RUNTIME_PATH=/run/user/${uid}/pulse

# Resource limits
LimitNOFILE=65536

# Security hardening
NoNewPrivileges=true
PrivateTmp=true

# Standard I/O
StandardOutput=journal
StandardError=journal
SyslogIdentifier=soundsync

[Install]
WantedBy=default.target
SVCEOF

    # Reload daemon and enable/start service
    systemctl --user daemon-reload
    systemctl --user enable soundsync

    info "Starting SoundSync service..."
    if systemctl --user start soundsync; then
        sleep 2
        if systemctl --user is-active --quiet soundsync; then
            success "SoundSync service started successfully"
        else
            warn "Service may have failed to start — checking logs..."
            journalctl --user -u soundsync --no-pager -n 20 || true
        fi
    else
        warn "Service start failed — check: journalctl --user -u soundsync -f"
    fi
}

# ── Firewall ──────────────────────────────────────────────────────────────────
configure_firewall() {
    header "Configuring firewall"

    if command -v ufw &>/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
        info "Allowing port ${PORT} through UFW..."
        sudo ufw allow "${PORT}/tcp" comment "SoundSync Web UI" || true
        success "Firewall rule added for port ${PORT}"
    else
        info "UFW not active — skipping firewall configuration"
    fi
}

# ── Verification ──────────────────────────────────────────────────────────────
verify_install() {
    header "Verifying installation"

    local failed=false

    # Check binary exists
    if [ -x "${INSTALL_BIN}/soundsync" ]; then
        success "Binary: ${INSTALL_BIN}/soundsync"
    else
        error "Binary not found or not executable"
        failed=true
    fi

    # Check config
    if [ -f "${CONFIG_DIR}/config.toml" ]; then
        success "Config: ${CONFIG_DIR}/config.toml"
    else
        error "Config not found"
        failed=true
    fi

    # Check service
    if [ -f "${SERVICE_FILE}" ]; then
        success "Service: ${SERVICE_FILE}"
    else
        error "Service file not found"
        failed=true
    fi

    # Check service status
    if systemctl --user is-active --quiet soundsync 2>/dev/null; then
        success "Service: running ✓"
    else
        warn "Service: not running (check: journalctl --user -u soundsync -f)"
    fi

    # Check Bluetooth adapter
    if command -v hciconfig &>/dev/null && hciconfig "${ADAPTER}" &>/dev/null; then
        success "Bluetooth adapter: ${ADAPTER} available"
    elif hciconfig 2>/dev/null | grep -q "hci"; then
        success "Bluetooth adapter: found"
    else
        warn "Bluetooth adapter: not detected (device may not be connected yet)"
    fi

    if [ "${failed}" = true ]; then
        error "Some verification checks failed — review the errors above"
        return 1
    fi
}

# ── Print summary ─────────────────────────────────────────────────────────────
print_summary() {
    local local_ip
    local_ip=$(hostname -I 2>/dev/null | awk '{print $1}' || echo "localhost")

    echo ""
    echo -e "${BLD}${GRN}╔════════════════════════════════════════════════╗${RST}"
    echo -e "${BLD}${GRN}║       SoundSync installed successfully!        ║${RST}"
    echo -e "${BLD}${GRN}╚════════════════════════════════════════════════╝${RST}"
    echo ""
    echo -e "  Web UI:     ${CYN}http://localhost:${PORT}${RST}"
    echo -e "  Network:    ${CYN}http://${local_ip}:${PORT}${RST}"
    echo -e "  BT Name:    ${BLD}${DEVICE_NAME}${RST}"
    echo -e "  Adapter:    ${ADAPTER}"
    echo ""
    echo -e "  Service management:"
    echo -e "    ${YLW}systemctl --user status soundsync${RST}    — check status"
    echo -e "    ${YLW}systemctl --user restart soundsync${RST}   — restart"
    echo -e "    ${YLW}journalctl --user -u soundsync -f${RST}    — live logs"
    echo ""
    echo -e "  Config: ${CONFIG_DIR}/config.toml"
    echo ""
    echo -e "  To uninstall: ${RED}./install.sh --uninstall${RST}"
    echo ""

    if ! groups "${INSTALL_USER}" | grep -q bluetooth; then
        echo -e "${YLW}  ⚠  Please log out and back in for Bluetooth group access${RST}"
    fi
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
    echo ""
    echo -e "${BLD}${CYN}SoundSync Installer v${APP_VERSION}${RST}"
    echo -e "Installing for user: ${INSTALL_USER}"
    echo ""

    # Handle uninstall
    if [ "${UNINSTALL}" = true ]; then
        do_uninstall
    fi

    # Pre-flight checks
    check_not_root
    check_os
    validate_port
    check_bt_audio_conflicts

    # Install steps
    install_system_packages
    install_rust
    configure_bluez
    configure_pipewire
    configure_snd_aloop
    enable_linger
    build_binary
    install_binary
    detect_aac_encoder
    write_config
    install_service
    configure_firewall
    verify_install
    print_summary
}

main "$@"
