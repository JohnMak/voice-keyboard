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

| Model | Size | RAM | Speed | Quality |
|-------|------|-----|-------|---------|
| tiny | 75 MB | ~400 MB | ~30x | Basic |
| base | 142 MB | ~500 MB | ~20x | Good |
| small | 466 MB | ~1 GB | ~10x | Better |
| **large-v3-turbo** | 1.6 GB | ~3 GB | ~10x | **Best** |

Download from [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp).

## License

MIT

## Credits

- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) - Whisper C++ implementation
- [whisper-rs](https://github.com/tazz4843/whisper-rs) - Rust bindings
- [OpenAI Whisper](https://github.com/openai/whisper) - Original model
