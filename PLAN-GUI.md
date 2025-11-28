# Voice Keyboard GUI - План реализации

## Текущий статус

### CLI (voice-typer) - ГОТОВО
- Кросс-платформенная компиляция (macOS, Linux, Windows)
- Push-to-talk запись с VAD (Voice Activity Detection)
- Whisper транскрипция с фильтрацией галлюцинаций
- Автовыбор хоткея по платформе (Fn на macOS, Right Ctrl на Linux/Windows)
- Keyboard simulation и clipboard режимы

### GUI (Tauri) - В ПРОЦЕССЕ

**Готово:**
- UI интерфейс (HTML/CSS/JS):
  - Вкладки Log и Settings
  - Лог транскрипций
  - Настройки: модель, язык, хоткей, метод ввода
  - Модальное окно debug-отчёта
- System tray с меню
- Tauri команды: config, transcriptions, models, debug report
- Структура проекта src-tauri/

**НЕ готово:**
- [ ] Интеграция аудио записи (audio.rs - пустой)
- [ ] Интеграция Whisper (whisper.rs - пустой)
- [ ] Real-time события статуса (recording/processing/idle)
- [ ] Скачивание моделей с прогрессом
- [ ] Фоновый процесс записи при нажатии хоткея

---

## План интеграции

### Этап 1: Перенос кода из voice_typer.rs

1. **audio.rs** - модуль записи аудио:
   - `start_recording()` - начать запись
   - `stop_recording()` - остановить и вернуть samples
   - `play_beep()` - звуковой сигнал
   - Использовать cpal (уже в зависимостях)

2. **whisper.rs** - модуль транскрипции:
   - `load_model()` - загрузка модели
   - `transcribe()` - транскрипция аудио
   - `is_hallucination()` - фильтрация
   - Константы: PROGRAMMER_PROMPT, HALLUCINATION_PATTERNS

3. **vad.rs** - Voice Activity Detection:
   - `VadPhraseDetector` структура
   - `detect_phrase()` - детекция фраз
   - `get_remaining()` - остаток после остановки

### Этап 2: Tauri интеграция

1. **Глобальные хоткеи:**
   - Использовать rdev для перехвата
   - Отдельный thread для listener
   - События в UI через Tauri emit

2. **Статус события:**
   - `status-update`: idle → recording → processing → idle
   - `transcription`: новый текст
   - `error`: ошибки

3. **Команды:**
   - `start_listening()` - запустить listener хоткеев
   - `stop_listening()` - остановить
   - `get_status()` - текущий статус

### Этап 3: Скачивание моделей

1. **UI:**
   - Кнопка "Download" рядом с моделью
   - Progress bar при скачивании
   - Статус: Not downloaded / Downloading X% / Downloaded

2. **Backend:**
   - `download_model(model_id)` - скачать модель
   - Huggingface URL: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin`
   - Сохранять в `~/.local/share/voice-keyboard/models/` (Linux) или аналог

3. **События:**
   - `download-progress`: { model, percent, bytes_downloaded, total_bytes }
   - `download-complete`: { model }
   - `download-error`: { model, error }

### Этап 4: Ввод текста

1. **Keyboard simulation:**
   - enigo для набора текста
   - На macOS: CGEvent API для Unicode

2. **Clipboard mode:**
   - arboard для clipboard
   - Cmd/Ctrl+V для вставки

---

## Файловая структура после интеграции

```
src-tauri/
├── src/
│   ├── main.rs          # Tauri setup, commands
│   ├── audio.rs         # Запись аудио (cpal)
│   ├── whisper.rs       # Whisper транскрипция
│   ├── vad.rs           # Voice Activity Detection
│   ├── hotkey.rs        # Глобальные хоткеи (rdev)
│   ├── input.rs         # Ввод текста (enigo)
│   ├── download.rs      # Скачивание моделей
│   └── debug_log.rs     # Debug логирование
├── Cargo.toml
└── tauri.conf.json
```

---

## Зависимости (src-tauri/Cargo.toml)

Уже есть:
- tauri, tauri-plugin-shell, tauri-plugin-dialog, tauri-plugin-fs
- whisper-rs (optional)
- cpal, hound (аудио)
- arboard (clipboard)
- rdev (хоткеи)
- enigo (keyboard)
- zip, chrono, dirs (утилиты)

Добавить:
- reqwest (для скачивания моделей)
- futures-util (для streaming download)

---

## Приоритеты

1. **Высокий:** Интеграция audio + whisper + hotkey (основной функционал)
2. **Средний:** Скачивание моделей (удобство)
3. **Низкий:** Улучшения UI (можно позже)

---

## Заметки

- На Linux нужны права `input` группы для глобальных хоткеев
- На macOS нужно разрешение Input Monitoring
- На Windows может потребоваться запуск от администратора
- Whisper модели большие (75MB - 1.6GB), нужен progress bar
