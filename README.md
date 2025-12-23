# Voice Keyboard

Push-to-talk voice keyboard with speech recognition. Supports both **cloud-based GPT-4o** (recommended for best quality) and **local Whisper** models.

## How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                 │
│   1. HOLD the hotkey    2. SPEAK         3. RELEASE the key    │
│                                                                 │
│      ┌─────┐               🎤                  ┌─────┐          │
│      │ Fn  │  ──────►  "Hello world"  ──────►  │ Fn  │          │
│      └─────┘                                   └─────┘          │
│       Press                                    Release          │
│                                                   │             │
│                                                   ▼             │
│                                           ┌─────────────┐       │
│                                           │ Hello world │       │
│                                           └─────────────┘       │
│                                           Text appears in       │
│                                           your active app       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Voice Keyboard** uses a push-to-talk interface:
1. **Press and hold** the hotkey to start recording
2. **Speak** — your voice is recorded
3. **Release** the key — speech is transcribed and text is typed into the active application

## Transcription Modes

| Mode | Quality | Speed | Privacy | Requirements |
|------|---------|-------|---------|--------------|
| **GPT-4o** (recommended) | Excellent | Fast | Cloud | OpenAI API key |
| **Whisper Local** | Very Good | Varies | Local | ~3GB RAM |

## Quick Start

### Prerequisites

**macOS:**
```bash
brew install cmake rust
```

**Linux (Ubuntu/Debian):**
```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake curl git \
  libasound2-dev libxdo-dev libx11-dev libxi-dev libxtst-dev pkg-config

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### Installation

```bash
# Clone repository
git clone https://github.com/alexmakeev/voice-keyboard.git
cd voice-keyboard

# Build with OpenAI support (recommended)
cargo build --release --features opus

# Or build with local Whisper support
cargo build --release --features whisper

# Or build with both
cargo build --release --features "opus,whisper"

# macOS with Metal acceleration
cargo build --release --features "whisper,metal"
```

### One-Line Install (macOS)

```bash
curl -sSL https://raw.githubusercontent.com/alexmakeev/voice-keyboard/main/scripts/setup-macos.sh | bash
```

### One-Line Install (Linux)

```bash
curl -sSL https://raw.githubusercontent.com/alexmakeev/voice-keyboard/main/scripts/setup-linux.sh | bash
```

---

## Mode 1: GPT-4o Transcription (Recommended)

Best quality transcription using OpenAI's GPT-4o model. Requires an API key.

### Setup

1. Get an API key from [OpenAI Platform](https://platform.openai.com/api-keys)

2. Set the environment variable:
```bash
export OPENAI_API_KEY="sk-your-api-key-here"
```

Or add to your shell profile (`~/.bashrc`, `~/.zshrc`):
```bash
echo 'export OPENAI_API_KEY="sk-your-api-key-here"' >> ~/.zshrc
source ~/.zshrc
```

3. Run with OpenAI mode:
```bash
./target/release/voice-typer --openai
```

### GPT-4o Advantages

- **Superior accuracy** for Russian, English, and mixed language
- **Better punctuation** and formatting
- **Context understanding** for technical terms
- **No local model download** required

---

## Mode 2: Local Whisper Transcription

All processing happens locally on your device — no data is sent to the cloud.

### Download a Model

**Automatic download (recommended):**
```bash
./target/release/voice-typer --download large-v3-turbo
```

**Manual download:**
```bash
# Create models directory
mkdir -p ~/.local/share/voice-keyboard/models
cd ~/.local/share/voice-keyboard/models

# Download large-v3-turbo (recommended, 1.6GB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# Or download large-v3 (best quality, 3.1GB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin

# Or download tiny for testing (75MB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
```

### Run with Local Whisper

```bash
# With default model
./target/release/voice-typer

