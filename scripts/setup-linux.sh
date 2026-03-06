#!/bin/bash
# Voice Keyboard Setup Script for Linux
# Run with: curl -sSL https://raw.githubusercontent.com/alexmak/voice-keyboard/main/scripts/setup-linux.sh | bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}"
echo "╔══════════════════════════════════════════╗"
echo "║     Voice Keyboard Setup for Linux       ║"
echo "╚══════════════════════════════════════════╝"
echo -e "${NC}"

# Check Linux
if [[ "$(uname)" != "Linux" ]]; then
    echo -e "${RED}Error: This script is for Linux only${NC}"
    exit 1
fi

# Model selection
MODEL_NAME="ggml-large-v3-turbo.bin"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_NAME}"
MODELS_DIR="$HOME/.local/share/voice-keyboard/models"
INSTALL_DIR="$HOME/voice-keyboard"

# Detect package manager
detect_package_manager() {
    if command -v apt-get &> /dev/null; then
        echo "apt"
    elif command -v dnf &> /dev/null; then
        echo "dnf"
    elif command -v pacman &> /dev/null; then
        echo "pacman"
    elif command -v zypper &> /dev/null; then
        echo "zypper"
    else
        echo "unknown"
    fi
}

PKG_MANAGER=$(detect_package_manager)
echo -e "${BLUE}→ Detected package manager: ${PKG_MANAGER}${NC}"

# Step 1: Install system dependencies
echo -e "\n${BLUE}[1/6] Installing system dependencies...${NC}"

case $PKG_MANAGER in
    apt)
        sudo apt-get update
        sudo apt-get install -y build-essential cmake curl git \
            libasound2-dev libxdo-dev libx11-dev libxi-dev libxtst-dev \
            pkg-config libssl-dev
        ;;
    dnf)
        sudo dnf install -y cmake gcc-c++ curl git \
            alsa-lib-devel libxdo-devel libX11-devel libXi-devel libXtst-devel \
            openssl-devel
        ;;
    pacman)
        sudo pacman -Sy --noconfirm cmake base-devel curl git \
            alsa-lib xdotool libx11 libxi libxtst openssl
        ;;
    zypper)
        sudo zypper install -y cmake gcc-c++ curl git \
            alsa-devel xdotool libX11-devel libXi-devel libXtst-devel \
            libopenssl-devel
        ;;
    *)
        echo -e "${RED}Error: Unsupported package manager${NC}"
        echo "Please install these dependencies manually:"
        echo "  - cmake, gcc/g++, curl, git"
        echo "  - alsa development libraries"
        echo "  - xdotool, X11 development libraries"
        exit 1
        ;;
esac
echo -e "${GREEN}✓ System dependencies installed${NC}"

# Step 2: Install Rust
echo -e "\n${BLUE}[2/6] Checking Rust...${NC}"
if ! command -v cargo &> /dev/null; then
    echo -e "${YELLOW}Rust not found. Installing...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo -e "${GREEN}✓ Rust is installed${NC}"
    # Ensure latest stable
    rustup update stable 2>/dev/null || true
fi

# Source cargo env
source "$HOME/.cargo/env" 2>/dev/null || true

# Step 3: Clone repository
echo -e "\n${BLUE}[3/6] Cloning repository...${NC}"
if [ -d "$INSTALL_DIR" ]; then
    echo -e "${YELLOW}Directory exists, updating...${NC}"
    cd "$INSTALL_DIR"
    git pull
else
    git clone https://github.com/alexmak/voice-keyboard.git "$INSTALL_DIR"
    cd "$INSTALL_DIR"
fi
echo -e "${GREEN}✓ Repository ready${NC}"

# Step 4: Build
echo -e "\n${BLUE}[4/6] Building (this may take a few minutes)...${NC}"

# Check for CUDA (optional GPU acceleration)
if command -v nvcc &> /dev/null; then
    echo "CUDA detected, building with GPU support..."
    cargo build --release --features "whisper,opus"
    # Note: CUDA support in whisper-rs may require additional setup
else
    echo "Building CPU-only version..."
    cargo build --release --features "whisper,opus"
fi
echo -e "${GREEN}✓ Build complete${NC}"

# Step 5: Download model
echo -e "\n${BLUE}[5/6] Downloading Whisper model...${NC}"
mkdir -p "$MODELS_DIR"

if [ -f "$MODELS_DIR/$MODEL_NAME" ]; then
    echo -e "${YELLOW}Model already exists, skipping download${NC}"
else
    echo "Downloading ${MODEL_NAME} (1.6 GB)..."
    echo "This may take a few minutes depending on your connection..."
    curl -L --progress-bar -o "$MODELS_DIR/$MODEL_NAME" "$MODEL_URL"
fi
echo -e "${GREEN}✓ Model ready${NC}"

# Step 6: Setup
echo -e "\n${BLUE}[6/6] Setting up...${NC}"

# Create symlink
mkdir -p "$HOME/.local/bin"
ln -sf "$INSTALL_DIR/target/release/voice-typer" "$HOME/.local/bin/voice-typer"

# Add to PATH if needed
if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    # Detect shell config file
    SHELL_RC=""
    if [ -f "$HOME/.bashrc" ]; then
        SHELL_RC="$HOME/.bashrc"
    elif [ -f "$HOME/.zshrc" ]; then
        SHELL_RC="$HOME/.zshrc"
    fi

    if [ -n "$SHELL_RC" ]; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$SHELL_RC"
        echo -e "${YELLOW}Added ~/.local/bin to PATH in $SHELL_RC${NC}"
    fi
fi

# Add user to input group for hotkey access
if ! groups "$USER" | grep -q '\binput\b'; then
    echo -e "${YELLOW}Adding user to 'input' group for hotkey access...${NC}"
    sudo usermod -aG input "$USER"
    echo -e "${YELLOW}Note: You need to log out and back in for group changes to take effect${NC}"
fi

echo -e "${GREEN}✓ Setup complete${NC}"

# Final instructions
echo -e "\n${GREEN}"
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║                    Installation Complete!                     ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

echo -e "${BLUE}To run Voice Keyboard:${NC}"
echo "  cd $INSTALL_DIR"
echo "  ./target/release/voice-typer"
echo ""
echo -e "${BLUE}Or after restarting terminal:${NC}"
echo "  voice-typer"
echo ""
echo -e "${YELLOW}Important notes:${NC}"
echo "  1. You may need to log out and back in for input group access"
echo "  2. Make sure your microphone is working (test with 'arecord -l')"
echo "  3. X11 is required (Wayland support is limited)"
echo ""
echo -e "${BLUE}Usage:${NC}"
echo "  1. Press and hold Fn key (or configured hotkey)"
echo "  2. Speak"
echo "  3. Release key - text appears in focused app"
echo ""
echo -e "${YELLOW}If hotkey doesn't work, try running with sudo:${NC}"
echo "  sudo ./target/release/voice-typer"
echo ""
