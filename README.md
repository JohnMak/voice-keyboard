# Voice Keyboard

Push-to-talk voice keyboard with local Whisper speech recognition.

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
2. **Speak** — your voice is recorded locally
3. **Release** the key — speech is transcribed using Whisper AI and text is typed into the active application

All processing happens **locally on your device** — no data is sent to the cloud.

### Hotkey by Platform

| Platform | Default Hotkey | Notes |
|----------|---------------|-------|
| **macOS** | `Fn` (Globe key) | Works on MacBook keyboards. The Fn key sends a special event that macOS can detect. |
| **macOS** | `F13` | Alternative for external keyboards without Fn key. |
| **Linux** | `F13` or `Right Ctrl` | The Fn key on most keyboards is hardware-only and invisible to the OS. Use F13 (if available) or configure another key. |
| **Windows** | `F13` or `Right Ctrl` | Same as Linux — Fn is usually not detectable. |

> **Important:** On most non-Apple keyboards, the **Fn key is a hardware modifier** that the operating system cannot see. It modifies other keys (like F1-F12) before they reach the OS. For Linux and Windows, use a different key like F13, Right Ctrl, or configure your preferred key in settings.

### Smart Features

- **Voice Activity Detection (VAD)**: Automatically detects pauses in your speech (~350ms) and transcribes incrementally. You can speak in sentences with natural pauses.
- **Context Continuation**: If you pause mid-sentence, the next phrase continues smoothly without breaking the sentence.
- **Hallucination Filter**: Filters out common Whisper artifacts (like subtitle credits that appear in training data).

## Features

- **Push-to-talk**: Hold a hotkey to record, release to transcribe
- **Local processing**: All speech recognition happens on your device (no cloud)
- **VAD (Voice Activity Detection)**: Automatically splits speech by pauses for incremental transcription
- **Context-aware**: Maintains sentence context across pauses
- **Universal input**: Works in any application (browsers, editors, chat apps)
- **Multi-platform**: macOS (full support), Linux, Windows (planned)

## Quick Start

### macOS (Recommended)

```bash
# One-line install
curl -sSL https://raw.githubusercontent.com/alexmak/voice-keyboard/main/scripts/setup-macos.sh | bash
```

Or step-by-step:

```bash
# 1. Install dependencies
brew install cmake rust

# 2. Clone and build
git clone https://github.com/alexmak/voice-keyboard.git
cd voice-keyboard
cargo build --release --features "whisper,metal"

# 3. Download model
mkdir -p ~/.local/share/voice-keyboard/models
curl -L -o ~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# 4. Run
./target/release/voice-typer
```

### Linux

```bash
# One-line install
curl -sSL https://raw.githubusercontent.com/alexmak/voice-keyboard/main/scripts/setup-linux.sh | bash
```

Or step-by-step:

```bash
# 1. Install dependencies (Ubuntu/Debian)
sudo apt-get update
sudo apt-get install -y build-essential cmake curl git \
  libasound2-dev libxdo-dev libx11-dev libxi-dev libxtst-dev

# 2. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 3. Clone and build
git clone https://github.com/alexmak/voice-keyboard.git
cd voice-keyboard
cargo build --release --features whisper

# 4. Download model
mkdir -p ~/.local/share/voice-keyboard/models
curl -L -o ~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# 5. Run
./target/release/voice-typer
```

### Windows

```powershell
# Run in PowerShell as Administrator
Set-ExecutionPolicy Bypass -Scope Process -Force
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/alexmak/voice-keyboard/main/scripts/setup-windows.ps1" -OutFile "setup-windows.ps1"
.\setup-windows.ps1
```

Or step-by-step:

1. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++"
2. Install [CMake](https://cmake.org/download/)
3. Install [Rust](https://rustup.rs/)
4. Open PowerShell:

```powershell
# Clone and build
git clone https://github.com/alexmak/voice-keyboard.git
cd voice-keyboard
cargo build --release --features whisper

# Download model
New-Item -ItemType Directory -Force -Path "$env:LOCALAPPDATA\voice-keyboard\models"
Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin" `
  -OutFile "$env:LOCALAPPDATA\voice-keyboard\models\ggml-large-v3-turbo.bin"

# Run
.\target\release\voice-typer.exe
```

## Usage

### Voice Typer (Recommended)

```bash
# Start voice typer
./target/release/voice-typer

# With specific model
./target/release/voice-typer --model large-v3-turbo

# Keyboard input (types character by character)
./target/release/voice-typer --keyboard

# Clipboard input (pastes via Cmd+V / Ctrl+V)
./target/release/voice-typer --clipboard
```

**Basic Usage:**
1. Run the voice-typer
2. Click on any text field (browser, editor, chat, etc.)
3. **Press and hold** the hotkey (Fn on Mac, see table above)
4. Speak clearly
5. **Release** the key — your speech appears as text!

**Pro tips:**
- You can pause naturally while speaking — VAD will transcribe each phrase separately
- Long pauses (~350ms) trigger incremental transcription while you're still holding the key
- Works best with the large-v3-turbo model for accuracy

### Voice Recorder

Records audio and saves as file (no transcription):

```bash
./target/release/voice-recorder
```

### Main Application

Full application with tray icon:

```bash
./target/release/voice-keyboard
./target/release/voice-keyboard --config    # Show config paths
./target/release/voice-keyboard --transcribe file.wav  # Transcribe file
```

## Configuration

Config file location:
- macOS/Linux: `~/.config/voice-keyboard/config.json`
- Windows: `%APPDATA%\voice-keyboard\config.json`

```json
{
  "model_path": "~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin",
  "language": "ru",
  "hotkey": {
    "trigger_key": "Function",
    "push_to_talk": true,
    "modifiers": []
  },
  "injection_method": "keyboard"
}
```

### Options

| Option | Values | Description |
|--------|--------|-------------|
| `language` | `"auto"`, `"en"`, `"ru"`, etc. | Recognition language ([full list](https://github.com/openai/whisper#available-models-and-languages)) |
| `injection_method` | `"keyboard"`, `"clipboard"` | How to input text |
| `trigger_key` | See table below | Push-to-talk key |
| `modifiers` | `["ControlLeft"]`, `["Alt", "Shift"]` | Optional key modifiers |

### Available Hotkeys

| Key Name | Description | Platform Notes |
|----------|-------------|----------------|
| `Function` | Fn/Globe key | **macOS only** — hardware Fn on other platforms is not detectable |
| `F13` | F13 key | Available on extended keyboards, some keyboards have it above F12 |
| `ControlRight` | Right Ctrl | Good choice for Linux/Windows |
| `AltRight` | Right Alt | Alternative option |
| `CapsLock` | Caps Lock | Can be remapped |
| `Space` | Space (with modifiers) | Use with modifiers like `["ControlLeft"]` |

**Recommended by platform:**
- **macOS (MacBook)**: `Function` (built-in Fn key)
- **macOS (external keyboard)**: `F13` or `ControlRight`
- **Linux**: `ControlRight` or `F13`
- **Windows**: `ControlRight` or `F13`

## Models

### Model Comparison

| Model | Size | RAM | Speed | Quality | Best For |
|-------|------|-----|-------|---------|----------|
| tiny | 75 MB | ~400 MB | ~32x | Basic | Testing |
| base | 142 MB | ~500 MB | ~16x | Good | Low-end devices |
| small | 466 MB | ~1 GB | ~6x | Very Good | Balanced |
| medium | 1.5 GB | ~3 GB | ~2x | Excellent | High accuracy |
| **large-v3-turbo** | **1.6 GB** | **~3 GB** | **~8x** | **Best** | **Recommended** |

> **Recommendation**: Use **large-v3-turbo** for the best quality with good speed.

### Download Models

```bash
# Models directory
mkdir -p ~/.local/share/voice-keyboard/models
cd ~/.local/share/voice-keyboard/models

# Recommended
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# For testing (small)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
```

All models: [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp)

## Platform-Specific Notes

### macOS

#### Requirements
- macOS 12.0+ (Apple Silicon recommended)
- ~3GB RAM for large-v3-turbo model
- Homebrew (for dependencies)

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

Metal provides ~2-3x speedup over CPU.

#### Troubleshooting

**Text goes to Dock instead of app:**
- Grant Accessibility permission to Terminal/iTerm
- Restart the application

**"Model not found" error:**
```bash
# Check model path
ls ~/.local/share/voice-keyboard/models/
```

### Linux

#### Requirements
- Ubuntu 20.04+ / Debian 11+ / Fedora 35+
- ALSA development libraries
- X11 (Wayland support planned)

#### Dependencies

Ubuntu/Debian:
```bash
sudo apt-get install -y build-essential cmake curl git \
  libasound2-dev libxdo-dev libx11-dev libxi-dev libxtst-dev
```

Fedora:
```bash
sudo dnf install -y cmake gcc-c++ alsa-lib-devel libxdo-devel \
  libX11-devel libXi-devel libXtst-devel
```

Arch Linux:
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
- ~3GB RAM

#### Build Tools

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with:
- "Desktop development with C++"
- Windows 10/11 SDK

#### Known Issues

- Windows support is experimental
- Some antivirus may flag keyboard simulation
- Run as Administrator if hotkey doesn't work

## Development

### Building from Source

```bash
# Clone
git clone https://github.com/alexmak/voice-keyboard.git
cd voice-keyboard

# Build without Whisper (for testing compilation)
cargo build

# Build with Whisper
cargo build --features whisper

# Build release with Metal (macOS)
cargo build --release --features "whisper,metal"

# Run tests
cargo test
cargo test --test voice_typer_test
cargo test --test vad_test
```

### Project Structure

```
voice-keyboard/
├── src/
│   ├── bin/
│   │   ├── voice_typer.rs    # Main voice-to-text binary
│   │   ├── voice_recorder.rs # Audio recorder
│   │   └── minimal.rs        # Minimal test binary
│   ├── lib.rs                # Library exports
│   ├── audio.rs              # Audio recording (cpal)
│   ├── transcribe.rs         # Whisper integration
│   ├── hotkey.rs             # Global hotkey listener
│   ├── inject.rs             # Text injection
│   └── config.rs             # Configuration
├── tests/
│   ├── voice_typer_test.rs   # Voice typer tests
│   └── vad_test.rs           # VAD tests
├── scripts/
│   ├── setup-macos.sh        # macOS installer
│   ├── setup-linux.sh        # Linux installer
│   └── setup-windows.ps1     # Windows installer
└── Cargo.toml
```

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Voice Typer                            │
├─────────────────────────────────────────────────────────────┤
│  HotkeyListener (rdev)                                      │
│       ↓ key_down / key_up                                   │
│  AudioRecorder (cpal) [48kHz]                               │
│       ↓ audio samples                                       │
│  VAD (Voice Activity Detection)                             │
│       ↓ phrase segments                                     │
│  Resampler [48kHz → 16kHz]                                  │
│       ↓ resampled audio                                     │
│  Transcriber (whisper-rs)                                   │
│       ↓ text                                                │
│  Context Processor (continuation handling)                  │
│       ↓ processed text                                      │
│  TextInjector (CGEvent/xdotool/SendInput)                   │
└─────────────────────────────────────────────────────────────┘
```

### VAD Parameters

```rust
VAD_ENERGY_THRESHOLD: 0.001  // Speech detection threshold
VAD_SILENCE_MS: 350          // Silence duration to end phrase
VAD_MIN_SPEECH_MS: 500       // Minimum phrase duration
VAD_WINDOW_MS: 30            // Analysis window size
VAD_SKIP_INITIAL_MS: 200     // Skip initial audio (button noise)
```

## Troubleshooting

### Common Issues

**No audio recorded:**
- Check microphone permissions
- Verify microphone is connected: `arecord -l` (Linux)
- Check System Preferences → Sound → Input (macOS)

**Transcription is slow:**
- Use Metal acceleration on macOS: `--features metal`
- Try smaller model: `--model tiny`
- Ensure sufficient RAM

**Wrong language detected:**
- Force language: set `"language": "ru"` in config
- Use `--model` with language-specific model

**Hotkey not working:**
- macOS: Grant Input Monitoring permission
- Linux: Run with sudo or add to input group
- Windows: Run as Administrator

**"Hallucination" text (random phrases):**
- Built-in filter removes known hallucinations
- If new ones appear, report on GitHub

### Debug Mode

```bash
# Enable debug logging
RUST_LOG=debug ./target/release/voice-typer

# Trace all events
RUST_LOG=trace ./target/release/voice-typer
```

## Contributing

1. Fork the repository
2. Create feature branch: `git checkout -b feature/my-feature`
3. Commit changes: `git commit -am 'Add feature'`
4. Push: `git push origin feature/my-feature`
5. Create Pull Request

## License

MIT License - see [LICENSE](LICENSE)

## Credits

- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) - Whisper C++ implementation
- [whisper-rs](https://github.com/tazz4843/whisper-rs) - Rust bindings
- [OpenAI Whisper](https://github.com/openai/whisper) - Original model
- [cpal](https://github.com/RustAudio/cpal) - Cross-platform audio
- [rdev](https://github.com/Narsil/rdev) - Global hotkey detection

## Support

- Issues: [GitHub Issues](https://github.com/alexmak/voice-keyboard/issues)
- Discussions: [GitHub Discussions](https://github.com/alexmak/voice-keyboard/discussions)