# With specific model
./target/release/voice-typer --model large-v3-turbo
./target/release/voice-typer --model large-v3
./target/release/voice-typer --model tiny
```

### Available Models

| Model | Size | RAM | Speed | Quality | Download |
|-------|------|-----|-------|---------|----------|
| tiny | 75 MB | ~400 MB | ~32x | Basic | `--download tiny` |
| base | 142 MB | ~500 MB | ~16x | Good | `--download base` |
| small | 466 MB | ~1 GB | ~6x | Very Good | `--download small` |
| medium | 1.5 GB | ~3 GB | ~2x | Excellent | `--download medium` |
| **large-v3-turbo** | **1.6 GB** | **~3 GB** | **~8x** | **Best** | `--download large-v3-turbo` |
| large-v3 | 3.1 GB | ~6 GB | ~1x | Best | `--download large-v3` |

> **Recommendation**: Use **large-v3-turbo** for the best balance of quality and speed.

---

## Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `OPENAI_API_KEY` | OpenAI API key for GPT-4o mode | `sk-...` |
| `MODEL_PATH` | Path to Whisper model file | `~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin` |
| `VOICE_KEYBOARD_DEV` | Enable dev mode (save reports) | `1` |
| `WHISPER_ENHANCE` | Audio enhancement settings | `all`, `none`, or `normalize,dc,pre_emphasis` |
| `RUST_LOG` | Logging level | `debug`, `trace` |

### WHISPER_ENHANCE Options

Controls audio preprocessing for local Whisper:
- `all` (default) — enable all enhancements
- `none` — disable all enhancements
- `normalize` — peak normalization
- `noise_reduction` or `denoise` — noise gate
- `dc` or `dc_offset` — DC offset removal
- `pre_emphasis` or `preemph` — high-frequency boost

Example:
```bash
WHISPER_ENHANCE=normalize,dc ./target/release/voice-typer
```

---

## Command Line Options

```
Usage: voice-typer [OPTIONS]

Transcription Mode:
  --openai             Use OpenAI GPT-4o API (requires OPENAI_API_KEY)
  --model <MODEL>      Use local Whisper model (tiny, base, small, medium, large-v3-turbo, large-v3)

Model Management:
  --download <MODEL>   Download a model from the internet
  --list-models        List available models and download URLs

Input/Output:
  --key <KEY>          Push-to-talk hotkey (fn, ctrl, ctrlright, alt, shift, cmd)
  --keyboard           Use keyboard simulation (default)
  --clipboard          Use clipboard + paste instead of keyboard

Audio:
  --volume <0.0-1.0>   Beep sounds volume (default: 0.1 = 10%)
  --silent, -q         Disable all beep sounds

Other:
  --list-keys          List available hotkeys
  --version, -V        Show version
  --help, -h           Show help
```

### Examples

```bash
# GPT-4o mode (best quality)
OPENAI_API_KEY="sk-..." ./target/release/voice-typer --openai

# Local Whisper with turbo model
./target/release/voice-typer --model large-v3-turbo

# Silent mode (no beeps)
./target/release/voice-typer --openai --silent

# Custom hotkey (Right Ctrl)
./target/release/voice-typer --openai --key ctrlright

# Clipboard mode (paste instead of type)
./target/release/voice-typer --openai --clipboard

# Download and use a model
./target/release/voice-typer --download large-v3-turbo
./target/release/voice-typer --model large-v3-turbo

# Debug logging
RUST_LOG=debug ./target/release/voice-typer --openai
```

---

## Hotkeys

### Default Hotkey by Platform

| Platform | Default Hotkey | Notes |
|----------|---------------|-------|
| **macOS** | `Fn` (Globe key) | Works on MacBook keyboards |
| **Linux** | `Right Ctrl` | Fn key is hardware-only on most keyboards |
| **Windows** | `Right Ctrl` | Fn key is hardware-only on most keyboards |

### Available Hotkeys

| Key | Flag | Description |
|-----|------|-------------|
| `fn` | `--key fn` | Fn/Globe key (macOS only) |
| `ctrl` | `--key ctrl` | Left Control |
| `ctrlright` | `--key ctrlright` | Right Control |
| `alt` | `--key alt` | Left Alt/Option |
| `altright` | `--key altright` | Right Alt/Option |
| `shift` | `--key shift` | Left Shift |
| `cmd` | `--key cmd` | Command/Super/Win |

---

## Platform-Specific Notes

### macOS

#### Permissions

Grant these permissions in **System Settings → Privacy & Security**:

| Permission | Why | How |
|------------|-----|-----|
| **Microphone** | Record speech | Prompted automatically |
| **Accessibility** | Type text | Add terminal/app to Accessibility |
| **Input Monitoring** | Detect hotkey | Add terminal/app to Input Monitoring |

#### Hardware Acceleration

```bash
# Metal (recommended for M1/M2/M3/M4)
cargo build --release --features "whisper,metal"

# CoreML (alternative)
cargo build --release --features "whisper,coreml"
```

### Linux

#### Dependencies

**Ubuntu/Debian:**
```bash
sudo apt-get install -y build-essential cmake curl git \
  libasound2-dev libxdo-dev libx11-dev libxi-dev libxtst-dev pkg-config
