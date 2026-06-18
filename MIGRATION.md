# ICC Color Management — Rebase Guide

This document describes the local changes on top of upstream niri to implement
DRM KMS-based ICC color management.  Use it when rebasing onto a new upstream
commit.

---

## Overview of the feature

Parse a binary ICC profile and apply it to the DRM CRTC via three properties:

```
[sRGB compositor output]
  → DEGAMMA_LUT  (sRGB EOTF: remove display gamma → linear light)
  → CTM          (3×3 matrix: sRGB primaries → display primaries, linear light)
  → GAMMA_LUT    (display TRC: re-encode for the panel)
  → [panel]
```

A window rule `icc-passthrough true` marks apps that already output in the
display's native color space (mpv, Krita, Firefox with color management).
For those windows, the compositor applies the inverse CTM on the GPU so the
net effect of GPU × DRM = identity.

---

## Changed files

### New files

| File | Purpose |
|------|---------|
| `src/icc.rs` | ICC v2/v4 parser + DRM blob math (`IccTransforms`, `load_icc()`) |
| `src/render_helpers/shaders/icc_passthrough.frag` | GLSL ES 1.00 shader: unpremultiply → EOTF → `icc_ctm_inverse` mat3 → OETF → re-premultiply |
| `src/render_helpers/icc_passthrough_element.rs` | `IccPassthroughRenderElement<R>` wrapping `WaylandSurfaceRenderElement` |

> ⚠️ Matrix layout gotcha: `IccTransforms::ctm_inverse` (in `src/icc.rs`) is a
> **row-major** `[f32; 9]`.  OpenGL uniform upload via `UniformValue::Matrix3x3`
> is column-major, so the uniform **must** be passed with `transpose: true` and
> the raw array — do *not* funnel it through `glam::Mat3::from_cols_array`,
> which silently transposes the matrix (the resulting shader output looks like
> "white turns pink, orange turns red").

---

### `niri-config/src/output.rs`

Added `icc_profile: Option<PathBuf>` field to `Output`:

```rust
#[knuffel(child, unwrap(argument))]
pub icc_profile: Option<PathBuf>,
```

Also add `use std::path::PathBuf;` and `icc_profile: None` in `Default`.

**Conflict risk**: low — appended after existing fields.


### `niri-config/src/window_rule.rs`

Added `icc_passthrough: Option<bool>` field to `WindowRule`:

```rust
/// When the output has an icc-profile set, skip the compositor ICC correction for this window.
/// Use for apps that handle color management themselves (mpv, Krita, Firefox, …).
#[knuffel(child, unwrap(argument))]
pub icc_passthrough: Option<bool>,
```

**Conflict risk**: low — appended after existing fields.


### `src/lib.rs`

Added `pub mod icc;` to the module list (alphabetical order, after `handlers`).

**Conflict risk**: very low.


### `src/window/mod.rs`

1. Added `icc_passthrough: Option<bool>` field to `ResolvedWindowRules` (with doc comment).
2. Added resolution logic in `ResolvedWindowRules::resolve()` (or wherever the
   last-wins merge loop is):

```rust
if let Some(x) = rule.icc_passthrough {
    resolved.icc_passthrough = Some(x);
}
```

**Conflict risk**: medium — `ResolvedWindowRules` gains new fields regularly
upstream.  If there is a conflict in the struct definition, add the field after
the last existing field.  If there is a conflict in the merge loop, add the
`if let Some(x)` block after the last existing `if let Some`.


### `src/render_helpers/mod.rs`

1. Added `pub mod icc_passthrough_element;` to the module list.
2. Added `icc_ctm_inverse: Option<[f32; 9]>` field to `RenderCtx`:

```rust
/// When the output has an ICC profile with a DRM CTM, this holds the inverse of that CTM
/// (display→sRGB). Used by the ICC passthrough shader to counteract the hardware CTM for
/// windows that manage their own color (mpv, Krita, Firefox, …).
///
/// `None` when no ICC profile is active, or when rendering to a screencast/screenshot target
/// (where the DRM CTM does not apply).
pub icc_ctm_inverse: Option<[f32; 9]>,
```

3. Propagated the field through both `RenderCtx` reborrow methods (the two
   `impl` functions that return a new `RenderCtx` from `&mut self`).

**Conflict risk**: medium — `RenderCtx` is touched occasionally upstream.
The field must be copied in all reborrow methods.


### `src/render_helpers/shaders/mod.rs`

1. Added `pub icc_passthrough: Option<GlesTexProgram>` field to `Shaders`.
2. In `Shaders::compile()`, added compilation:

```rust
let icc_passthrough = renderer
    .compile_custom_texture_shader(
        include_str!("icc_passthrough.frag"),
        &[UniformName::new("icc_ctm_inverse", UniformType::Matrix3x3)],
    )
    .map_err(|err| {
        warn!("error compiling ICC passthrough shader: {err:?}");
    })
    .ok();
```

3. Added `icc_passthrough` to the `Self { ... }` constructor.

**Conflict risk**: low — new shader field appended after existing ones.


### `src/layout/mod.rs`

