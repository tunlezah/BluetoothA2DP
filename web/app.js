/**
 * SoundSync Web Application
 *
 * Handles real-time Bluetooth management, EQ control, and UI updates.
 * Uses a WebSocket connection to /ws/status for live state updates.
 * REST API calls to /api/* for control actions.
 *
 * No external dependencies — vanilla JS with Web Components pattern.
 */

'use strict';

const SoundSync = (() => {
  // ── State ──────────────────────────────────────────────────────────────────

  const state = {
    ws: null,
    wsReconnectDelay: 1000,
    wsReconnectTimer: null,
    devices: [],
    status: 'unavailable',
    activeDevice: null,
    eq: { bands: [], enabled: true },
    presets: [],
    scanning: false,
    settingsOpen: false,
    eqDirtyTimer: null,
  };

  // EQ band frequencies matching the backend
  const EQ_FREQS = [60, 120, 250, 500, 1000, 2000, 4000, 8000, 12000, 16000];
  const EQ_FREQ_LABELS = ['60Hz', '120Hz', '250Hz', '500Hz', '1kHz', '2kHz', '4kHz', '8kHz', '12kHz', '16kHz'];

  // ── Initialisation ─────────────────────────────────────────────────────────

  function init() {
    buildEqSliders();
    connectWebSocket();
    loadPresets();

    // Keyboard shortcut: S = scan
    document.addEventListener('keydown', (e) => {
      if (e.target.tagName === 'INPUT' || e.target.tagName === 'SELECT') return;
      if (e.key === 's' || e.key === 'S') toggleScan();
      if (e.key === 'Escape') closeSettings();
    });
  }

  // ── WebSocket ──────────────────────────────────────────────────────────────

  function connectWebSocket() {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${location.host}/ws/status`;

    if (state.ws) {
      state.ws.onclose = null;
      state.ws.close();
    }

    state.ws = new WebSocket(url);

    state.ws.onopen = () => {
      state.wsReconnectDelay = 1000;
      clearTimeout(state.wsReconnectTimer);
      updateStatusPill('connecting', 'Connected');
    };

    state.ws.onmessage = (e) => {
      try {
        const event = JSON.parse(e.data);
        handleServerEvent(event);
      } catch (err) {
        console.warn('Failed to parse WebSocket message:', err);
      }
    };

    state.ws.onerror = () => {
      // Will trigger onclose
    };

    state.ws.onclose = () => {
      updateStatusPill('unavailable', 'Reconnecting…');
      // Exponential backoff reconnect
      state.wsReconnectTimer = setTimeout(() => {
        state.wsReconnectDelay = Math.min(state.wsReconnectDelay * 2, 30000);
        connectWebSocket();
      }, state.wsReconnectDelay);
    };
  }

  // ── Event handling ─────────────────────────────────────────────────────────

  function handleServerEvent(event) {
    switch (event.type) {
      case 'state_snapshot':
        applySnapshot(event.data);
        break;

      case 'bluetooth_status_changed':
        updateBluetoothStatus(event.data.status);
        break;

      case 'device_state_changed':
        updateDeviceState(event.data);
        break;

      case 'device_list_updated':
        fetchDevices();
        break;

      case 'stream_started':
        updateStreamStatus(true, event.data.address);
        break;

      case 'stream_stopped':
        updateStreamStatus(false, event.data.address);
        break;

      case 'eq_changed':
        fetchEq();
        break;

      case 'error':
        showToast(event.data.message, 'error');
        break;

      default:
        // Unknown event type — ignore
        break;
    }
  }

  function applySnapshot(data) {
    // Apply full state snapshot received on WebSocket connect
    updateBluetoothStatus(data.status);

    if (data.devices) {
      state.devices = data.devices;
      renderDeviceList();
    }

    if (data.eq && data.eq.length) {
      state.eq.bands = data.eq;
      updateEqSliders(data.eq);
    }

    if (data.active_device) {
      state.activeDevice = data.active_device;
      updatePlaybackPanel();
    }
  }

  // ── API calls ──────────────────────────────────────────────────────────────

  async function apiFetch(path, options = {}) {
    try {
      const res = await fetch(path, {
        headers: { 'Content-Type': 'application/json' },
        ...options,
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(err.error || res.statusText);
      }
      return await res.json();
    } catch (err) {
      showToast(err.message, 'error');
      throw err;
    }
  }

  async function fetchDevices() {
    try {
      const data = await apiFetch('/api/devices');
      state.devices = data.devices || [];
      renderDeviceList();
    } catch (_) {}
  }

  async function fetchEq() {
    try {
      const data = await apiFetch('/api/eq');
      state.eq = data;
      updateEqSliders(data.bands);
      updateEqToggle(data.enabled);
    } catch (_) {}
  }

  async function loadPresets() {
    try {
      const data = await apiFetch('/api/eq/presets');
      state.presets = data.presets || [];
      renderPresetSelect();
    } catch (_) {}
  }

  // ── Bluetooth control ──────────────────────────────────────────────────────

  async function toggleScan() {
    state.scanning = !state.scanning;
    try {
      await apiFetch('/api/bluetooth/scan', {
        method: 'POST',
        body: JSON.stringify({ scanning: state.scanning }),
      });
      const btn = document.getElementById('btn-scan');
      if (btn) btn.classList.toggle('active', state.scanning);
      showToast(state.scanning ? 'Scanning for devices…' : 'Scan stopped', 'info');
    } catch (_) {
      state.scanning = !state.scanning; // revert
    }
  }

  async function connectDevice(address) {
    showToast('Connecting…', 'info');
    try {
      await apiFetch('/api/bluetooth/connect', {
        method: 'POST',
        body: JSON.stringify({ address }),
      });
    } catch (_) {}
  }

  async function disconnectDevice(address) {
    try {
      await apiFetch('/api/bluetooth/disconnect', {
        method: 'POST',
        body: JSON.stringify({ address }),
      });
    } catch (_) {}
  }

  async function removeDevice(address) {
    if (!confirm('Remove this device from trusted list?')) return;
    try {
      await apiFetch('/api/bluetooth/device', {
        method: 'DELETE',
        body: JSON.stringify({ address }),
      });
      showToast('Device removed', 'info');
    } catch (_) {}
  }

  // ── EQ control ─────────────────────────────────────────────────────────────

  function onBandChanged(index, value) {
    // Update the gain label immediately for responsiveness
    const band = document.querySelector(`.eq-band[data-index="${index}"]`);
    if (band) {
      const label = band.querySelector('.eq-gain-label');
      if (label) {
        const db = parseFloat(value).toFixed(1);
        label.textContent = db > 0 ? `+${db}` : `${db}`;
      }
    }

    // Update local state
    if (state.eq.bands[index]) {
      state.eq.bands[index].gain_db = parseFloat(value);
    }

    // Debounce API call — send 300ms after last change
    clearTimeout(state.eqDirtyTimer);
    state.eqDirtyTimer = setTimeout(() => pushEqUpdate(), 300);
  }

  async function pushEqUpdate() {
    const bands = state.eq.bands.map(b => ({ freq: b.freq, gain_db: b.gain_db }));
    try {
      await apiFetch('/api/eq', {
        method: 'POST',
        body: JSON.stringify({ bands }),
      });
    } catch (_) {}
  }

  async function toggleEq() {
    state.eq.enabled = !state.eq.enabled;
    updateEqToggle(state.eq.enabled);
    try {
      const bands = state.eq.bands.map(b => ({ freq: b.freq, gain_db: b.gain_db }));
      await apiFetch('/api/eq', {
        method: 'POST',
        body: JSON.stringify({ bands, enabled: state.eq.enabled }),
      });
    } catch (_) {
      state.eq.enabled = !state.eq.enabled;
      updateEqToggle(state.eq.enabled);
    }
  }

  async function applyPreset(name) {
    if (!name) return;
    try {
      const data = await apiFetch('/api/eq/preset', {
        method: 'POST',
        body: JSON.stringify({ name }),
      });
      state.eq = data;
      updateEqSliders(data.bands);
      updateEqToggle(data.enabled);
      showToast(`Preset "${name}" applied`, 'success');
    } catch (_) {
      document.getElementById('eq-preset-select').value = '';
    }
  }

  async function savePreset() {
    const name = prompt('Preset name:');
    if (!name || !name.trim()) return;
    try {
      await apiFetch('/api/eq/preset/save', {
        method: 'POST',
        body: JSON.stringify({ name: name.trim() }),
      });
      await loadPresets();
      showToast(`Preset "${name.trim()}" saved`, 'success');
    } catch (_) {}
  }

  async function resetEq() {
    const flatBands = EQ_FREQS.map(freq => ({ freq, gain_db: 0.0 }));
    try {
      await apiFetch('/api/eq', {
        method: 'POST',
        body: JSON.stringify({ bands: flatBands }),
      });
      state.eq.bands = flatBands;
      updateEqSliders(flatBands);
      document.getElementById('eq-preset-select').value = '';
      showToast('EQ reset to flat', 'info');
    } catch (_) {}
  }

  function setVolume(value) {
    // Volume control updates are informational — actual volume control
    // goes through PipeWire/ALSA which is handled at system level
    // This slider provides visual feedback for the user
    const icon = document.querySelector('.volume-icon');
    if (value < 30) {
      document.querySelectorAll('.volume-icon')[0].textContent = '🔇';
    } else if (value < 60) {
      document.querySelectorAll('.volume-icon')[0].textContent = '🔈';
    } else {
      document.querySelectorAll('.volume-icon')[0].textContent = '🔉';
    }
  }

  // ── Settings ───────────────────────────────────────────────────────────────

  function toggleSettings() {
    state.settingsOpen = !state.settingsOpen;
    const content = document.getElementById('settings-content');
    const chevron = document.getElementById('drawer-chevron');
    const toggle = document.querySelector('.drawer-toggle');

    if (content) content.classList.toggle('open', state.settingsOpen);
    if (chevron) chevron.classList.toggle('open', state.settingsOpen);
    if (toggle) toggle.setAttribute('aria-expanded', state.settingsOpen);

    // Update the app grid row height for the drawer
    const app = document.getElementById('app');
    if (app) {
      app.style.gridTemplateRows = state.settingsOpen
        ? '72px 1fr 320px'
        : '72px 1fr 48px';
    }

    if (state.settingsOpen) {
      // Populate settings fields
      const nameInput = document.getElementById('settings-name');
      if (nameInput && !nameInput.value) {
        const titleEl = document.querySelector('.app-name');
        nameInput.placeholder = titleEl ? titleEl.textContent : 'SoundSync';
      }
      refreshSettings();
    }
  }

  function closeSettings() {
    if (state.settingsOpen) toggleSettings();
  }

  async function refreshSettings() {
    try {
      const data = await apiFetch('/api/status');
      const uptimeEl = document.getElementById('settings-uptime');
      if (uptimeEl) uptimeEl.textContent = formatUptime(data.uptime_seconds);

      const countEl = document.getElementById('settings-device-count');
      if (countEl) countEl.textContent = data.device_count;
    } catch (_) {}
  }

  async function applyName() {
    const input = document.getElementById('settings-name');
    if (!input) return;
    const name = input.value.trim();
    if (!name) { showToast('Enter a name first', 'error'); return; }

    try {
      await apiFetch('/api/bluetooth/name', {
        method: 'POST',
        body: JSON.stringify({ name }),
      });
      showToast(`Speaker name set to "${name}"`, 'success');
      const appName = document.querySelector('.app-name');
      if (appName) appName.textContent = name;
    } catch (_) {}
  }

  async function refresh() {
    await fetchDevices();
    await fetchEq();
    showToast('Status refreshed', 'info');
  }

  // ── UI rendering ───────────────────────────────────────────────────────────

  function buildEqSliders() {
    const container = document.getElementById('eq-sliders');
    if (!container) return;

    container.innerHTML = '';

    EQ_FREQS.forEach((freq, i) => {
      const band = document.createElement('div');
      band.className = 'eq-band';
      band.dataset.index = i;
      band.setAttribute('role', 'group');
      band.setAttribute('aria-label', `${EQ_FREQ_LABELS[i]} band`);

      band.innerHTML = `
        <span class="eq-gain-label" aria-live="polite">0.0</span>
        <div class="eq-slider-wrap">
          <div class="eq-zero-line" aria-hidden="true"></div>
          <input type="range"
                 class="eq-slider"
                 min="-12" max="12" step="0.5" value="0"
                 aria-label="${EQ_FREQ_LABELS[i]} gain"
                 aria-valuemin="-12" aria-valuemax="12" aria-valuenow="0"
                 data-band="${i}"
                 oninput="SoundSync.onBandChanged(${i}, this.value)"
                 ondblclick="SoundSync.resetBand(${i})" />
        </div>
        <span class="eq-freq-label">${EQ_FREQ_LABELS[i]}</span>
      `;

      container.appendChild(band);
    });

    // Initialise with default flat bands
    state.eq.bands = EQ_FREQS.map(freq => ({ freq, gain_db: 0.0 }));
  }

  function updateEqSliders(bands) {
    if (!bands || bands.length !== 10) return;
    state.eq.bands = bands;

    bands.forEach((band, i) => {
      const slider = document.querySelector(`.eq-slider[data-band="${i}"]`);
      const label = document.querySelector(`.eq-band[data-index="${i}"] .eq-gain-label`);

      if (slider) {
        slider.value = band.gain_db;
        slider.setAttribute('aria-valuenow', band.gain_db);
      }
      if (label) {
        const db = parseFloat(band.gain_db).toFixed(1);
        label.textContent = db > 0 ? `+${db}` : `${db}`;
        label.style.color = band.gain_db > 0 ? 'var(--teal)' : band.gain_db < 0 ? 'var(--orange)' : 'var(--text-muted)';
      }
    });
  }

  function resetBand(index) {
    const slider = document.querySelector(`.eq-slider[data-band="${index}"]`);
    if (slider) {
      slider.value = 0;
      onBandChanged(index, 0);
    }
  }

  function updateEqToggle(enabled) {
    state.eq.enabled = enabled;
    const btn = document.getElementById('eq-toggle');
    if (btn) btn.classList.toggle('active', enabled);

    // Dim the sliders when EQ is off
    const sliders = document.getElementById('eq-sliders');
    if (sliders) sliders.style.opacity = enabled ? '1' : '0.4';
  }

  function renderPresetSelect() {
    const select = document.getElementById('eq-preset-select');
    if (!select) return;

    const current = select.value;
    select.innerHTML = '<option value="">— Preset —</option>';

    // Group: built-in presets
    const builtins = ['flat', 'bass_boost', 'treble_boost', 'vinyl_warm', 'speech', 'rock', 'classical', 'electronic'];
    const builtin = state.presets.filter(p => builtins.includes(p));
    const custom = state.presets.filter(p => !builtins.includes(p));

    if (builtin.length) {
      const group = document.createElement('optgroup');
      group.label = 'Built-in';
      builtin.forEach(name => {
        const opt = document.createElement('option');
        opt.value = name;
        opt.textContent = formatPresetName(name);
        group.appendChild(opt);
      });
      select.appendChild(group);
    }

    if (custom.length) {
      const group = document.createElement('optgroup');
      group.label = 'Saved';
      custom.forEach(name => {
        const opt = document.createElement('option');
        opt.value = name;
        opt.textContent = name;
        group.appendChild(opt);
      });
      select.appendChild(group);
    }

    // Restore selection if still valid
    if (state.presets.includes(current)) {
      select.value = current;
    }
  }

  function formatPresetName(name) {
    return name.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
  }

  function renderDeviceList() {
    const list = document.getElementById('device-list');
    const noDevices = document.getElementById('no-devices');
    if (!list) return;

    const devices = state.devices;

    if (!devices.length) {
      if (noDevices) noDevices.style.display = 'flex';
      // Remove existing device cards
      list.querySelectorAll('.device-card').forEach(el => el.remove());
      return;
    }

    if (noDevices) noDevices.style.display = 'none';

    // Reconcile existing cards with current device list
    const existingCards = {};
    list.querySelectorAll('.device-card').forEach(el => {
      existingCards[el.dataset.address] = el;
    });

    // Add or update cards
    const seen = new Set();
    devices.forEach(device => {
      seen.add(device.address);
      let card = existingCards[device.address];

      if (!card) {
        card = document.createElement('div');
        card.className = 'device-card';
        card.dataset.address = device.address;
        card.setAttribute('role', 'listitem');
        list.appendChild(card);
      }

      updateDeviceCard(card, device);
    });

    // Remove cards for devices no longer present
    Object.entries(existingCards).forEach(([addr, el]) => {
      if (!seen.has(addr)) el.remove();
    });
  }

  function updateDeviceCard(card, device) {
    const isConnected = isDeviceConnected(device.state);
    const isStreaming = device.state === 'audio_active';
    const isConnecting = device.state === 'pairing' || device.state === 'connecting';

    card.className = `device-card ${isStreaming ? 'streaming' : isConnected ? 'connected' : ''}`;
    card.setAttribute('aria-label', `${device.name} — ${device.state}`);

    const rssiText = device.rssi != null ? `${device.rssi} dBm` : '';
    const stateText = formatDeviceState(device.state);
    const btnText = isConnected ? 'Disconnect' : isConnecting ? '…' : 'Connect';
    const btnClass = isConnected ? 'connect-btn disconnect' : 'connect-btn';

    card.innerHTML = `
      <div class="device-info">
        <div class="device-name">${escapeHtml(device.name)}</div>
        <div class="device-meta">
          ${rssiText ? `<span class="device-rssi">${rssiText}</span>` : ''}
          <span class="device-state">${stateText}</span>
        </div>
      </div>
      <button class="${btnClass}"
              aria-label="${isConnected ? 'Disconnect from' : 'Connect to'} ${escapeHtml(device.name)}"
              onclick="SoundSync.${isConnected ? 'disconnectDevice' : 'connectDevice'}('${escapeHtml(device.address)}')"
              ${isConnecting ? 'disabled' : ''}>
        ${btnText}
      </button>
    `;
  }

  function isDeviceConnected(stateStr) {
    return ['connected', 'profile_negotiated', 'pipewire_source_ready', 'audio_active'].includes(stateStr);
  }

  function formatDeviceState(state) {
    const labels = {
      disconnected: 'Disconnected',
      discovered: 'Nearby',
      pairing: 'Pairing…',
      paired: 'Paired',
      connected: 'Connected',
      profile_negotiated: 'A2DP Ready',
      pipewire_source_ready: 'Audio Ready',
      audio_active: 'Streaming',
    };
    return labels[state] || state;
  }

  function updateDeviceState(data) {
    const existing = state.devices.find(d => d.address === data.address);
    if (existing) {
      existing.state = data.state;
      if (data.name) existing.name = data.name;
    } else if (data.name) {
      state.devices.push({ address: data.address, name: data.name, state: data.state });
    }
    renderDeviceList();
    updatePlaybackPanel();
  }

  function updatePlaybackPanel() {
    const active = state.devices.find(d => isDeviceConnected(d.state));
    const streaming = state.devices.find(d => d.state === 'audio_active');

    const deviceEl = document.getElementById('playback-device');
    const stateEl = document.getElementById('playback-state');
    const streamEl = document.getElementById('stat-stream');
    const waveform = document.getElementById('waveform');

    if (streaming) {
      if (deviceEl) deviceEl.textContent = streaming.name;
      if (stateEl) { stateEl.textContent = 'Audio Active'; stateEl.style.color = 'var(--green)'; }
      if (streamEl) { streamEl.textContent = 'Active'; streamEl.className = 'stat-value active'; }
      if (waveform) waveform.classList.add('active');
      state.activeDevice = streaming.address;
    } else if (active) {
      if (deviceEl) deviceEl.textContent = active.name;
      if (stateEl) { stateEl.textContent = formatDeviceState(active.state); stateEl.style.color = 'var(--teal)'; }
      if (streamEl) { streamEl.textContent = 'Connected'; streamEl.className = 'stat-value'; }
      if (waveform) waveform.classList.remove('active');
    } else {
      if (deviceEl) deviceEl.textContent = '—';
      if (stateEl) { stateEl.textContent = 'No active stream'; stateEl.style.color = 'var(--text-muted)'; }
      if (streamEl) { streamEl.textContent = 'Idle'; streamEl.className = 'stat-value inactive'; }
      if (waveform) waveform.classList.remove('active');
    }
  }

  function updateStreamStatus(active, address) {
    const streamEl = document.getElementById('stat-stream');
    const waveform = document.getElementById('waveform');

    if (active) {
      if (streamEl) { streamEl.textContent = 'Active'; streamEl.className = 'stat-value active'; }
      if (waveform) waveform.classList.add('active');
      state.activeDevice = address;

      // Update device state
      const device = state.devices.find(d => d.address === address || address === 'pipewire_source');
      if (device) device.state = 'audio_active';
    } else {
      if (streamEl) { streamEl.textContent = 'Idle'; streamEl.className = 'stat-value inactive'; }
      if (waveform) waveform.classList.remove('active');
    }

    renderDeviceList();
    updatePlaybackPanel();
  }

  function updateBluetoothStatus(status) {
    state.status = status;
    state.scanning = status === 'scanning';

    // Map backend status to display
    let display, pillStatus;
    if (status === 'scanning') {
      display = 'Scanning…';
      pillStatus = 'scanning';
    } else if (status === 'ready') {
      display = 'Ready';
      pillStatus = 'ready';
    } else if (status.startsWith('error')) {
      display = 'Error';
      pillStatus = 'unavailable';
    } else {
      display = 'Unavailable';
      pillStatus = 'unavailable';
    }

    // Check if any device is streaming
    const hasStream = state.devices.some(d => d.state === 'audio_active');
    if (hasStream) {
      display = 'Streaming';
      pillStatus = 'streaming';
    } else if (state.devices.some(d => isDeviceConnected(d.state))) {
      display = 'Connected';
      pillStatus = 'connected';
    }

    updateStatusPill(pillStatus, display);

    // Update scan button state
    const scanBtn = document.getElementById('btn-scan');
    if (scanBtn) scanBtn.classList.toggle('active', state.scanning);

    // Update PipeWire status indicator
    const pwEl = document.getElementById('stat-pw');
    if (pwEl) {
      pwEl.textContent = status !== 'unavailable' ? 'Active' : '—';
      pwEl.className = `stat-value ${status !== 'unavailable' ? 'active' : 'inactive'}`;
    }
  }

  function updateStatusPill(status, text) {
    const pill = document.getElementById('status-pill');
    const textEl = document.getElementById('status-text');
    if (pill) pill.setAttribute('data-status', status);
    if (textEl) textEl.textContent = text;
  }

  // ── Toast notifications ────────────────────────────────────────────────────

  function showToast(message, type = 'info') {
    const container = document.getElementById('toast-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    toast.setAttribute('role', 'alert');

    container.appendChild(toast);

    // Auto-remove after 3 seconds
    setTimeout(() => {
      toast.style.opacity = '0';
      toast.style.transform = 'translateX(20px)';
      toast.style.transition = 'opacity 200ms, transform 200ms';
      setTimeout(() => toast.remove(), 220);
    }, 3000);
  }

  // ── Utility functions ──────────────────────────────────────────────────────

  function escapeHtml(str) {
    if (typeof str !== 'string') return '';
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function formatUptime(seconds) {
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    return `${h}h ${m}m`;
  }

  // ── Public API ─────────────────────────────────────────────────────────────

  // Initialise when DOM is ready
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }

  return {
    toggleScan,
    connectDevice,
    disconnectDevice,
    removeDevice,
    onBandChanged,
    resetBand,
    toggleEq,
    applyPreset,
    savePreset,
    resetEq,
    setVolume,
    toggleSettings,
    applyName,
    refresh,
    // Expose state for debugging
    get state() { return state; },
  };
})();
