# Voice Keyboard - План релизов и CI/CD

## Обзор

Этот документ описывает best practices для автоматизации сборки и релизов кросс-платформенного desktop приложения.

---

## 1. Целевые платформы

### Основные
| Платформа | Формат | Архитектура |
|-----------|--------|-------------|
| **macOS** | `.dmg`, `.app` | x86_64, aarch64 (Apple Silicon) |
| **Windows** | `.msi`, `.exe` (NSIS) | x86_64 |
| **Linux** | `.AppImage`, `.deb`, `.rpm` | x86_64 |

### Российские дистрибутивы Linux
Для России актуальны следующие дистрибутивы (используются в госструктурах и крупных компаниях):

| Дистрибутив | База | Формат пакетов | Где используется |
|-------------|------|----------------|------------------|
| **Astra Linux** | Debian | `.deb` | Армия, ФСБ, Газпром, Росатом, РЖД |
| **ALT Linux** | RPM-based | `.rpm` | Образование, корпоративный сектор |
| **ROSA Linux** | Mandriva/RPM | `.rpm` | Госорганизации |
| **RED OS** | RPM-based | `.rpm` | Гос. сертификация ФСТЭК |

**Вывод:** `.deb` + `.rpm` покрывают все российские дистрибутивы.

---

## 2. CI/CD сервисы - сравнение

### Рекомендация: **GitHub Actions** (бесплатно для open source)

| Сервис | macOS | Windows | Linux | Цена (open source) | Цена (private) |
|--------|-------|---------|-------|-------------------|----------------|
| **GitHub Actions** | ✅ | ✅ | ✅ | Бесплатно | 2000 мин/мес free |
| **CircleCI** | ✅ | ✅ | ✅ | 400k credits/мес | От $15/мес |
| **Azure DevOps** | ✅ | ✅ | ✅ | 1800 мин/мес | От $15/мес |
| **Buildkite** | ✅* | ✅* | ✅* | Бесплатно (self-hosted) | $9-35/user/мес |
| **Travis CI** | ✅ | ✅ | ✅ | Ограниченно | Дорого |

*Buildkite требует собственной инфраструктуры для runners

### Почему GitHub Actions лучший выбор:
1. **Бесплатно для public repos** - неограниченные минуты
2. **Нативные runners** для macOS, Windows, Linux
3. **Встроенная интеграция** с GitHub Releases
4. **Tauri Action** - официальный action от команды Tauri
5. **Простота** - не нужны внешние сервисы

---

## 3. Специализированный сервис: CrabNebula Cloud

