#!/usr/bin/env bash
set -euo pipefail

REPO="nixuuu/ralph-wiggum-rs"
BINARY_NAME="ralph-wiggum"
INSTALL_DIR="$HOME/.local/bin"

# --- Helpers ---

info()    { echo "  -> $*"; }
success() { echo "  ✓  $*"; }
error()   { echo "  ✗  $*" >&2; }

# --- Dependency check ---

if ! command -v curl &>/dev/null; then
    error "curl is required but not installed."
    exit 1
fi

# --- Platform detection ---

detect_target() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)
            error "Unsupported OS: $os"
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64)          arch="x86_64" ;;
        aarch64|arm64)   arch="aarch64" ;;
        *)
            error "Unsupported architecture: $arch"
            exit 1
            ;;
    esac

    target="${arch}-${os}"
    echo "$target"
}

# --- Download latest release binary ---

download_binary() {
    local target="$1"
    local asset_name="${BINARY_NAME}-${target}"
    local tmp_file
    tmp_file="$(mktemp)"

    info "Fetching latest release info from GitHub..."

    local release_json
    release_json="$(curl --fail-with-body -sL "https://api.github.com/repos/${REPO}/releases/latest")"

    local download_url
    download_url="$(echo "$release_json" \
        | sed -n 's/.*"browser_download_url": *"\([^"]*\)".*/\1/p' \
        | grep "$asset_name" \
        | head -1)"

    if [ -z "$download_url" ]; then
        error "Could not find asset '${asset_name}' in the latest release."
        return 1
    fi

    info "Downloading ${asset_name} from:"
    info "  ${download_url}"

    curl --fail-with-body -sL -o "$tmp_file" "$download_url"

    mkdir -p "$INSTALL_DIR"
    mv "$tmp_file" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    success "Binary installed to ${INSTALL_DIR}/${BINARY_NAME}"
    return 0
}

# --- Fallback: cargo install from source ---

build_from_source() {
    info "Attempting to build from source as fallback..."

    if ! command -v cargo &>/dev/null; then
        error "cargo is not installed. Install Rust from https://rustup.rs/ and try again."
        exit 1
    fi

    if ! command -v git &>/dev/null; then
        error "git is required to build from source."
        exit 1
    fi

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    trap "rm -rf '$tmp_dir'" EXIT

    info "Cloning repository..."
    git clone --depth 1 "https://github.com/${REPO}.git" "$tmp_dir"

    info "Building with cargo (release mode)..."
    cargo build --release --manifest-path "${tmp_dir}/Cargo.toml"

    mkdir -p "$INSTALL_DIR"
    cp "${tmp_dir}/target/release/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    success "Binary built and installed to ${INSTALL_DIR}/${BINARY_NAME}"
}

# --- PATH configuration ---

ensure_path() {
    # Check if INSTALL_DIR is already in PATH
    if echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
        success "${INSTALL_DIR} is already in PATH"
        return
    fi

    local shell_name
    shell_name="$(basename "${SHELL:-bash}")"
    local rc_file=""
    local path_line=""

    case "$shell_name" in
        zsh)
            rc_file="$HOME/.zshrc"
            path_line="export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
        bash)
            # Prefer .bashrc, fall back to .bash_profile on macOS
            if [ -f "$HOME/.bashrc" ]; then
                rc_file="$HOME/.bashrc"
            else
                rc_file="$HOME/.bash_profile"
            fi
            path_line="export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
        fish)
            rc_file="$HOME/.config/fish/config.fish"
            path_line="fish_add_path \$HOME/.local/bin"
            ;;
        *)
            info "Unknown shell '${shell_name}'. Please add ${INSTALL_DIR} to your PATH manually."
            return
            ;;
    esac

    # Check if the line already exists in the rc file
    if [ -f "$rc_file" ] && grep -qF '.local/bin' "$rc_file"; then
        success "${INSTALL_DIR} already configured in ${rc_file}"
        return
    fi

    info "Adding ${INSTALL_DIR} to PATH in ${rc_file}..."

    # Ensure the rc file's parent directory exists (for fish)
    mkdir -p "$(dirname "$rc_file")"

    echo "" >> "$rc_file"
    echo "# Added by ralph-wiggum installer" >> "$rc_file"
    echo "$path_line" >> "$rc_file"

    success "PATH updated in ${rc_file}"
    info "Run 'source ${rc_file}' or open a new terminal to apply."
}

# --- Verification ---

verify_install() {
    # Temporarily add install dir to PATH for verification
    export PATH="${INSTALL_DIR}:${PATH}"

    if command -v "$BINARY_NAME" &>/dev/null; then
        local version
        version="$("$BINARY_NAME" --version 2>&1 || true)"
        success "Installed: ${version}"
    else
        error "Installation could not be verified."
        exit 1
    fi
}

# --- Main ---

main() {
    echo ""
    echo "  ralph-wiggum installer"
    echo "  ======================"
    echo ""

    local target
    target="$(detect_target)"
    info "Detected platform: ${target}"

    if ! download_binary "$target"; then
        info "Binary download failed, falling back to source build..."
        build_from_source
    fi

    ensure_path
    verify_install

    echo ""
    success "ralph-wiggum is ready to use!"
    echo ""
}

main
