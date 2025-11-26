# Voice Keyboard

Push-to-talk voice keyboard for macOS with local Whisper speech recognition.

## Features

- **Push-to-talk**: Hold a hotkey to record, release to transcribe
- **Local processing**: All speech recognition happens on your device
- **Fast transcription**: Optimized for Apple Silicon with CoreML/Metal support
- **Universal input**: Works in any application (browsers, editors, chat apps)

## Requirements

- macOS 12.0+ (Apple Silicon recommended)
- ~3GB RAM for large-v3-turbo model
- Permissions: Microphone, Accessibility, Input Monitoring

## Installation

### From Release (recommended)

1. Download the latest `.dmg` from [Releases](https://github.com/alexmakeev/voice-keyboard/releases)
2. Drag to Applications
3. Launch and grant permissions when prompted

### From Source

```bash
# Clone
git clone https://github.com/alexmakeev/voice-keyboard.git
cd voice-keyboard

# Build
cargo build --release

# Download model
mkdir -p ~/.local/share/voice-keyboard/models
curl -L -o ~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# Run
./target/release/voice-keyboard
```

## Usage

1. Launch Voice Keyboard (runs in menu bar)
2. Press and hold **F13** (or configured hotkey)
3. Speak
4. Release key — text appears in active field

### CLI Options

```bash
voice-keyboard                        # Run normally
voice-keyboard --config               # Show config paths
voice-keyboard --transcribe file.wav  # Transcribe a file (for testing)
```

## Configuration

Config file: `~/.config/voice-keyboard/config.json`

```json
{
  "model_path": "~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin",
  "language": "auto",
  "hotkey": {
    "trigger_key": "F13",
    "push_to_talk": true,
    "modifiers": []
  },
  "injection_method": "clipboard"
}
```

### Hotkey Options

- `F13` - dedicated key (recommended)
- `Space` with modifiers `["cmd", "shift"]` - Cmd+Shift+Space

### Languages

- `"auto"` - automatic detection
- `"en"` - English
- `"ru"` - Russian
- [Full list](https://github.com/openai/whisper#available-models-and-languages)

## Development

### Setup (Linux/macOS)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Linux dependencies
sudo apt-get install libasound2-dev libxdo-dev

# Build
cargo build

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run
```

### Testing with Audio Files

```bash
# Download tiny model for fast testing
mkdir -p models
curl -L -o models/ggml-tiny.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin

# Transcribe a file
MODEL_PATH=./models/ggml-tiny.bin cargo run -- --transcribe test.wav

# Run integration tests
MODEL_PATH=./models/ggml-tiny.bin cargo test --test transcription_test -- --ignored
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Voice Keyboard                          │
├─────────────────────────────────────────────────────────────┤
│  HotkeyListener (rdev)                                      │
│       ↓ key_down / key_up                                   │
│  AudioRecorder (cpal)                                       │
│       ↓ audio samples (16kHz mono)                          │
│  Transcriber (whisper-rs)                                   │
│       ↓ text                                                │
│  TextInjector (clipboard + Cmd+V)                           │
└─────────────────────────────────────────────────────────────┘
```

## Permissions (macOS)

Voice Keyboard requires these permissions:

| Permission | Why | How to Grant |
|------------|-----|--------------|
| **Microphone** | Record speech | Prompted automatically |
| **Accessibility** | Simulate keyboard input | System Settings → Privacy → Accessibility |
| **Input Monitoring** | Global hotkey detection | System Settings → Privacy → Input Monitoring |

## Models

### Comparison

| Model | Size | RAM | Speed | Quality | Best For |
|-------|------|-----|-------|---------|----------|
| tiny | 75 MB | ~400 MB | ~32x | Basic | Testing, low-end devices |
| base | 142 MB | ~500 MB | ~16x | Good | Quick transcription |
| small | 466 MB | ~1 GB | ~6x | Very Good | Balanced quality/speed |
| medium | 1.5 GB | ~3 GB | ~2x | Excellent | High accuracy |
| **large-v3-turbo** | **1.6 GB** | **~3 GB** | **~8x** | **Best** | **Recommended** |

> **Recommendation**: Use **large-v3-turbo** for the best quality with good speed. It's optimized to run nearly as fast as medium while maintaining large-v3 quality. Excellent for Russian and English.

### Installation

Models are stored in `~/.local/share/voice-keyboard/models/`

```bash
# Create models directory
mkdir -p ~/.local/share/voice-keyboard/models
cd ~/.local/share/voice-keyboard/models
```

#### Recommended: large-v3-turbo (Best quality, fast)

```bash
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

#### Alternative models

```bash
# tiny - For testing (75 MB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin

# base - Good balance for older hardware (142 MB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin

# small - Very good quality (466 MB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin

# medium - Excellent quality, slower (1.5 GB)
curl -L -O https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin
```

### Hardware Acceleration (macOS)

For Apple Silicon Macs (M1/M2/M3/M4), enable GPU acceleration:

```bash
# Metal acceleration (recommended for Apple Silicon)
cargo build --release --features "whisper,metal"

# CoreML acceleration (alternative)
cargo build --release --features "whisper,coreml"
```

Metal provides ~2-3x speedup over CPU-only inference.

### Specifying Model Path

Set the model path via environment variable or config:

```bash
# Via environment variable
MODEL_PATH=~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin ./voice-typer

# Or in config.json
{
  "model_path": "~/.local/share/voice-keyboard/models/ggml-large-v3-turbo.bin"
}
```

All models available at [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp).

## License

MIT

## Credits

- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) - Whisper C++ implementation
- [whisper-rs](https://github.com/tazz4843/whisper-rs) - Rust bindings
- [OpenAI Whisper](https://github.com/openai/whisper) - Original model