[CrabNebula](https://crabnebula.dev/) - официальный партнёр Tauri для дистрибуции.

### Возможности:
- CDN для глобальной раздачи приложений
- Интеграция с Tauri Updater (автообновления)
- Метрики скачиваний
- Несколько release channels (stable, beta, nightly)

### Цены:
- **Open Source:** €5/месяц за неограниченные скачивания
- **Стандарт:** €5 за 10k скачиваний

### Когда использовать:
- Если нужны автообновления в приложении
- Если важна аналитика скачиваний
- Для production-ready дистрибуции

**Для начала можно обойтись GitHub Releases, CrabNebula добавить позже.**

---

## 4. Версионирование (Semantic Versioning)

### Формат: `vMAJOR.MINOR.PATCH[-PRERELEASE]`

```
v1.0.0-alpha.1  → Ранняя версия, нестабильная
v1.0.0-beta.1   → Фичи готовы, тестирование
v1.0.0-rc.1     → Release Candidate, финальное тестирование
v1.0.0          → Стабильный релиз
```

### Правила:
- **MAJOR** (1.x.x): Несовместимые изменения API
- **MINOR** (x.1.x): Новые фичи, обратная совместимость
- **PATCH** (x.x.1): Багфиксы

### Порядок приоритета:
```
1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-beta < 1.0.0-rc.1 < 1.0.0
```

---

## 5. Git Workflow для релизов

### Ветки:
```
main              → Стабильный код
develop           → Разработка
release/v1.0.0    → Подготовка релиза
feature/*         → Новые фичи
hotfix/*          → Срочные фиксы
```

### Процесс релиза:

```bash
# 1. Создать release candidate
git checkout develop
git checkout -b release/v1.0.0
# Обновить версию в Cargo.toml, tauri.conf.json
git commit -m "Bump version to 1.0.0-rc.1"
git tag v1.0.0-rc.1
git push origin release/v1.0.0 --tags

# 2. CI автоматически соберёт и создаст pre-release на GitHub

# 3. Тестирование RC...

# 4. Если всё ОК - финальный релиз
git tag v1.0.0
git push origin v1.0.0
git checkout main
git merge release/v1.0.0
git push origin main
```

### GitHub Release типы:
- **Pre-release** (галочка): для alpha, beta, rc версий
- **Latest release**: для стабильных версий

---

## 6. GitHub Actions Workflow

### Структура файлов:
```
.github/
└── workflows/
    ├── ci.yml           # Проверка PR (тесты, lint)
    ├── release.yml      # Сборка при создании тега
    └── nightly.yml      # Ночные сборки (опционально)
```

### release.yml (основной):

```yaml
name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          # macOS Apple Silicon
          - platform: macos-latest
            args: --target aarch64-apple-darwin

          # macOS Intel
          - platform: macos-latest
            args: --target x86_64-apple-darwin

          # Windows
          - platform: windows-latest
            args: ''

          # Linux
          - platform: ubuntu-22.04
            args: ''

    runs-on: ${{ matrix.platform }}

    steps:
      - uses: actions/checkout@v4

      - name: Setup Rust
        uses: dtolnay/rust-action@stable

      - name: Install Linux dependencies
        if: matrix.platform == 'ubuntu-22.04'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev \
            libappindicator3-dev librsvg2-dev patchelf \
            libasound2-dev libxdo-dev

      - name: Build Tauri app
        uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          # macOS signing (опционально)
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
        with:
          tagName: ${{ github.ref_name }}
          releaseName: 'Voice Keyboard ${{ github.ref_name }}'
          releaseBody: 'See the changelog for details.'
          # Пометить как pre-release если тег содержит alpha/beta/rc
          prerelease: ${{ contains(github.ref_name, 'alpha') || contains(github.ref_name, 'beta') || contains(github.ref_name, 'rc') }}
          args: ${{ matrix.args }}
```

---

## 7. Code Signing (подпись приложений)

### macOS (обязательно для распространения)

**Требования:**
- Apple Developer Program ($99/год)
- Developer ID Application certificate
- Developer ID Installer certificate (для .pkg)

**Что нужно в GitHub Secrets:**
```
APPLE_CERTIFICATE           # Base64 .p12 файла
APPLE_CERTIFICATE_PASSWORD  # Пароль от .p12
APPLE_SIGNING_IDENTITY      # "Developer ID Application: Your Name (TEAM_ID)"
APPLE_ID                    # Email Apple Developer
APPLE_PASSWORD              # App-specific password
APPLE_TEAM_ID               # Team ID
```

**Notarization:**
Tauri Action автоматически нотаризирует приложение если указаны credentials.

### Windows (опционально, но рекомендуется)

**Без подписи:** Windows покажет SmartScreen предупреждение
**С подписью:** Нужен EV Code Signing Certificate (~$200-400/год)

Для open source проекта можно начать без подписи - пользователи смогут установить, просто нажав "More info" → "Run anyway".

### Linux
Подпись не требуется.

---

## 8. Артефакты релиза

После успешной сборки GitHub Release будет содержать:

```
voice-keyboard_1.0.0_aarch64.dmg      # macOS Apple Silicon
voice-keyboard_1.0.0_x64.dmg          # macOS Intel
voice-keyboard_1.0.0_x64-setup.exe    # Windows NSIS installer
voice-keyboard_1.0.0_x64.msi          # Windows MSI
voice-keyboard_1.0.0_amd64.deb        # Debian/Ubuntu/Astra
voice-keyboard_1.0.0_amd64.AppImage   # Universal Linux
voice-keyboard_1.0.0_x86_64.rpm       # Fedora/ROSA/ALT/RED OS
```

---

## 9. Автообновления (Tauri Updater)

Tauri поддерживает автоматические обновления. Для этого нужно:

1. Включить updater в `tauri.conf.json`
2. Хостить `latest.json` (GitHub Releases или CrabNebula)
3. Подписывать обновления (TAURI_SIGNING_PRIVATE_KEY)

**Можно добавить позже, когда GUI будет готов.**

---

## 10. Чеклист для первого релиза

### Подготовка репозитория:
- [ ] Обновить README.md с инструкциями установки
- [ ] Создать CHANGELOG.md
- [ ] Добавить LICENSE файл
- [ ] Настроить .github/workflows/release.yml
- [ ] Добавить иконки приложения (icons/)

### Для macOS signing (опционально сейчас):
- [ ] Зарегистрироваться в Apple Developer Program
- [ ] Создать Developer ID сертификаты
- [ ] Добавить secrets в GitHub

### Первый релиз:
- [ ] Обновить версию в Cargo.toml / tauri.conf.json
- [ ] Создать тег: `git tag v0.1.0-alpha.1`
- [ ] Push: `git push origin v0.1.0-alpha.1`
- [ ] Проверить GitHub Actions
- [ ] Протестировать скачанные артефакты
- [ ] Если ОК → v0.1.0

---

## 11. Полезные ссылки

- [Tauri GitHub Action](https://github.com/tauri-apps/tauri-action)
- [Tauri Distribution Guide](https://v2.tauri.app/distribute/)
- [CrabNebula Cloud](https://crabnebula.dev/)
- [macOS Code Signing Guide](https://v2.tauri.app/distribute/sign/macos/)
- [Semantic Versioning](https://semver.org/)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)

---

## Резюме

**Минимальный старт (бесплатно):**
1. GitHub Actions для сборки (tauri-action)
2. GitHub Releases для хостинга
3. Без code signing (пользователи увидят предупреждения)

**Production-ready (рекомендуется):**
1. Apple Developer Program ($99/год) для macOS signing
2. CrabNebula Cloud (€5/мес) для CDN и автообновлений
3. Windows EV Certificate ($200-400/год) - опционально

**Итого минимум для production:** ~$150/год (только Apple Developer)
