Voice Typer
===========

Voice to text input using local Whisper AI.
No internet connection required for transcription.

USAGE
-----
1. Open Command Prompt or PowerShell
2. Run: voice-typer --help
3. Hold Right Ctrl key to record, release to transcribe

The text will be automatically typed into the active window.

QUICK START
-----------
  voice-typer                    # Start with default settings
  voice-typer --model tiny       # Use smaller/faster model
  voice-typer --key alt          # Use Alt key instead of Ctrl
  voice-typer --clipboard        # Use clipboard instead of keyboard

MODELS
------
Download Whisper models from:
https://huggingface.co/ggerganov/whisper.cpp/tree/main

Place them in:
%APPDATA%\voice-keyboard\models\

Recommended: ggml-large-v3-turbo.bin (best quality)
Alternative: ggml-tiny.bin (fastest, lower quality)

SUPPORT
-------
GitHub: https://github.com/alexmak/voice-keyboard
Issues: https://github.com/alexmak/voice-keyboard/issues

LICENSE
-------
MIT License - see LICENSE file for details.
