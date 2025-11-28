// Voice Keyboard UI Application

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// State
let transcriptions = [];
let config = {
    model: 'ggml-small.bin',
    language: 'auto',
    hotkey: 'fn',
    inputMethod: 'keyboard'
};

// Models configuration
const MODELS = [
    { id: 'ggml-tiny.bin', name: 'Tiny', desc: 'Fastest, lowest accuracy', size: '75 MB' },
    { id: 'ggml-base.bin', name: 'Base', desc: 'Fast, good accuracy', size: '142 MB' },
    { id: 'ggml-small.bin', name: 'Small', desc: 'Balanced speed/accuracy', size: '466 MB' },
    { id: 'ggml-medium.bin', name: 'Medium', desc: 'High accuracy, slower', size: '1.5 GB' },
    { id: 'ggml-large-v3-turbo.bin', name: 'Large v3 Turbo', desc: 'Best accuracy', size: '1.6 GB' }
];

const LANGUAGES = [
    { code: 'auto', name: 'Auto-detect' },
    { code: 'en', name: 'English' },
    { code: 'ru', name: 'Russian' },
    { code: 'de', name: 'German' },
    { code: 'fr', name: 'French' },
    { code: 'es', name: 'Spanish' },
    { code: 'it', name: 'Italian' },
    { code: 'pt', name: 'Portuguese' },
    { code: 'zh', name: 'Chinese' },
    { code: 'ja', name: 'Japanese' },
    { code: 'ko', name: 'Korean' }
];

// DOM Elements
let elements = {};

// Initialize app
document.addEventListener('DOMContentLoaded', async () => {
    cacheElements();
    setupTabs();
    setupEventListeners();
    await loadConfig();
    await loadTranscriptions();
    renderModels();
    renderLanguages();
    setupTauriListeners();
});

function cacheElements() {
    elements = {
        tabs: document.querySelectorAll('.tab'),
        tabContents: document.querySelectorAll('.tab-content'),
        statusIndicator: document.querySelector('.status-indicator'),
        statusText: document.querySelector('.status-text'),
        hotkeyHint: document.querySelector('.hotkey-hint kbd'),
        transcriptionLog: document.getElementById('transcription-log'),
        clearLogBtn: document.getElementById('clear-log'),
        reportIssueBtn: document.getElementById('report-issue'),
        modelsList: document.getElementById('models-list'),
        languageSelect: document.getElementById('language-select'),
        hotkeySelect: document.getElementById('hotkey-select'),
        inputMethodSelect: document.getElementById('input-method-select'),
        saveSettingsBtn: document.getElementById('save-settings'),
        reportModal: document.getElementById('report-modal'),
        cancelReportBtn: document.getElementById('cancel-report'),
        createReportBtn: document.getElementById('create-report')
    };
}

function setupTabs() {
    elements.tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const tabId = tab.dataset.tab;

            // Update tab buttons
            elements.tabs.forEach(t => t.classList.remove('active'));
            tab.classList.add('active');

            // Update tab content
            elements.tabContents.forEach(content => {
                content.classList.remove('active');
                if (content.id === `${tabId}-tab`) {
                    content.classList.add('active');
                }
            });
        });
    });
}

function setupEventListeners() {
    // Clear log
    elements.clearLogBtn.addEventListener('click', async () => {
        transcriptions = [];
        renderTranscriptions();
        try {
            await invoke('clear_transcriptions');
        } catch (e) {
            console.error('Failed to clear transcriptions:', e);
        }
    });

    // Report issue
    elements.reportIssueBtn.addEventListener('click', () => {
        elements.reportModal.classList.remove('hidden');
    });

    elements.cancelReportBtn.addEventListener('click', () => {
        elements.reportModal.classList.add('hidden');
    });

    elements.createReportBtn.addEventListener('click', async () => {
        elements.createReportBtn.disabled = true;
        elements.createReportBtn.textContent = 'Creating...';

        try {
            const zipPath = await invoke('create_debug_report');
            await invoke('open_github_issue', { zipPath });
            elements.reportModal.classList.add('hidden');
        } catch (e) {
            console.error('Failed to create report:', e);
            alert('Failed to create debug report: ' + e);
        } finally {
            elements.createReportBtn.disabled = false;
            elements.createReportBtn.innerHTML = '<span class="icon">⚠️</span> Create & Open GitHub Issue';
        }
    });

    // Save settings
    elements.saveSettingsBtn.addEventListener('click', saveSettings);

    // Settings changes
    elements.languageSelect.addEventListener('change', (e) => {
        config.language = e.target.value;
    });

    elements.hotkeySelect.addEventListener('change', (e) => {
        config.hotkey = e.target.value;
        updateHotkeyHint();
    });

    elements.inputMethodSelect.addEventListener('change', (e) => {
        config.inputMethod = e.target.value;
    });
}