1. Added import:
   ```rust
   use crate::render_helpers::icc_passthrough_element::IccPassthroughRenderElement;
   ```
2. Added `IccPassthrough = IccPassthroughRenderElement<R>` variant to
   `LayoutElementRenderElement`.

**Conflict risk**: medium — this enum gets new variants occasionally.  Adding
another variant causes no breakage unless upstream code has exhaustive matches
without wildcards (see below).


### `src/layout/tile.rs`

1. Added import of `IccPassthroughRenderElement`.
2. Added `IccPassthrough = IccPassthroughRenderElement<R>` to `TileRenderElement`.
3. Added arm to the exhaustive `clip` closure in `render_inner()`:

```rust
elem @ LayoutElementRenderElement::IccPassthrough(_) => {
    // ICC passthrough elements already have a custom texture shader applied.
    // Combining it with the clipped-surface shader is not currently supported,
    // so pass through without clipping.
    elem.into()
}
```

4. Added `icc_ctm_inverse: None` to the three `RenderCtx { ... }` literals
   inside `Tile::render_snapshot()` (snapshot paths — ICC doesn't apply).

**Conflict risk**: medium — the `clip` closure is refactored periodically.
When rebasing, `cargo check` will tell you if a new arm is needed.


### `src/ui/mru.rs`

Added `IccPassthrough` arm to the `clip` closure in `Thumbnail::render()`:

```rust
elem @ LayoutElementRenderElement::IccPassthrough(_) => {
    // ICC passthrough is only active when icc_ctm_inverse is Some, which is never the
    // case for thumbnail rendering. Pass through without the ICC shader.
    elem.into()
}
```

**Conflict risk**: low — arm is appended after `BackgroundEffect`.


### `src/window/mapped.rs`

1. Added import of `IccPassthroughRenderElement`.
2. Rewrote the `else` branch of `render_normal()` to conditionally wrap
   surface elements in `IccPassthroughRenderElement`:

```rust
let icc_shader = if self.rules.icc_passthrough == Some(true) {
    ctx.icc_ctm_inverse
        .and_then(|inv| IccPassthroughRenderElement::shader(ctx.renderer)
            .map(|s| (s.clone(), inv)))
} else {
    None
};

if let Some((shader, ctm_inverse)) = icc_shader {
    let mut push_wrapped = |elem: WaylandSurfaceRenderElement<R>| {
        push(IccPassthroughRenderElement::new(elem, shader.clone(), ctm_inverse).into())
    };
    push_elements_from_surface_tree(ctx.renderer, surface, ..., &mut push_wrapped);
} else {
    let mut push_plain = |elem: WaylandSurfaceRenderElement<R>| push(elem.into());
    push_elements_from_surface_tree(ctx.renderer, surface, ..., &mut push_plain);
}
```

3. Added `icc_ctm_inverse: None` to the `RenderCtx` inside
   `render_for_screen_cast()`.

**Conflict risk**: high — `render_normal()` / `render_for_screen_cast()` are
actively maintained.  When rebasing, check the function body carefully.  The
wrapping logic must surround the `push_elements_from_surface_tree` call.


### `src/niri.rs`

1. Changed import: `use crate::backend::tty::{IccCtmInverse, SurfaceDmabufFeedback};`
2. Added field to `OutputState`:

```rust
/// Inverse of the ICC CTM applied via DRM for this output (display→sRGB, linear light).
/// `Some` when an ICC profile is active and a CTM has been applied to the DRM CRTC.
/// Used by the ICC passthrough shader to counteract the hardware CTM for windows that
/// perform their own color management (e.g. mpv, Krita, Firefox).
/// `None` on the winit backend or when no ICC profile is configured.
pub icc_ctm_inverse: Option<[f32; 9]>,
```

3. In `Niri::add_output()`, populate the field from `Output` user data:

```rust
let icc_ctm_inverse = output.user_data().get::<IccCtmInverse>().map(|d| d.0);
let state = OutputState {
    // ... existing fields ...
    icc_ctm_inverse,
};
```

4. Added `icc_ctm_inverse: None` to **all** other `RenderCtx { ... }` literals
   (screen capture, screenshot, screencopy, pick-colour-grab, winit — anywhere
   the DRM CTM is not active).  Run `cargo check` and fix each E[missing field]
   error.

**Conflict risk**: high — `OutputState` and `add_output()` are frequently
touched.  When there's a struct conflict, add the field after
`debug_damage_tracker`.  When there's a conflict in `add_output()`, add the
`get::<IccCtmInverse>()` call just before the `OutputState { ... }` literal.


### `src/backend/tty.rs`

This is the most complex file.  The diff touches several disjoint areas:

#### 1. `IccCtmInverse` newtype (near `TtyOutputState`)

```rust
/// Output user data: inverse of the ICC CTM (display→sRGB, linear light).
///
/// Stored on the `Output` when an ICC profile with a valid CTM is applied to the CRTC.
/// Read by `niri.rs::add_output()` to populate `OutputState::icc_ctm_inverse`.
#[derive(Debug, Clone, Copy)]
pub struct IccCtmInverse(pub [f32; 9]);
```

#### 2. Extended `GammaProps` struct

Added after `previous_blob`:

```rust
/// Optional CTM (Color Transform Matrix) property handle on the CRTC.
ctm: Option<property::Handle>,
previous_ctm_blob: Option<NonZeroU64>,
/// Optional DEGAMMA_LUT property handle on the CRTC.
degamma_lut: Option<property::Handle>,
degamma_lut_size: Option<property::Handle>,
previous_degamma_blob: Option<NonZeroU64>,
```

#### 3. `GammaProps::new()` — discover CTM / DEGAMMA_LUT properties

In the property-name `match`, after the `"GAMMA_LUT_SIZE"` arm:

```rust
"CTM" => {
    if matches!(info.value_type(), property::ValueType::Blob) {
        ctm = Some(prop);
    }
}
"DEGAMMA_LUT" => {
    if matches!(info.value_type(), property::ValueType::Blob) {
        degamma_lut = Some(prop);
    }
}
"DEGAMMA_LUT_SIZE" => {
    if matches!(info.value_type(), property::ValueType::UnsignedRange(_, _)) {
        degamma_lut_size = Some(prop);
    }
}
```

Also initialise and return the new fields from the constructor.

#### 4. `GammaProps::set_color_transform()` — new method

Applies / clears DEGAMMA_LUT, CTM and GAMMA_LUT blobs atomically.
See the full implementation in the diff.

#### 5. `apply_icc_to_gamma_props()` — new free function

Loads an ICC file, calls `set_color_transform`, returns
`Option<[f32; 9]>` (the ctm_inverse, or `None` on failure).

#### 6. Output creation in `Tty::add_output()` (or similar)

After the gamma-reset call:

```rust
let icc_ctm_inverse = config
    .icc_profile
    .as_ref()
    .and_then(|icc_path| apply_icc_to_gamma_props(&mut gamma_props, &device.drm, icc_path));
```

After `Output` is created, store the inverse in user data:

```rust
if let Some(inv) = icc_ctm_inverse {
    output.user_data().insert_if_missing(|| IccCtmInverse(inv));
}
```

#### 7. Main render path in `Tty::render_output()` (or similar)

Read the inverse from `OutputState` and pass it to `RenderCtx`:

```rust
let icc_ctm_inverse = niri
    .output_state
    .get(output)
    .and_then(|s| s.icc_ctm_inverse);
let ctx = RenderCtx {
    renderer: &mut renderer,
    target: RenderTarget::Output,
    xray: None,
    icc_ctm_inverse,
};
```

**Conflict risk**: very high — `tty.rs` is the most actively developed file.
The key anchors are:
- The `GammaProps` struct definition (usually stable).
- The property-discovery loop in `GammaProps::new()`.
- The `apply_icc_profile` call site: it must come *after* the gamma-reset and
  *before* surface creation, so that the CRTC properties are set before the
  first frame is committed.
- The `RenderCtx` construction in the main render loop.


### `src/backend/winit.rs`, `src/input/pick_color_grab.rs`, `src/screencasting/mod.rs`

Each has one `RenderCtx { ... }` literal that gains `icc_ctm_inverse: None`.
These are purely mechanical — `cargo check` will identify each missing field.

---

## Rebase checklist

When pulling a new upstream commit:

1. `git rebase upstream/main` (or `git merge upstream/main`).
2. Resolve conflicts using the notes above.
3. `cargo check` — address every `error[E...]`.
   - Missing `icc_ctm_inverse` field in a `RenderCtx` literal → add `icc_ctm_inverse: None`.
   - Non-exhaustive match on `LayoutElementRenderElement` / `TileRenderElement`
     → add the `IccPassthrough` arm manually in the hand-written `clip`
     closures (`tile.rs::render_inner`, `mru.rs::Thumbnail::render`).
     The `niri_render_elements!` macro only generates the enum and `From`
     impls; it does not patch hand-written match arms elsewhere.
4. Verify `src/icc.rs` unit tests still pass: `cargo test -p niri icc`.
5. On a real device: set `icc-profile "/path/to/profile.icc"` in an `output`
   block, run niri, and verify colours look correct.  Set `icc-passthrough true`
   on a window rule for mpv/Krita and verify those windows are not
   double-corrected.

---

## Known limitations / future work

- **ICC passthrough + rounded corners conflict**: `IccPassthroughRenderElement`
  and `ClippedSurfaceRenderElement` both use `override_default_tex_program()`.
  They cannot be stacked on the same `WaylandSurfaceRenderElement`.  When a
  window has both `geometry-corner-radius` and `icc-passthrough true`, the
  tile's `clip` closure passes the ICC passthrough element through unchanged
  (no clipping).  A future fix would merge both into a single combined shader.

- **Only applied at output-add time**: the ICC profile is read once when the
  output is connected.  A future improvement would re-apply when the config
  reloads while the output is already active.

- **No ICC for wlr-gamma-control**: if a user installs a separate gamma-control
  client (e.g. gammastep), the two DRM blob writes will race.  This is an
  inherent limitation of the single-CRTC-property design.
