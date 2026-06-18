# Niri Compositor — Codebase Index

Version: 26.4.0 · License: GPL-3.0-or-later

---

## Top-level layout

```
niri/
├── src/                  Main compositor source
│   ├── backend/          TTY/DRM, Winit, Headless backends
│   ├── handlers/         Wayland protocol event handlers
│   ├── input/            Input event processing & grabs
│   ├── layout/           Tiling/floating window layout engine
│   ├── protocols/        Wayland protocol implementations
│   ├── render_helpers/   GLES rendering utilities & shaders
│   ├── ui/               Internal UI (alt-tab switcher, overlays)
│   ├── utils/            Misc helpers
│   ├── animation/        Bezier/spring animation engine
│   ├── dbus/             D-Bus service integrations
│   ├── screencasting/    Pipewire screencasting
│   ├── ipc/              IPC server and socket
│   └── layer/            Layer-shell surface management
├── niri-config/          Config parsing crate (KDL)
│   └── src/
│       ├── lib.rs        Config struct + include handling
│       ├── output.rs     Output struct (scale, transform, mode, max_bpc, icc_profile, …)
│       ├── binds.rs      Keybinding definitions
│       ├── appearance.rs Color/border/blur/shadow settings
│       └── …
├── niri-ipc/             IPC types crate
├── niri-visual-tests/    Visual regression tests
├── resources/            Desktop files, default-config.kdl, icons
└── Cargo.toml            Workspace manifest (version 26.4.0)
```

---

## src/ — file-by-file

| File | Purpose |
|------|---------|
| `niri.rs` | **Main compositor struct** (`Niri`, `State`, `OutputState`). Render dispatch, window management, output map. |
| `main.rs` | Entry point: CLI, config load, event-loop setup, dbus init. |
| `lib.rs` | Crate root, module declarations. |
| `icc.rs` | *(added)* ICC v2/v4 profile parser → DRM CTM + DEGAMMA/GAMMA LUT blobs. |

### backend/

| File | Purpose |
|------|---------|
| `tty.rs` | **TTY/DRM/KMS backend.** `Tty`, `OutputDevice`, `Surface`, `GammaProps`, `ConnectorProperties`. GPU manager, gamma, HDR, CTM, ICC application. |
| `winit.rs` | Windowed backend (testing). |
| `headless.rs` | Headless backend (no display, testing/screencasting). |
| `mod.rs` | `Backend` enum: Tty / Winit / Headless. |

### render_helpers/

| File | Purpose |
|------|---------|
| `mod.rs` | `RenderCtx`, `RenderTarget`. Rendering utilities. |
| `shader_element.rs` | Custom GLSL shader render elements. |
| `blur.rs` | Gaussian blur. |
| `border.rs` | Window borders with gradients. |
| `shadow.rs` | Drop shadows. |
| `solid_color.rs` | Solid color buffers. |
| `texture.rs` | Texture handling. |
| `surface.rs` | Wayland surface → texture. |
| `offscreen.rs` | Offscreen framebuffer rendering. |
| `damage.rs` | Damage region tracking. |
| `renderer.rs` | `NiriRenderer` trait. |
| `shaders/` | GLSL shaders; `colorspace` uniform present. |

### protocols/

| File | Protocol |
|------|---------|
| `gamma_control.rs` | `zwlr-gamma-control-v1` — gamma ramp via client. |
| `output_management.rs` | `zwlr-output-management-v1` — output configuration. |
| `screencopy.rs` | Screen capture. |
| `foreign_toplevel.rs` | Window list for taskbars. |
| `ext_workspace.rs` | Dynamic workspace protocol. |
| `virtual_pointer.rs` | Virtual pointer. |
| `mutter_x11_interop.rs` | Mutter X11 interop. |

---

## Key structs

### `OutputState` — `src/niri.rs:~449`
Per-output compositor state (frame clock, redraw state, lock surfaces, backdrop buffer).

### `Niri` — `src/niri.rs:~195`
Main struct: config, layout, output map, seat, cursor, etc.

### `Surface` — `src/backend/tty.rs:377`
```
Surface {
    name, compositor, connector,
    gamma_props: Option<GammaProps>,   // CRTC gamma/CTM/degamma blobs
    pending_gamma_change,              // deferred while session inactive
    …
}
```

### `GammaProps` — `src/backend/tty.rs:401`
```
GammaProps {
    crtc,
    gamma_lut,       // property::Handle for GAMMA_LUT blob
    gamma_lut_size,  // property::Handle for GAMMA_LUT_SIZE
    previous_blob,   // NonZeroU64 — old blob to destroy after update
    // Added for ICC:
    ctm,             Option<property::Handle>  — CTM 3×3 matrix blob
    degamma_lut,     Option<property::Handle>  — DEGAMMA_LUT blob
    degamma_lut_size,Option<property::Handle>
    previous_ctm_blob,
    previous_degamma_blob,
}
```

