# Voice Keyboard для macOS — План реализации

## Цель
Push-to-Talk голосовая клавиатура с локальным распознаванием через Whisper, работающая во всех приложениях macOS.

---

## Архитектура

```
┌─────────────────────────────────────────────────────────────┐
│                      Voice Keyboard                          │
├─────────────────────────────────────────────────────────────┤
│  Global Hotkey Listener                                      │
│  (pynput / rdev)                                            │
│       ↓ key_down                                            │
│  Audio Recorder (sounddevice / cpal)                        │
│       ↓ audio buffer                                        │
│  Whisper.cpp (whisper-rs / pywhisper-cpp)                   │
│       ↓ text                                                │
│  Text Injector (CGEventPost / clipboard+paste)              │
└─────────────────────────────────────────────────────────────┘
```

---

## Коммерциализация — ключевые ограничения

### App Store — НЕВОЗМОЖНО

**Критическое ограничение:** App Store sandbox **запрещает** CGEventPost (эмуляцию ввода).
- Input Monitoring (слушать клавиши) — разрешено
- Accessibility (постить события) — **запрещено**, Apple считает это "escape from sandbox"

Голосовая клавиатура требует вставки текста в другие приложения → **только прямое распространение**.

### Варианты распространения

| Способ | Стоимость | Требования |
|--------|-----------|------------|
| **Direct (Developer ID)** | $99/год | Notarization обязательна |
| **Gumroad/Paddle** | $99/год + комиссия ~5-10% | Developer ID + notarization |
| **Setapp** | $99/год + revenue share | Developer ID + их SDK |

### Лицензирование зависимостей

| Компонент | Лицензия | Коммерция |
|-----------|----------|-----------|
| whisper.cpp | MIT | ✅ Разрешено |
| WhisperKit | MIT | ✅ Разрешено |
| Модели Whisper | MIT | ✅ Разрешено |
| pynput | LGPL | ⚠️ Динамическая линковка OK |
| rdev | MIT | ✅ Разрешено |

---

## Сравнение языков (с учётом коммерциализации)

### Swift (РЕКОМЕНДУЕТСЯ для коммерции)
| + | - |
|---|---|
| Нативная интеграция с macOS APIs | Только Apple платформы |
| WhisperKit — нативный Swift | Более сложный язык |
| Лучшая производительность на Apple Silicon | |
| Простая notarization и code signing | |
| Нативный UI через SwiftUI/AppKit | |

### Rust + Tauri
| + | - |
|---|---|
| Кроссплатформенность (macOS + Windows + Linux) | Сложнее permissions на macOS |
| Один codebase для всех платформ | Web-based UI (медленнее) |
| whisper-rs зрелые биндинги | Проблемы с code signing в Tauri |
| Нативный бинарник | |

### Python
| + | - |
|---|---|
| Быстрый прототип | **НЕ для коммерции** |
| | Код легко декомпилировать |
| | Зависимости сложно упаковать |
| | pynput — LGPL (ограничения) |

### Рекомендация для коммерции

**Swift + WhisperKit** — лучший выбор:
1. Нативная производительность на Apple Silicon
2. Простая интеграция с macOS permissions
3. Code signing и notarization "из коробки"
4. WhisperKit оптимизирован под CoreML/ANE
5. Возможность продать iOS версию в будущем

**Rust** — если нужна кроссплатформенность (Windows/Linux)

---

## Существующие решения (референсы)

