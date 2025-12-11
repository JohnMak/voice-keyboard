#!/bin/bash
# Sync voice-keyboard transcription logs and audio to remote server for analysis
# Usage: ./sync-logs.sh

REMOTE_HOST="alexmak@robobobr.ru"
REMOTE_DIR="~/materials/voice-keyboard-logs"

# Determine local data directory
if [[ "$OSTYPE" == "darwin"* ]]; then
    LOCAL_DIR="$HOME/.local/share/voice-keyboard"
elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "win32" ]]; then
    LOCAL_DIR="$APPDATA/voice-keyboard"
else
    LOCAL_DIR="$HOME/.local/share/voice-keyboard"
fi

LOG_FILE="$LOCAL_DIR/transcriptions.log"
AUDIO_DIR="$LOCAL_DIR/audio"

if [ ! -f "$LOG_FILE" ]; then
    echo "No transcription log found at: $LOG_FILE"
    exit 1
fi

echo "Creating remote directory..."
ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR/audio"

echo "Syncing transcription log..."
scp "$LOG_FILE" "$REMOTE_HOST:$REMOTE_DIR/"

if [ -d "$AUDIO_DIR" ]; then
    echo "Syncing audio files..."
    # Use rsync for incremental sync if available
    if command -v rsync &> /dev/null; then
        rsync -avz --progress "$AUDIO_DIR/" "$REMOTE_HOST:$REMOTE_DIR/audio/"
    else
        scp -r "$AUDIO_DIR/"* "$REMOTE_HOST:$REMOTE_DIR/audio/"
    fi
fi

echo "Done! Logs synced to $REMOTE_HOST:$REMOTE_DIR"
echo ""
echo "Log format: timestamp | audio_file | raw_whisper | processed_text | [cont]"
echo "Audio files: $REMOTE_DIR/audio/*.wav"