### `ConnectorProperties` — `src/backend/tty.rs:408`
```
ConnectorProperties<'a> {
    device, connector,
    properties: Vec<(property::Info, property::RawValue)>,
    has_change: bool,
    requests: AtomicModeReq,
}
```
Used to atomically set connector-level properties (max_bpc, HDR_OUTPUT_METADATA, Colorspace).

### `OutputDevice` — `src/backend/tty.rs:134`
```
OutputDevice {
    drm: DrmDevice,
    gbm, allocator,
    surfaces: HashMap<crtc::Handle, Surface>,
    known_crtcs, drm_scanner, …
}
```

### `Output` (config) — `niri-config/src/output.rs:51`
```
Output {
    name, off, scale, transform, position,
    max_bpc, mode, modeline, variable_refresh_rate,
    focus_at_startup, backdrop_color, hot_corners, layout,
    icc_profile: Option<PathBuf>,   // added
}
```

---

## Key functions — rendering & display

| Function | Location |
|----------|---------|
| `Tty::render()` | `tty.rs:~1849` — main TTY render, DRM commit |
| `Niri::render_to_vec()` | `niri.rs:~4162` — assemble render elements |
| `Niri::render()` | `niri.rs:~4175` — render elements to frame |
| `GammaProps::new()` | `tty.rs:2611` — enumerate CRTC props (GAMMA_LUT, now also CTM, DEGAMMA_LUT) |
| `GammaProps::gamma_size()` | `tty.rs:2657` |
| `GammaProps::set_gamma()` | `tty.rs:2663` — set GAMMA_LUT blob |
| `GammaProps::set_color_transform()` | `tty.rs` *(added)* — set CTM + DEGAMMA_LUT blobs |
| `GammaProps::restore_gamma()` | `tty.rs:2732` |
| `Tty::set_gamma()` | `tty.rs:2091` — public gamma setter (session-aware) |
| `set_gamma_for_crtc()` | `tty.rs:3406` — legacy fallback |
| `ConnectorProperties::reset_hdr()` | `tty.rs:3309` — clear HDR_OUTPUT_METADATA, Colorspace |
| `ConnectorProperties::set_max_bpc()` | `tty.rs:3339` |
| `ConnectorProperties::commit()` | `tty.rs:3367` — atomic DRM commit |
| `set_connector_properties()` | `tty.rs:3379` — apply max_bpc + HDR reset |
| `apply_icc_to_surface()` | `tty.rs` *(added)* — load ICC, call icc module, set CTM/LUTs |

---

## DRM property blob pattern

```rust
// 1. Create blob
let blob = drm_ffi::mode::create_property_blob(device.as_fd(), bytemuck_cast_slice(&data))?;
let blob_id = NonZeroU64::new(u64::from(blob.blob_id));

// 2. Set on CRTC
device.set_property(crtc, prop_handle, property::Value::Blob(blob_id.unwrap_or(0)).into())?;

// 3. Destroy old blob
if let Some(old) = mem::replace(&mut self.previous_blob, blob_id) {
    let _ = device.destroy_property_blob(old.get());
}
```

For connector properties (HDR_OUTPUT_METADATA, max_bpc, Colorspace), changes are queued in
`AtomicModeReq` and committed once via `ConnectorProperties::commit()`.

---

## Color management additions (ICC → DRM)

**Pipeline (DRM atomic KMS):**
```
[scanout buffer]
      │
  DEGAMMA_LUT   (linearize: remove display TRC / assume sRGB input)
      │
    CTM          (3×3 S31.32 fixed-point matrix: sRGB→XYZ→display primaries)
      │
  GAMMA_LUT     (apply display TRC / re-encode to display transfer function)
      │
   [panel]
```

**DRM structs used:**
```c
// drm_color_lut  (DEGAMMA_LUT, GAMMA_LUT entries)
struct drm_color_lut { u16 red, green, blue, reserved; }

// drm_color_ctm  (CTM — 3×3 matrix of S31.32 values)
struct drm_color_ctm { u64 matrix[9]; }
// S31.32: bit63=sign, bits62:32=integer, bits31:0=fraction
// positive x → (x * (1u64 << 32))
// negative x → (1u64 << 63) | (|x| * (1u64 << 32))
```

**ICC parsing (src/icc.rs):**
- Tags read: `rXYZ`, `gXYZ`, `bXYZ` (primaries in D50 XYZ), `wtpt` (white point)
- Tags read: `rTRC`, `gTRC`, `bTRC` (transfer curves: parametric or LUT)
- Matrix path: build [rXYZ|gXYZ|bXYZ] 3×3 in D50 XYZ, then
  M_ctm = M_display^-1 × M_sRGB_D50  (sRGB→XYZ→display)
- LUTs: DEGAMMA = inverse-sRGB EOTF (linear), GAMMA = display forward TRC

---

## Config (KDL) example

```kdl
output "eDP-1" {
    icc-profile "/path/to/display.icc"
}
```

---

## Cargo deps relevant to ICC

```toml
bytemuck = "1.25"        # pod casting for drm_color_lut / drm_color_ctm
drm-ffi = "0.9.1"        # create_property_blob
# No new deps needed — pure Rust math, binary ICC parsing by hand
```
