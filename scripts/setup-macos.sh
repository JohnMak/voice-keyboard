#!/bin/bash
# Voice Keyboard Setup Script for macOS
# Run with: curl -sSL https://raw.githubusercontent.com/alexmakeev/voice-keyboard/main/scripts/setup-macos.sh | bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m' # No Color

# Directories
MODELS_DIR="$HOME/.local/share/voice-keyboard/models"
CONFIG_DIR="$HOME/.config/voice-keyboard"
CONFIG_FILE="$CONFIG_DIR/config.toml"
INSTALL_DIR="$HOME/voice-keyboard"

# Model definitions: name|size_mb|description|min_ram_gb|latency
MODELS=(
    "tiny|75|Fastest, lowest quality. Good for testing|4|~0.5s"
    "base|142|Fast, acceptable quality|4|~1s"
    "small|466|Good balance of speed and quality|6|~2s"
    "medium|1500|High quality, slower|8|~4s"
    "large-v3-turbo|1600|Best quality/speed ratio. Recommended|8|~2s"
)

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
echo -e "${BLUE}→ Architecture: ${ARCH}${NC}"

# Detect system specs
detect_system() {
    # Get total RAM in GB
    RAM_BYTES=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
    RAM_GB=$((RAM_BYTES / 1024 / 1024 / 1024))

    # Get CPU info
    CPU_BRAND=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo "Unknown")
    CPU_CORES=$(sysctl -n hw.ncpu 2>/dev/null || echo 4)

    # Check for Apple Silicon
    if [[ "$ARCH" == "arm64" ]]; then
        HAS_NEURAL_ENGINE=true
        CHIP_TYPE="Apple Silicon"
    else
        HAS_NEURAL_ENGINE=false
        CHIP_TYPE="Intel"
    fi

    echo -e "${BLUE}→ Chip: ${CHIP_TYPE}${NC}"
    echo -e "${BLUE}→ CPU: ${CPU_BRAND}${NC}"
    echo -e "${BLUE}→ RAM: ${RAM_GB} GB${NC}"
    echo -e "${BLUE}→ Cores: ${CPU_CORES}${NC}"
}

# Get recommended model based on system specs
get_recommended_model() {
    if [[ $RAM_GB -ge 16 ]]; then
        echo "large-v3-turbo"
    elif [[ $RAM_GB -ge 8 ]]; then
        if [[ "$ARCH" == "arm64" ]]; then
            echo "large-v3-turbo"  # Apple Silicon is efficient
        else
            echo "small"
        fi
    elif [[ $RAM_GB -ge 6 ]]; then
        echo "small"
    else
        echo "base"
    fi
}

# Interactive model selection
select_model() {
    local recommended=$(get_recommended_model)
    local selected=0
    local num_models=${#MODELS[@]}

    # Find recommended model index
    for i in "${!MODELS[@]}"; do
        IFS='|' read -r name _ <<< "${MODELS[$i]}"
        if [[ "$name" == "$recommended" ]]; then
            selected=$i
            break
        fi
    done

    echo -e "\n${BOLD}Select Whisper Model:${NC}"
    echo -e "${DIM}Use ↑/↓ arrows to navigate, Enter to select${NC}\n"

    # Check if we're in interactive mode
    if [[ -t 0 ]]; then
        # Interactive mode with arrow keys
        while true; do
            # Clear and redraw menu
            for i in "${!MODELS[@]}"; do
                IFS='|' read -r name size_mb desc min_ram latency <<< "${MODELS[$i]}"
                local size_str
                if [[ $size_mb -ge 1000 ]]; then
                    size_str="$(echo "scale=1; $size_mb/1000" | bc) GB"
                else
                    size_str="${size_mb} MB"
                fi

                local rec_marker=""
                if [[ "$name" == "$recommended" ]]; then
                    rec_marker="${GREEN} ★ RECOMMENDED${NC}"
                fi

                if [[ $i -eq $selected ]]; then
                    echo -e "  ${CYAN}▶${NC} ${BOLD}${name}${NC} ${DIM}(${size_str})${NC}${rec_marker}"
                    echo -e "      ${desc}"
                    echo -e "      ${DIM}Min RAM: ${min_ram} GB | Latency: ${latency}${NC}"
                else
                    echo -e "    ${name} ${DIM}(${size_str})${NC}${rec_marker}"
                fi
            done

            # Read single keypress
            read -rsn1 key

            # Handle arrow keys (escape sequences)
            if [[ $key == $'\x1b' ]]; then
                read -rsn2 key
                case $key in
                    '[A') # Up arrow
                        ((selected > 0)) && ((selected--))
                        ;;
                    '[B') # Down arrow
                        ((selected < num_models - 1)) && ((selected++))
                        ;;
                esac
            elif [[ $key == "" ]]; then
                # Enter pressed
                break
            fi

            # Move cursor up to redraw
            for ((i = 0; i < num_models * 3; i++)); do
                echo -en "\033[A\033[K"
            done
        done
    else
        # Non-interactive mode (piped input) - use recommended
        echo -e "${YELLOW}Non-interactive mode detected, using recommended model: ${recommended}${NC}"
        for i in "${!MODELS[@]}"; do
            IFS='|' read -r name _ <<< "${MODELS[$i]}"
            if [[ "$name" == "$recommended" ]]; then
                selected=$i
                break
            fi
        done
    fi

    IFS='|' read -r SELECTED_MODEL SELECTED_SIZE _ _ _ <<< "${MODELS[$selected]}"
    echo -e "\n${GREEN}✓ Selected: ${SELECTED_MODEL}${NC}"
}

