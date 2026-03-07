# SoundSync Latency Budget

Target total end-to-end audio latency: **< 150ms**

## Pipeline Stage Breakdown

| Stage | Budget | Notes |
|-------|--------|-------|
| Bluetooth A2DP SBC encoding (source device) | ~30ms | Fixed — source device controlled |
| BlueZ/PipeWire receive buffer | ~20ms | Tunable via quantum settings |
| PipeWire graph processing | ~5ms | Buffer schedule |
| SoundSync DSP EQ filter chain | < 2ms | 10 biquad filters in float32 |
| PipeWire output buffer | ~20ms | Tunable |
| ALSA hardware buffer | ~10ms | Driver dependent |
| **Total (estimated)** | **~87ms** | Well within 150ms target |

## Tuning Parameters

If latency exceeds 150ms, reduce PipeWire quantum:

```bash
# Set quantum to 256 samples at 48kHz = ~5.3ms
pw-metadata -n settings 0 clock.force-quantum 256
```

### PipeWire Buffer Configuration

`~/.config/pipewire/pipewire.conf.d/soundsync-latency.conf`:

```json
context.properties = {
    default.clock.rate          = 48000
    default.clock.quantum       = 512
    default.clock.min-quantum   = 256
    default.clock.max-quantum   = 1024
}
```

## DSP Processing Budget

The 10-band biquad EQ processes each sample through 10 Direct-Form-II
transposed filters. Per quantum (512 samples stereo = 1024 samples):

- 10 filters × 1024 samples × ~3 multiply-add ops = ~30,720 FLOPS
- At 48kHz with 512 quantum: 10.67ms budget per quantum
- EQ processing: < 0.1ms per quantum on modern hardware
- **DSP adds negligible latency**

## Measurement

To measure actual latency, use:

```bash
# Install JACK latency test tools
sudo apt-get install -y jackd2

# Test PipeWire round-trip latency
pw-jack jack_iodelay
```

Expected output should show < 20ms hardware round-trip on modern systems.