```

**Fedora:**
```bash
sudo dnf install -y cmake gcc-c++ alsa-lib-devel libxdo-devel \
  libX11-devel libXi-devel libXtst-devel
```

**Arch Linux:**
```bash
sudo pacman -S cmake alsa-lib xdotool libx11 libxi libxtst
```

#### Running

```bash
# May need to run with sudo for input events
./target/release/voice-typer

# Or add user to input group
sudo usermod -aG input $USER
# Log out and back in
```

### Windows

#### Requirements
- Windows 10/11
- Visual Studio Build Tools 2019+
- CMake 3.20+

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++".

---

## Configuration File

Location:
- macOS/Linux: `~/.config/voice-keyboard/config.json`
- Windows: `%APPDATA%\voice-keyboard\config.json`

Example:
```json
{
  "model_path": "~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin",
  "language": "ru",
  "hotkey": {
    "trigger_key": "Function",
    "push_to_talk": true
  },
  "injection_method": "keyboard",
  "streaming": false
}
```

### Config Options

| Option | Values | Description |
|--------|--------|-------------|
| `language` | `"auto"`, `"en"`, `"ru"`, etc. | Recognition language |
| `injection_method` | `"keyboard"`, `"clipboard"` | How to input text |
| `trigger_key` | `"Function"`, `"ControlRight"`, etc. | Push-to-talk key |
| `streaming` | `true`, `false` | Enable fragmentary recognition |

---

## Troubleshooting

### Common Issues

**"Model not found" error:**
```bash
# Check if model exists
ls ~/.local/share/voice-keyboard/models/

# Download model
./target/release/voice-typer --download large-v3-turbo
```

**Text goes to wrong app (macOS):**
- Grant Accessibility permission to Terminal/iTerm
- Restart the application

**Hotkey not working:**
- macOS: Grant Input Monitoring permission
- Linux: Run with sudo or add user to input group
- Windows: Run as Administrator

**Transcription is slow:**
- Use GPT-4o mode (`--openai`) for fastest results
- Use Metal acceleration on macOS: `--features metal`
- Use smaller model: `--model tiny`

**Wrong language detected:**
- Set language in config: `"language": "ru"`
- GPT-4o mode auto-detects language better

### Debug Mode

```bash
# Enable debug logging
RUST_LOG=debug ./target/release/voice-typer --openai

# Trace all events
RUST_LOG=trace ./target/release/voice-typer
```

---

## Development

### Building from Source

```bash
# Clone
git clone https://github.com/alexmakeev/voice-keyboard.git
cd voice-keyboard

# Build with OpenAI support only
cargo build --release --features opus

# Build with Whisper only
cargo build --release --features whisper

# Build with both
cargo build --release --features "opus,whisper"

# Run tests
cargo test
```

### Project Structure

```
voice-keyboard/
├── src/
│   ├── bin/
│   │   ├── voice_typer.rs       # Main voice-to-text binary
│   │   ├── whisper_enhance.rs   # Audio preprocessing for Whisper
│   │   └── voice_recorder.rs    # Audio recorder
│   ├── lib.rs
│   ├── audio.rs                 # Audio recording (cpal)
│   ├── transcribe.rs            # Whisper integration
│   ├── hotkey.rs                # Global hotkey listener
│   ├── inject.rs                # Text injection
│   └── config.rs                # Configuration
├── scripts/
│   ├── setup-macos.sh           # macOS installer
│   ├── setup-linux.sh           # Linux installer
│   └── setup-windows.ps1        # Windows installer
└── Cargo.toml
```

---

## License

MIT License - see [LICENSE](LICENSE)

## Credits

- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) - Whisper C++ implementation
- [whisper-rs](https://github.com/tazz4843/whisper-rs) - Rust bindings
- [OpenAI Whisper](https://github.com/openai/whisper) - Original model
- [OpenAI GPT-4o](https://platform.openai.com/) - Cloud transcription
- [cpal](https://github.com/RustAudio/cpal) - Cross-platform audio
- [rdev](https://github.com/Narsil/rdev) - Global hotkey detection

## Support

- Issues: [GitHub Issues](https://github.com/alexmakeev/voice-keyboard/issues)
- Discussions: [GitHub Discussions](https://github.com/alexmakeev/voice-keyboard/discussions)
