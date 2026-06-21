// Fragment shader for ICC passthrough windows.
//
// When an ICC profile is active on the output, the hardware applies a 3-stage color
// pipeline to whatever the compositor writes to the framebuffer:
//
//     panel = GAMMA_LUT( CTM( DEGAMMA_LUT( shader_output ) ) )
//
// where DEGAMMA_LUT is the sRGB EOTF (encoded → linear), CTM converts sRGB linear to
// display-native linear, and GAMMA_LUT applies the display's TRC (linear → encoded
// signal). For a color-managed app (mpv, Krita, Firefox with CMS) the pixel `p` it
// writes is *already* the encoded signal it wants the panel to receive. So we need
// the shader to produce `s` such that the pipeline above outputs `p`:
//
//     GAMMA_LUT( CTM( DEGAMMA_LUT(s) ) ) = p
//   ⇒ CTM( DEGAMMA_LUT(s) ) = display_eotf(p)            // invert GAMMA_LUT
//   ⇒ DEGAMMA_LUT(s) = ctm_inverse · display_eotf(p)     // invert CTM
//   ⇒ s = srgb_oetf( ctm_inverse · display_eotf(p) )     // invert DEGAMMA_LUT
//
// So the shader: linearises with the **display** TRC (not sRGB!), multiplies by the
// inverse CTM, then re-encodes with the **sRGB** OETF (matching the hardware DEGAMMA).
// Linearising with sRGB instead of the display TRC was the bug in the first version
// of this shader — it would only round-trip when the display TRC was sRGB-shaped.

#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Inverse of the DRM CTM (display-native linear → sRGB linear), column-major after
// upload (we pass row-major data with transpose=true).
uniform mat3 icc_ctm_inverse;

// Per-channel display gamma exponent: `display_linear = encoded^display_gamma`.
// This is the EOTF of the panel's TRC, approximated as a pure power per channel
// (exact for ICC pure-power profiles; a least-squares fit for parametric/sampled
// curves — matches typical display TRCs within ~1 LSB).
uniform vec3 display_gamma;

// Display TRC EOTF (encoded → linear) using the per-channel power approximation.
vec3 display_to_linear(vec3 c) {
    c = max(c, vec3(0.0));
    return pow(c, display_gamma);
}

// sRGB OETF: linear → encoded. Matches what hardware DEGAMMA_LUT inverts.
float linear_to_srgb_channel(float c) {
    c = clamp(c, 0.0, 1.0);
    if (c <= 0.0031308) {
        return c * 12.92;
    } else {
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
}

vec3 linear_to_srgb(vec3 c) {
    return vec3(
        linear_to_srgb_channel(c.r),
        linear_to_srgb_channel(c.g),
        linear_to_srgb_channel(c.b)
    );
}

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    // Wayland surfaces use premultiplied alpha. Work on straight-alpha for the colour
    // math (linearisation and the CTM are non-linear / not alpha-affine), then re-multiply.
    float a = color.a;
    vec3 rgb = (a > 0.0) ? color.rgb / a : vec3(0.0);

    // Linearise with the display TRC, undo the CTM, re-encode with the sRGB OETF that
    // the hardware DEGAMMA_LUT inverts.
    rgb = display_to_linear(rgb);
    rgb = icc_ctm_inverse * rgb;
    rgb = linear_to_srgb(rgb);

    color = vec4(rgb * a, a);
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.0, 0.2, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
