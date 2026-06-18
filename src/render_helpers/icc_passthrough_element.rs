use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformValue};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::renderer::{AsGlesFrame as _, NiriRenderer};
use super::shaders::Shaders;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

/// Wraps a `WaylandSurfaceRenderElement` and applies the inverse of the DRM CTM as a pre-pass,
/// so that color-managed windows (e.g. mpv, Krita, Firefox) are not double-corrected by the
/// hardware color transform matrix.
#[derive(Debug)]
pub struct IccPassthroughRenderElement<R: NiriRenderer> {
    inner: WaylandSurfaceRenderElement<R>,
    program: GlesTexProgram,
    /// Inverse CTM as a column-major `[f32; 9]` (display-native → sRGB, linear light).
    ctm_inverse: [f32; 9],
}

impl<R: NiriRenderer> IccPassthroughRenderElement<R> {
    pub fn new(
        elem: WaylandSurfaceRenderElement<R>,
        program: GlesTexProgram,
        ctm_inverse: [f32; 9],
    ) -> Self {
        Self {
            inner: elem,
            program,
            ctm_inverse,
        }
    }

    pub fn shader(renderer: &mut R) -> Option<&GlesTexProgram> {
        Shaders::get(renderer).icc_passthrough.as_ref()
    }

    fn uniforms(&self) -> Vec<Uniform<'static>> {
        // ctm_inverse is stored row-major (see src/icc.rs); ask GL to transpose
        // on upload so the shader sees the correct column-major matrix.
        vec![Uniform::new(
            "icc_ctm_inverse",
            UniformValue::Matrix3x3 {
                matrices: vec![self.ctm_inverse],
                transpose: true,
            },
        )]
    }
}

impl<R: NiriRenderer> Element for IccPassthroughRenderElement<R> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // We can't claim opaque regions because the shader may produce non-opaque results
        // (in practice the CTM is a linear transform that shouldn't add transparency, but
        // it's safest to be conservative here).
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for IccPassthroughRenderElement<GlesRenderer> {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(self.program.clone(), self.uniforms());
        RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        frame.clear_tex_program_override();
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>>
    for IccPassthroughRenderElement<TtyRenderer<'render>>
{
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        frame
            .as_gles_frame()
            .override_default_tex_program(self.program.clone(), self.uniforms());
        RenderElement::draw(&self.inner, frame, src, dst, damage, opaque_regions, cache)?;
        frame.as_gles_frame().clear_tex_program_override();
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