# Detect system
detect_system

# Step 1: Check/Install Homebrew
echo -e "\n${BLUE}[1/7] Checking Homebrew...${NC}"
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
echo -e "\n${BLUE}[2/7] Installing dependencies...${NC}"
brew install cmake rust 2>/dev/null || true
echo -e "${GREEN}✓ Dependencies installed${NC}"

# Step 3: Clone repository
echo -e "\n${BLUE}[3/7] Cloning repository...${NC}"
if [ -d "$INSTALL_DIR" ]; then
    echo -e "${YELLOW}Directory exists, updating...${NC}"
    cd "$INSTALL_DIR"
    git pull
else
    git clone https://github.com/alexmakeev/voice-keyboard.git "$INSTALL_DIR"
    cd "$INSTALL_DIR"
fi
echo -e "${GREEN}✓ Repository ready${NC}"

# Step 4: Build
echo -e "\n${BLUE}[4/7] Building (this may take a few minutes)...${NC}"
if [[ "$ARCH" == "arm64" ]]; then
    echo "Building with Metal acceleration for Apple Silicon..."
    cargo build --release --features "whisper,metal"
else
    echo "Building for Intel Mac..."
    cargo build --release --features "whisper"
fi
echo -e "${GREEN}✓ Build complete${NC}"

# Step 5: Select model
echo -e "\n${BLUE}[5/7] Model selection...${NC}"
select_model

# Step 6: Download model
echo -e "\n${BLUE}[6/7] Downloading Whisper model...${NC}"
mkdir -p "$MODELS_DIR"

MODEL_FILE="ggml-${SELECTED_MODEL}.bin"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_FILE}"

if [ -f "$MODELS_DIR/$MODEL_FILE" ]; then
    echo -e "${YELLOW}Model already exists, skipping download${NC}"
else
    if [[ $SELECTED_SIZE -ge 1000 ]]; then
        SIZE_STR="$(echo "scale=1; $SELECTED_SIZE/1000" | bc) GB"
    else
        SIZE_STR="${SELECTED_SIZE} MB"
    fi
    echo "Downloading ${MODEL_FILE} (${SIZE_STR})..."
    echo "This may take a few minutes depending on your connection..."
    curl -L --progress-bar -o "$MODELS_DIR/$MODEL_FILE" "$MODEL_URL"
fi
echo -e "${GREEN}✓ Model ready${NC}"

# Step 7: Create config and symlinks
echo -e "\n${BLUE}[7/7] Setting up configuration...${NC}"

# Create config directory
mkdir -p "$CONFIG_DIR"

# Write config file
cat > "$CONFIG_FILE" << EOF
# Voice Keyboard Configuration
# Generated by setup script

[whisper]
# Model to use for speech recognition
# Options: tiny, base, small, medium, large-v3-turbo
model = "${SELECTED_MODEL}"

[audio]
# Push-to-talk hotkey
# Options: fn, ctrl, ctrlright, alt, altright, shift, cmd
hotkey = "fn"

[input]
# Text input method
# Options: keyboard, clipboard
method = "keyboard"
EOF

echo -e "${GREEN}✓ Config saved to ${CONFIG_FILE}${NC}"

# Create symlink
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

echo -e "${BOLD}Configuration:${NC}"
echo "  Model: ${SELECTED_MODEL}"
echo "  Config: ${CONFIG_FILE}"
echo ""
echo -e "${BLUE}To run Voice Keyboard:${NC}"
echo "  voice-typer"
echo ""
echo -e "${BLUE}Or with a different model:${NC}"
echo "  voice-typer --model tiny"
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
echo -e "${DIM}To change model later, edit: ${CONFIG_FILE}${NC}"
echo ""
