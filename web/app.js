/**
 * SoundSync Web Application
 *
 * Handles real-time Bluetooth management, spectrum visualiser, and track
 * info display. Uses a WebSocket connection to /ws/status for live state
 * updates and REST API calls to /api/* for control actions.
 *
 * No external dependencies — vanilla JS only.
 */

'use strict';

const SoundSync = (() => {

  // ── Application state ──────────────────────────────────────────────────────

  const state = {
    ws: null,
    wsReconnectDelay: 1000,
    wsReconnectTimer: null,
    devices: [],
    status: 'unavailable',
    activeDevice: null,
    scanning: false,
    settingsOpen: false,
    // AVRCP track info
    currentTrack: null,      // { title, artist, album, duration_ms } | null
    playbackStatus: 'unknown', // 'playing' | 'paused' | 'stopped' | 'unknown'
    isStreaming: false,
    theme: 'dark',           // 'dark' | 'light' | 'system'
    streamQuality: 'mp3',    // 'mp3' | 'aac' | 'wav'
    lowLatency: false,
  };

  // ── Spectrum analyser ──────────────────────────────────────────────────────

  const spectrum = (() => {
    const NUM_BANDS = 64;
    let canvas = null;
    let ctx = null;
    let animId = null;

    // Current band magnitudes (0.0 – 1.0)
    let bands = new Float32Array(NUM_BANDS);
    // Peak-hold values
    let peaks = new Float32Array(NUM_BANDS);
    // Frames remaining before peak starts decaying
    let peakHold = new Int32Array(NUM_BANDS);

    // Gentle idle decay when not streaming
    let idleDecayTimer = null;

    function init() {
      canvas = document.getElementById('spectrum-canvas');
      if (!canvas) return;
      ctx = canvas.getContext('2d');
      render(); // kick off animation loop
    }

    function update(newBands) {
      if (!newBands || !newBands.length) return;
      for (let i = 0; i < Math.min(newBands.length, NUM_BANDS); i++) {
        bands[i] = newBands[i];
      }
      // Cancel idle decay while receiving live data
      if (idleDecayTimer) {
        clearInterval(idleDecayTimer);
        idleDecayTimer = null;
      }
    }

    function startIdleDecay() {
      // Slowly decay bands to zero when stream stops
      if (idleDecayTimer) return;
      idleDecayTimer = setInterval(() => {
        let anyNonZero = false;
        for (let i = 0; i < NUM_BANDS; i++) {
          bands[i] *= 0.85;
          peaks[i] *= 0.92;
          if (bands[i] > 0.001) anyNonZero = true;
        }
        if (!anyNonZero) {
          clearInterval(idleDecayTimer);
          idleDecayTimer = null;
        }
      }, 50);
    }

    function render() {
      animId = requestAnimationFrame(render);
      if (!canvas || !ctx) return;

      // Resize canvas to fill its CSS container (HiDPI aware)
      const wrap = canvas.parentElement;
      if (!wrap) return;
      const rect = wrap.getBoundingClientRect();
      const dpr = Math.min(window.devicePixelRatio || 1, 2); // cap at 2× for perf
      const cssW = Math.floor(rect.width);
      const cssH = Math.floor(rect.height);
      const physW = Math.round(cssW * dpr);
      const physH = Math.round(cssH * dpr);

      if (canvas.width !== physW || canvas.height !== physH) {
        canvas.width = physW;
        canvas.height = physH;
        canvas.style.width = cssW + 'px';
        canvas.style.height = cssH + 'px';
        ctx.scale(dpr, dpr);
      }

      const W = cssW;
      const H = cssH;

      // Background
      ctx.clearRect(0, 0, W, H);

      // --- dB grid lines ---
      const dbLevels = [0.0, 0.25, 0.5, 0.75, 1.0]; // mapped to -80..-20..0 dB
      ctx.save();
      ctx.strokeStyle = 'rgba(255,255,255,0.05)';
      ctx.lineWidth = 1;
      dbLevels.forEach(level => {
        const y = Math.round(H * (1.0 - level));
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(W, y);
        ctx.stroke();
      });
      ctx.restore();

      // --- Frequency bars ---
      const n = bands.length;
      const totalGap = n - 1;
      const barW = Math.max(1, (W - totalGap) / n);
      const gap = (W - barW * n) / Math.max(1, n - 1);

      for (let i = 0; i < n; i++) {
        const amp = Math.max(0.0, Math.min(1.0, bands[i]));
        const barH = amp * H;
        const x = i * (barW + gap);
        const y = H - barH;

        // Colour: teal (#1db8c0) → green (#30d158) → orange (#ff8c42) → pink (#d946a8)
        // Mapped left-to-right across frequency
        const t = i / (n - 1);
        let r, g, b;
        if (t < 0.33) {
          // teal → green
          const u = t / 0.33;
          r = Math.round(29  + u * (48  - 29));
          g = Math.round(184 + u * (209 - 184));
          b = Math.round(192 + u * (88  - 192));
        } else if (t < 0.66) {
          // green → orange
          const u = (t - 0.33) / 0.33;
          r = Math.round(48  + u * (255 - 48));
          g = Math.round(209 + u * (140 - 209));
          b = Math.round(88  + u * (66  - 88));
        } else {
          // orange → pink
          const u = (t - 0.66) / 0.34;
          r = Math.round(255 + u * (217 - 255));
          g = Math.round(140 + u * (70  - 140));
          b = Math.round(66  + u * (168 - 66));
        }

        // Minimum glow for non-zero bands
        const baseAlpha = 0.15;
        const alpha = baseAlpha + amp * (1.0 - baseAlpha);

        // Vertical gradient — brighter at top of bar
        if (barH > 1) {
          const grad = ctx.createLinearGradient(0, y, 0, H);
          grad.addColorStop(0, `rgba(${r},${g},${b},${Math.min(1, alpha + 0.25)})`);
          grad.addColorStop(0.7, `rgba(${r},${g},${b},${alpha})`);
          grad.addColorStop(1, `rgba(${r},${g},${b},${alpha * 0.4})`);
          ctx.fillStyle = grad;
        } else {
          ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`;
        }
        ctx.fillRect(x, y, barW, barH);

        // --- Peak hold ---
        if (amp >= peaks[i]) {
          peaks[i] = amp;
          peakHold[i] = 48; // hold for ~48 frames (≈0.8s at 60fps)
        } else if (peakHold[i] > 0) {
          peakHold[i]--;
        } else {
          peaks[i] = Math.max(0, peaks[i] - 0.006);
        }

        if (peaks[i] > 0.015) {
          const py = H - peaks[i] * H;
          ctx.fillStyle = `rgba(255,255,255,0.5)`;
          ctx.fillRect(x, py - 1, barW, 2);
        }
      }

      // --- Idle overlay ---
      const idleEl = document.getElementById('spectrum-idle');
      if (idleEl) {
        const hasSignal = bands.some(v => v > 0.015);
        idleEl.style.display = hasSignal ? 'none' : 'flex';
      }
    }

    return { init, update, startIdleDecay };
  })();

  // ── Initialisation ─────────────────────────────────────────────────────────

  function init() {
    initTheme();
    initLowLatency();
    spectrum.init();
    connectWebSocket();
    fetchStreamQualities();

    document.addEventListener('keydown', (e) => {
      if (e.target.tagName === 'INPUT' || e.target.tagName === 'SELECT') return;
      if (e.key === 's' || e.key === 'S') toggleScan();
      if (e.key === 'Escape') closeSettings();
    });
  }

  function initLowLatency() {
    const saved = localStorage.getItem('soundsync-low-latency');
    if (saved === '1') {
      state.lowLatency = true;
      const toggle = document.getElementById('latency-toggle');
      if (toggle) toggle.checked = true;
    }
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

    state.ws.onerror = () => { /* triggers onclose */ };

    state.ws.onclose = () => {
      updateStatusPill('unavailable', 'Reconnecting…');
      state.wsReconnectTimer = setTimeout(() => {
        state.wsReconnectDelay = Math.min(state.wsReconnectDelay * 2, 30_000);
        connectWebSocket();
      }, state.wsReconnectDelay);
    };
  }

  // ── Server event handler ───────────────────────────────────────────────────

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
        spectrum.startIdleDecay();
        break;

      case 'spectrum_data':
        spectrum.update(event.data.bands);
        updateSpectrumStatus(true);
        break;

      case 'track_changed':
        updateTrackInfo(event.data.track);
        break;

      case 'playback_status_changed':
        updatePlaybackStatus(event.data.status);
        break;

      case 'error':
        showToast(event.data.message, 'error');
        break;

      case 'service_stopping':
        updateStatusPill('unavailable', 'Service restarting…');
        showToast('SoundSync is restarting — reconnecting shortly', 'info');
        break;

      default:
        break;
    }
  }

  function applySnapshot(data) {
    updateBluetoothStatus(data.status);

    if (data.devices) {
      state.devices = data.devices;
      renderDeviceList();
    }

    if (data.active_device) {
      state.activeDevice = data.active_device;
    }

    if (data.track_info !== undefined) {
      updateTrackInfo(data.track_info);
    }

    if (data.playback_status) {
      updatePlaybackStatus(data.playback_status);
    }

    updatePlaybackPanel();
  }

  // ── Stream quality ─────────────────────────────────────────────────────────

  // Fetch available qualities from the server and sync the selector to the
  // currently configured quality.  Also updates the codec stat label.
  async function fetchStreamQualities() {
    try {
      const data = await apiFetch('/api/stream/qualities');
      if (data.current) {
        state.streamQuality = data.current;
        const sel = document.getElementById('quality-select');
        if (sel) sel.value = data.current;
      }
      // Refresh the codec stat label with the resolved quality info.
      fetchStreamInfo();
    } catch (_) {}
  }

  async function fetchStreamInfo() {
    try {
      const data = await apiFetch('/api/stream/info');
      const el = document.getElementById('stat-codec');
      if (el && data.label) el.textContent = `A2DP → ${data.label}`;
    } catch (_) {}
  }

  // Change the stream quality, persist it to the server, and reconnect any
  // active browser audio session so the change takes effect immediately.
  async function setStreamQuality(quality) {
    try {
      await apiFetch('/api/stream/quality', {
        method: 'POST',
        body: JSON.stringify({ quality }),
      });
      state.streamQuality = quality;
      fetchStreamInfo();

      // If the browser is currently playing, restart with the new quality.
      const audio = getAudioPlayer();
      if (audio && !audio.paused) {
        const stop = () => {
          audio.pause();
          audio.src = '';
          // Small delay so the old stream closes before the new one opens.
          setTimeout(() => toggleBrowserAudio(), 120);
        };
        if (_playPromise) {
          _playPromise.then(stop).catch(() => {});
          _playPromise = null;
        } else {
          stop();
        }
      }
    } catch (_) {}
  }

  // ── API helpers ────────────────────────────────────────────────────────────

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
      state.scanning = !state.scanning;
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

  // ── Browser audio player ───────────────────────────────────────────────────

  // Track the pending play() promise so we can safely call pause() after it
  // resolves — avoids Chrome's "play() interrupted by pause()" DOMException.
  let _playPromise = null;

  function getAudioPlayer() {
    return document.getElementById('audio-player');
  }

  // ── Latency measurement state ─────────────────────────────────────────────

  let _playStartTime = 0;   // Date.now() when play() is called
  let _bufferPollId = null;  // requestAnimationFrame id for buffer depth polling
  let _onPlaying = null;     // 'playing' listener ref so _stopBufferPoll can remove it

  function setLowLatency(enabled) {
    state.lowLatency = enabled;
    localStorage.setItem('soundsync-low-latency', enabled ? '1' : '0');

    // If currently playing, restart with the new latency setting.
    const audio = getAudioPlayer();
    if (audio && !audio.paused) {
      const stop = () => {
        audio.pause();
        audio.src = '';
        setTimeout(() => toggleBrowserAudio(), 120);
      };
      if (_playPromise) {
        _playPromise.then(stop).catch(() => {});
        _playPromise = null;
      } else {
        stop();
      }
    }
  }

  function _buildStreamUrl() {
    let url = `/audio/stream?quality=${encodeURIComponent(state.streamQuality)}`;
    if (state.lowLatency) url += '&latency=low';
    return url;
  }

  function _startBufferPoll() {
    _stopBufferPoll();
    const audio = getAudioPlayer();
    if (!audio) return;

    const latencyEl = document.getElementById('stat-latency');
    const bufferEl = document.getElementById('stat-buffer');

    // Measure startup latency once, on the first `playing` event.
    _onPlaying = () => {
      if (_playStartTime > 0) {
        const ms = Date.now() - _playStartTime;
        if (latencyEl) {
          latencyEl.textContent = ms + ' ms';
          latencyEl.className = 'stat-value';
        }
        _playStartTime = 0;
      }
      audio.removeEventListener('playing', _onPlaying);
      _onPlaying = null;
    };
    audio.addEventListener('playing', _onPlaying);

    // Poll buffer depth at ~4 Hz via requestAnimationFrame.
    let lastPoll = 0;
    const poll = (ts) => {
      _bufferPollId = requestAnimationFrame(poll);
      if (ts - lastPoll < 250) return;
      lastPoll = ts;
      if (audio.paused || !audio.buffered.length) return;

      const end = audio.buffered.end(audio.buffered.length - 1);
      const depth = Math.max(0, end - audio.currentTime);
      if (bufferEl) {
        bufferEl.textContent = depth.toFixed(1) + ' s';
        bufferEl.className = 'stat-value' + (depth < 0.5 ? ' inactive' : '');
      }
    };
    _bufferPollId = requestAnimationFrame(poll);
  }

  function _stopBufferPoll() {
    if (_bufferPollId) {
      cancelAnimationFrame(_bufferPollId);
      _bufferPollId = null;
    }
    if (_onPlaying) {
      const audio = getAudioPlayer();
      if (audio) audio.removeEventListener('playing', _onPlaying);
      _onPlaying = null;
    }
    const latencyEl = document.getElementById('stat-latency');
    const bufferEl = document.getElementById('stat-buffer');
    if (latencyEl) { latencyEl.textContent = '—'; latencyEl.className = 'stat-value inactive'; }
    if (bufferEl) { bufferEl.textContent = '—'; bufferEl.className = 'stat-value inactive'; }
  }

  function toggleBrowserAudio() {
    const audio = getAudioPlayer();
    const btn   = document.getElementById('btn-listen');
    if (!audio) return;

    if (audio.paused) {
      // Set src fresh each time so the browser opens a new HTTP connection.
      // Do NOT call audio.load() — that aborts the pending play() promise.
      audio.src = _buildStreamUrl();
      _playStartTime = Date.now();
      _playPromise = audio.play();
      if (_playPromise) {
        _playPromise.then(() => {
          _playPromise = null;
          if (btn) btn.textContent = 'Stop';
          _startBufferPoll();
        }).catch(err => {
          _playPromise = null;
          _playStartTime = 0;
          // AbortError fires when Stop is clicked before buffering finishes — ignore.
          if (err.name !== 'AbortError') {
            showToast('Could not start audio: ' + err.message, 'error');
          }
          if (btn) btn.textContent = 'Listen';
        });
      }
    } else {
      const stop = () => {
        audio.pause();
        // Detach src to close the HTTP connection immediately
        audio.src = '';
        if (btn) btn.textContent = 'Listen';
        _stopBufferPoll();
      };
      // If play() is still pending, wait for it to resolve before pausing.
      // Calling pause() on a pending play() throws in Chrome.
      if (_playPromise) {
        _playPromise.then(stop).catch(() => {});
        _playPromise = null;
      } else {
        stop();
      }
    }
  }

  // ── Volume ─────────────────────────────────────────────────────────────────

  function setVolume(value) {
    const icons = document.querySelectorAll('.volume-icon');
    if (icons[0]) {
      icons[0].textContent = value < 10 ? '🔇' : value < 50 ? '🔈' : '🔉';
    }
    const audio = getAudioPlayer();
    if (audio) audio.volume = Number(value) / 100;
  }

  // ── Theme ──────────────────────────────────────────────────────────────────

  function setTheme(theme) {
    state.theme = theme;
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('soundsync-theme', theme);
    // Update active button
    document.querySelectorAll('.theme-opt').forEach(btn => {
      btn.classList.toggle('active', btn.dataset.for === theme);
    });
  }

  function initTheme() {
    const saved = localStorage.getItem('soundsync-theme') || 'dark';
    setTheme(saved);
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

    const app = document.getElementById('app');
    if (app) {
      app.style.gridTemplateRows = state.settingsOpen
        ? '72px 1fr 320px'
        : '72px 1fr 48px';
    }

    if (state.settingsOpen) {
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
    showToast('Status refreshed', 'info');
  }

  // ── UI rendering: device list ──────────────────────────────────────────────

  function renderDeviceList() {
    const list = document.getElementById('device-list');
    const noDevices = document.getElementById('no-devices');
    if (!list) return;

    const devices = state.devices;

    if (!devices.length) {
      if (noDevices) noDevices.style.display = 'flex';
      list.querySelectorAll('.device-card').forEach(el => el.remove());
      return;
    }

    if (noDevices) noDevices.style.display = 'none';

    const existingCards = {};
    list.querySelectorAll('.device-card').forEach(el => {
      existingCards[el.dataset.address] = el;
    });

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

  function formatDeviceState(s) {
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
    return labels[s] || s;
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

  // ── UI rendering: spectrum status badge ───────────────────────────────────

  function updateSpectrumStatus(active) {
    const badge = document.getElementById('spectrum-status');
    const text = document.getElementById('spectrum-status-text');
    if (badge) badge.setAttribute('data-active', active ? 'true' : 'false');
    if (text) text.textContent = active ? 'Live' : 'No Signal';
  }

  // ── UI rendering: playback panel ──────────────────────────────────────────

  function updatePlaybackPanel() {
    const active = state.devices.find(d => isDeviceConnected(d.state));
    const streaming = state.devices.find(d => d.state === 'audio_active');

    const deviceEl = document.getElementById('playback-device');
    const streamEl = document.getElementById('stat-stream');

    if (streaming) {
      if (deviceEl) deviceEl.textContent = streaming.name;
      if (streamEl) { streamEl.textContent = 'Active'; streamEl.className = 'stat-value active'; }
      state.activeDevice = streaming.address;
      state.isStreaming = true;
    } else if (active) {
      if (deviceEl) deviceEl.textContent = active.name;
      if (streamEl) { streamEl.textContent = 'Connected'; streamEl.className = 'stat-value'; }
      state.isStreaming = false;
    } else {
      if (deviceEl) deviceEl.textContent = '—';
      if (streamEl) { streamEl.textContent = 'Idle'; streamEl.className = 'stat-value inactive'; }
      state.isStreaming = false;
      updateSpectrumStatus(false);
    }

    // If no AVRCP track info available, show device name as fallback
    if (!state.currentTrack) {
      const titleEl = document.getElementById('track-title');
      const artistEl = document.getElementById('track-artist-album');
      if (titleEl) titleEl.textContent = streaming ? streaming.name : (active ? active.name : '—');
      if (artistEl) artistEl.textContent = streaming ? 'Streaming to browser' : '—';
    }
  }

  function updateTrackInfo(track) {
    state.currentTrack = track;

    const titleEl = document.getElementById('track-title');
    const artistEl = document.getElementById('track-artist-album');

    if (track) {
      if (titleEl) titleEl.textContent = track.title || '(Unknown Track)';
      const parts = [track.artist, track.album].filter(Boolean);
      if (artistEl) artistEl.textContent = parts.length ? parts.join(' — ') : '—';
    } else {
      // Fall back to device name
      const streaming = state.devices.find(d => d.state === 'audio_active');
      const active = state.devices.find(d => isDeviceConnected(d.state));
      if (titleEl) titleEl.textContent = streaming ? streaming.name : (active ? active.name : '—');
      if (artistEl) artistEl.textContent = streaming ? 'Streaming to browser' : '—';
    }
  }

  function updatePlaybackStatus(status) {
    state.playbackStatus = status;

    const badge = document.getElementById('playback-badge');
    const icon = document.querySelector('#playback-badge .pb-icon');
    const text = document.getElementById('pb-status-text');

    if (badge) badge.setAttribute('data-status', status);

    const statusMap = {
      playing: { icon: '▶', label: 'Playing', cls: 'playing' },
      paused:  { icon: '❙❙', label: 'Paused',  cls: 'paused'  },
      stopped: { icon: '■', label: 'Stopped', cls: 'stopped' },
      unknown: { icon: '■', label: 'No stream', cls: 'unknown' },
    };
    const info = statusMap[status] || statusMap.unknown;
    if (icon) icon.textContent = info.icon;
    if (text) text.textContent = info.label;
    if (badge) {
      badge.className = `playback-badge status-${info.cls}`;
      badge.setAttribute('data-status', status);
    }
  }

  function updateStreamStatus(active, address) {
    const streamEl = document.getElementById('stat-stream');

    if (active) {
      if (streamEl) { streamEl.textContent = 'Active'; streamEl.className = 'stat-value active'; }
      state.activeDevice = address;
      state.isStreaming = true;

      const device = state.devices.find(d =>
        d.address === address || address === 'pipewire_detected'
      );
      if (device) device.state = 'audio_active';
    } else {
      if (streamEl) { streamEl.textContent = 'Idle'; streamEl.className = 'stat-value inactive'; }
      state.isStreaming = false;
      updateSpectrumStatus(false);
    }

    renderDeviceList();
    updatePlaybackPanel();
  }

  function updateBluetoothStatus(status) {
    state.status = status;
    state.scanning = status === 'scanning';

    let display, pillStatus;
    if (status === 'scanning') {
      display = 'Scanning…';
      pillStatus = 'scanning';
    } else if (status === 'ready') {
      display = 'Ready';
      pillStatus = 'ready';
    } else if (status && status.startsWith('error')) {
      display = 'Error';
      pillStatus = 'unavailable';
    } else {
      display = 'Unavailable';
      pillStatus = 'unavailable';
    }

    const hasStream = state.devices.some(d => d.state === 'audio_active');
    if (hasStream) { display = 'Streaming'; pillStatus = 'streaming'; }
    else if (state.devices.some(d => isDeviceConnected(d.state))) {
      display = 'Connected'; pillStatus = 'connected';
    }

    updateStatusPill(pillStatus, display);

    const scanBtn = document.getElementById('btn-scan');
    if (scanBtn) scanBtn.classList.toggle('active', state.scanning);

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

    setTimeout(() => {
      toast.style.opacity = '0';
      toast.style.transform = 'translateX(20px)';
      toast.style.transition = 'opacity 200ms, transform 200ms';
      setTimeout(() => toast.remove(), 220);
    }, 3000);
  }

  // ── Utilities ──────────────────────────────────────────────────────────────

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

  // ── Bootstrap ──────────────────────────────────────────────────────────────

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }

  // ── Public API (called from HTML inline handlers) ──────────────────────────

  return {
    toggleScan,
    connectDevice,
    disconnectDevice,
    removeDevice,
    setVolume,
    toggleBrowserAudio,
    setTheme,
    toggleSettings,
    applyName,
    refresh,
    setStreamQuality,
    setLowLatency,
    get state() { return state; },
  };

})();