1. **[Handy](https://github.com/cjpais/Handy)** — Tauri + whisper.cpp, кроссплатформенный
2. **[OpenSuperWhisper](https://github.com/Starmel/OpenSuperWhisper)** — macOS native (Swift)
3. **[WhisperWriter](https://github.com/savbell/whisper-writer)** — Python + PyQt5
4. **[MisterWhisper](https://github.com/openconcerto/MisterWhisper)** — Push-to-talk с F-клавишами

---

## Необходимые разрешения macOS

| Разрешение | Путь в настройках | Зачем |
|------------|-------------------|-------|
| **Accessibility** | System Settings → Privacy & Security → Accessibility | Глобальный перехват клавиш, эмуляция ввода |
| **Input Monitoring** | System Settings → Privacy & Security → Input Monitoring | Мониторинг нажатий клавиш (Big Sur+) |
| **Microphone** | System Settings → Privacy & Security → Microphone | Запись голоса |

### Как добавить разрешения для своего приложения

1. **Запустить приложение** — оно попросит разрешение или молча откажет
2. **Открыть System Settings → Privacy & Security**
3. **Добавить приложение** в Accessibility и Input Monitoring вручную
4. **Для unsigned apps (Sequoia+)**:
   - Попытаться запустить → закрыть диалог
   - Privacy & Security → Security → "Open Anyway"
   - Либо: `sudo spctl --master-disable` (временно включает "Anywhere")

### Для Python-скриптов
- Добавить **Terminal.app** (или iTerm) в Accessibility/Input Monitoring
- Либо упаковать в .app через py2app/PyInstaller

---

## Компоненты и библиотеки

### Вариант 1: Python

```
pynput           — глобальные hotkeys
sounddevice      — запись аудио (PortAudio wrapper)
pywhisper        — биндинги whisper.cpp (или subprocess whisper.cpp CLI)
pyobjc-framework-Quartz — CGEventPost для эмуляции ввода
pyperclip        — альтернатива: clipboard + Cmd+V
```

### Вариант 2: Rust

```
rdev             — глобальные hotkeys
cpal             — запись аудио
whisper-rs       — биндинги whisper.cpp
enigo            — эмуляция ввода (альтернатива: core-foundation + CGEvent)
```

---

## Модель распознавания

### Whisper.cpp с CoreML (рекомендуется для Apple Silicon)

**Модели по размеру:**
| Модель | Размер | RAM | Скорость | Качество |
|--------|--------|-----|----------|----------|
| tiny | 75 MB | ~400 MB | ~30x realtime | базовое |
| base | 142 MB | ~500 MB | ~20x realtime | хорошее |
| small | 466 MB | ~1 GB | ~10x realtime | очень хорошее |
| medium | 1.5 GB | ~3 GB | ~5x realtime | отличное |
| large-v3-turbo | 1.6 GB | ~3 GB | ~10x realtime | **лучшее** |

**Рекомендация:** `ggml-large-v3-turbo` — скорость small, качество large

### Включение CoreML (ускорение на Apple Neural Engine)

```bash
cmake -DWHISPER_COREML=1 ..
# или для whisper-rs: feature flag "coreml"
```

---

## План реализации (коммерческий путь)

### Этап 1: Прототип на Python (1-2 дня)

Цель: валидация концепции, проверка качества Whisper.

```bash
python3 -m venv venv
source venv/bin/activate
pip install pynput sounddevice numpy pyobjc-framework-Quartz
```

1. Global hotkey → запись → whisper.cpp CLI → clipboard + Cmd+V
2. Тестирование в разных приложениях
3. Оценка latency и качества распознавания

### Этап 2: Production на Swift (основной)

**Структура проекта:**
```
VoiceKeyboard/
├── VoiceKeyboard.xcodeproj
├── VoiceKeyboard/
│   ├── App.swift                 # @main, SwiftUI lifecycle
│   ├── AppDelegate.swift         # NSApplicationDelegate для hotkeys
│   ├── MenuBarController.swift   # Status bar icon + menu
│   ├── HotkeyManager.swift       # CGEventTap для global hotkeys
│   ├── AudioRecorder.swift       # AVAudioEngine запись
│   ├── TranscriptionService.swift # WhisperKit интеграция
│   ├── TextInjector.swift        # CGEventPost для вставки
│   ├── PermissionsManager.swift  # Проверка/запрос разрешений
│   └── Settings/
│       ├── SettingsView.swift
│       └── PreferencesStore.swift
├── Resources/
│   ├── Assets.xcassets
│   └── Info.plist
└── Models/                       # WhisperKit модели (bundle или download)
```

**Ключевые компоненты:**

1. **HotkeyManager** — CGEventTap для глобальных hotkeys
   ```swift
   let eventMask = CGEventMask(1 << CGEventType.keyDown.rawValue)
   let tap = CGEvent.tapCreate(...)
   ```

2. **AudioRecorder** — AVAudioEngine (нативный, не требует PortAudio)
   ```swift
   let audioEngine = AVAudioEngine()
   let inputNode = audioEngine.inputNode
   ```

3. **TranscriptionService** — WhisperKit
   ```swift
   import WhisperKit
   let whisper = try await WhisperKit(model: "large-v3-turbo")
   let result = try await whisper.transcribe(audioPath: url)
   ```

4. **TextInjector** — CGEventPost или clipboard
   ```swift
   // Вариант 1: CGEventPost (требует Accessibility)
   let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true)
   event?.post(tap: .cghidEventTap)

   // Вариант 2: Clipboard + Cmd+V (надёжнее)
   NSPasteboard.general.setString(text, forType: .string)
   // Симулировать Cmd+V
   ```

### Этап 3: Полировка и релиз

1. **UI/UX:**
   - Menu bar app (LSUIElement = YES)
   - Визуальный feedback при записи (иконка меняется)
   - Настройки: выбор hotkey, модели, языка

2. **Permissions flow:**
   - Проверка при запуске: Microphone, Accessibility
   - Понятные инструкции для пользователя

3. **Code signing и Notarization:**
   ```bash
   # Developer ID certificate
   codesign --sign "Developer ID Application: Name" --options runtime VoiceKeyboard.app

   # Notarization
   xcrun notarytool submit VoiceKeyboard.zip --apple-id ... --password ... --team-id ...
   xcrun stapler staple VoiceKeyboard.app
   ```

4. **Распространение:**
   - DMG с drag-to-Applications
   - Auto-update через Sparkle framework
   - Gumroad/Paddle для оплаты

### Этап 4 (опционально): Кроссплатформенность на Rust

Если нужны Windows/Linux:
- `rdev` для hotkeys
- `cpal` для аудио
- `whisper-rs` для распознавания
- Tauri для UI

---

## Потенциальные проблемы и решения

| Проблема | Решение |
|----------|---------|
| Hotkeys не работают | Проверить Accessibility + Input Monitoring |
| Микрофон не пишет | Проверить Microphone permission, запускать из Terminal |
| CGEventPost не работает | Добавить app в Accessibility, использовать clipboard как fallback |
| Whisper медленный | Использовать CoreML, меньшую модель, GPU |
| Текст вставляется в неправильное окно | Минимизировать задержку между нажатием и вставкой |

---

## Альтернативный подход: Input Method (IMKit)

**Не рекомендуется** из-за:
- Очень плохая документация Apple
- Множество известных багов
- Нельзя распространять через App Store
- Требует установки в `/Library/Input Methods`

Push-to-talk + clipboard/CGEvent — более надёжный подход.

---

## Ссылки

### Распознавание речи
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) — порт Whisper на C++
- [WhisperKit](https://github.com/argmaxinc/WhisperKit) — нативный Swift для Apple Silicon
- [whisper-rs](https://github.com/tazz4843/whisper-rs) — Rust биндинги

### Hotkeys и ввод
- [pynput](https://pynput.readthedocs.io/) — Python keyboard/mouse
- [rdev](https://github.com/Narsil/rdev) — Rust keyboard/mouse
- [CGEvent docs](https://developer.apple.com/documentation/coregraphics/cgevent)

### Существующие решения
- [Handy](https://github.com/cjpais/Handy)
- [OpenSuperWhisper](https://github.com/Starmel/OpenSuperWhisper)
- [WhisperWriter](https://github.com/savbell/whisper-writer)

### macOS разрешения
- [Accessibility permissions](https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac)
- [Input Monitoring](https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac)

---

## Итоговая рекомендация

### Для коммерческого продукта: **Swift + WhisperKit**

```
┌─────────────────────────────────────────────────────────┐
│  Прототип (Python)  →  Production (Swift)  →  Релиз    │
│       1-2 дня              2-4 недели          +       │
└─────────────────────────────────────────────────────────┘
```

**Почему Swift:**
1. **Производительность** — WhisperKit использует CoreML/ANE, до 16% быстрее whisper.cpp
2. **Интеграция** — нативные macOS APIs для permissions, hotkeys, audio
3. **Распространение** — простая notarization, Xcode всё делает сам
4. **Защита кода** — скомпилированный бинарник, не декомпилировать как Python
5. **Будущее** — тот же код можно адаптировать для iOS/iPadOS

**Стоимость входа:**
- Apple Developer Program: $99/год
- Xcode: бесплатно
- WhisperKit: MIT, бесплатно

**Конкуренты для анализа:**
- [Superwhisper](https://superwhisper.com/) — $99/год, Swift
- [MacWhisper](https://goodsnooze.gumroad.com/l/macwhisper) — $29-49, Swift
- [Whisper Transcription](https://apps.apple.com/app/whisper-transcription/id1668083311) — $29

### Для кроссплатформы: **Rust + Tauri**

Если нужны Windows/Linux — Rust единственный разумный выбор.
Но это удвоит время разработки из-за platform-specific кода.