async function setupTauriListeners() {
    // Listen for status updates
    await listen('status-update', (event) => {
        updateStatus(event.payload.status, event.payload.text);
    });

    // Listen for new transcriptions
    await listen('transcription', (event) => {
        addTranscription(event.payload);
    });

    // Listen for show window request
    await listen('show-window', () => {
        // Window is already shown by Tauri
    });
}

async function loadConfig() {
    try {
        const savedConfig = await invoke('get_config');
        if (savedConfig) {
            config = { ...config, ...savedConfig };
        }
    } catch (e) {
        console.error('Failed to load config:', e);
    }

    // Apply config to UI
    elements.hotkeySelect.value = config.hotkey;
    elements.inputMethodSelect.value = config.inputMethod;
    updateHotkeyHint();
}

async function loadTranscriptions() {
    try {
        transcriptions = await invoke('get_transcriptions');
    } catch (e) {
        console.error('Failed to load transcriptions:', e);
        transcriptions = [];
    }
    renderTranscriptions();
}

function renderTranscriptions() {
    if (transcriptions.length === 0) {
        elements.transcriptionLog.innerHTML = '<p class="placeholder">Transcriptions will appear here...</p>';
        return;
    }

    elements.transcriptionLog.innerHTML = transcriptions.map(t => `
        <div class="log-entry ${t.is_continuation ? 'continuation' : ''}">
            <div class="timestamp">${formatTimestamp(t.timestamp)}</div>
            <div class="text">${escapeHtml(t.text)}</div>
            <div class="meta">
                ${t.duration ? `Duration: ${t.duration.toFixed(1)}s` : ''}
                ${t.is_continuation ? ' (continuation)' : ''}
            </div>
        </div>
    `).join('');

    // Scroll to bottom
    elements.transcriptionLog.scrollTop = elements.transcriptionLog.scrollHeight;
}

function addTranscription(transcription) {
    transcriptions.push(transcription);
    renderTranscriptions();
}

function renderModels() {
    elements.modelsList.innerHTML = MODELS.map(model => `
        <div class="model-item ${config.model === model.id ? 'selected' : ''}" data-model="${model.id}">
            <div class="radio"></div>
            <div class="model-info">
                <div class="model-name">${model.name}</div>
                <div class="model-desc">${model.desc}</div>
            </div>
            <div class="model-size">${model.size}</div>
            <div class="model-status" id="status-${model.id}">Checking...</div>
        </div>
    `).join('');

    // Add click handlers
    elements.modelsList.querySelectorAll('.model-item').forEach(item => {
        item.addEventListener('click', () => {
            elements.modelsList.querySelectorAll('.model-item').forEach(i => i.classList.remove('selected'));
            item.classList.add('selected');
            config.model = item.dataset.model;
        });
    });

    // Check model statuses
    checkModelStatuses();
}

async function checkModelStatuses() {
    for (const model of MODELS) {
        const statusEl = document.getElementById(`status-${model.id}`);
        try {
            const isDownloaded = await invoke('check_model_exists', { modelName: model.id });
            statusEl.textContent = isDownloaded ? 'Downloaded' : 'Not downloaded';
            statusEl.className = `model-status ${isDownloaded ? 'downloaded' : 'not-downloaded'}`;
        } catch (e) {
            statusEl.textContent = 'Unknown';
            statusEl.className = 'model-status';
        }
    }
}

function renderLanguages() {
    elements.languageSelect.innerHTML = LANGUAGES.map(lang =>
        `<option value="${lang.code}" ${config.language === lang.code ? 'selected' : ''}>${lang.name}</option>`
    ).join('');
}

async function saveSettings() {
    elements.saveSettingsBtn.disabled = true;
    elements.saveSettingsBtn.textContent = 'Saving...';

    try {
        await invoke('save_config', { config });
        elements.saveSettingsBtn.textContent = 'Saved!';
        setTimeout(() => {
            elements.saveSettingsBtn.textContent = 'Save Settings';
            elements.saveSettingsBtn.disabled = false;
        }, 1500);
    } catch (e) {
        console.error('Failed to save config:', e);
        alert('Failed to save settings: ' + e);
        elements.saveSettingsBtn.textContent = 'Save Settings';
        elements.saveSettingsBtn.disabled = false;
    }
}

function updateStatus(status, text) {
    elements.statusIndicator.className = `status-indicator ${status}`;
    elements.statusText.textContent = text || getStatusText(status);
}

function getStatusText(status) {
    switch (status) {
        case 'idle': return 'Ready';
        case 'recording': return 'Recording...';
        case 'processing': return 'Processing...';
        default: return status;
    }
}

function updateHotkeyHint() {
    const hotkeyNames = {
        'fn': 'Fn',
        'ctrl': 'Ctrl',
        'ctrlright': 'Right Ctrl',
        'alt': 'Alt',
        'altright': 'Right Alt',
        'shift': 'Shift',
        'cmd': 'Cmd'
    };
    elements.hotkeyHint.textContent = hotkeyNames[config.hotkey] || config.hotkey;
}

function formatTimestamp(ts) {
    if (!ts) return '';
    const date = new Date(ts);
    return date.toLocaleTimeString();
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}
