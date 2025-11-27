#!/bin/bash
# Voice Keyboard Setup Script for macOS
# Run with: curl -sSL https://raw.githubusercontent.com/alexmak/voice-keyboard/main/scripts/setup-macos.sh | bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}"
echo "╔══════════════════════════════════════════╗"
echo "║     Voice Keyboard Setup for macOS       ║"
echo "╚══════════════════════════════════════════╝"
echo -e "${NC}"

# Check macOS
if [[ "$(uname)" != "Darwin" ]]; then
    echo -e "${RED}Error: This script is for macOS only${NC}"
    exit 1
fi

# Check architecture
ARCH=$(uname -m)
echo -e "${BLUE}→ Detected architecture: ${ARCH}${NC}"

# Model selection
MODEL_NAME="ggml-large-v3-turbo.bin"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_NAME}"
MODELS_DIR="$HOME/.local/share/voice-keyboard/models"
INSTALL_DIR="$HOME/voice-keyboard"

# Step 1: Check/Install Homebrew
echo -e "\n${BLUE}[1/6] Checking Homebrew...${NC}"
if ! command -v brew &> /dev/null; then
    echo -e "${YELLOW}Homebrew not found. Installing...${NC}"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

    # Add to path for Apple Silicon
    if [[ "$ARCH" == "arm64" ]]; then
        echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
else
    echo -e "${GREEN}✓ Homebrew is installed${NC}"
fi

# Step 2: Install dependencies
echo -e "\n${BLUE}[2/6] Installing dependencies...${NC}"
brew install cmake rust 2>/dev/null || true
echo -e "${GREEN}✓ Dependencies installed${NC}"

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
if [[ "$ARCH" == "arm64" ]]; then
    echo "Building with Metal acceleration for Apple Silicon..."
    cargo build --release --features "whisper,metal"
else
    echo "Building for Intel Mac..."
    cargo build --release --features "whisper"
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

# Step 6: Create symlink
echo -e "\n${BLUE}[6/6] Setting up...${NC}"
mkdir -p "$HOME/.local/bin"
ln -sf "$INSTALL_DIR/target/release/voice-typer" "$HOME/.local/bin/voice-typer"

# Add to PATH if needed
if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
    echo -e "${YELLOW}Added ~/.local/bin to PATH in ~/.zshrc${NC}"
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
echo -e "${YELLOW}Important: Grant permissions when prompted:${NC}"
echo "  1. Microphone - for recording speech"
echo "  2. Accessibility - for typing text"
echo "  3. Input Monitoring - for hotkey detection"
echo ""
echo "Open System Settings → Privacy & Security to grant permissions"
echo ""
echo -e "${BLUE}Usage:${NC}"
echo "  1. Press and hold Fn key"
echo "  2. Speak"
echo "  3. Release key - text appears in focused app"
echo ""
