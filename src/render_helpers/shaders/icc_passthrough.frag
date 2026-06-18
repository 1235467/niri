// Fragment shader for ICC passthrough windows.
//
// When the DRM CRTC has a CTM (color transform matrix) applied for ICC color management,
// color-managed applications (mpv, Krita, Firefox with color management enabled) output pixels
// that are already in the display's native color space. However the DRM CTM will then also be
// applied on top, causing double-correction.
//
// This shader counteracts the DRM CTM by applying its inverse (ctm_inverse, display→sRGB) in the
// GPU compositing step, so that after the hardware CTM the window ends up unchanged.
//
// Math:
//   GPU applies:  ctm_inverse  (display-native → sRGB)
//   DRM applies:  CTM          (sRGB → display-native)
//   Net effect:   CTM × ctm_inverse = I  (identity)
//
// Both matrices operate in linear light.  We linearise the sampled sRGB colour, apply the
// matrix, and re-encode to sRGB before writing gl_FragColor.

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

// Inverse of the DRM CTM (display-native → sRGB, linear light), stored column-major.
uniform mat3 icc_ctm_inverse;

// sRGB EOTF: encoded → linear
float srgb_to_linear_channel(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

vec3 srgb_to_linear(vec3 c) {
    return vec3(
        srgb_to_linear_channel(c.r),
        srgb_to_linear_channel(c.g),
        srgb_to_linear_channel(c.b)
    );
}

// sRGB OETF: linear → encoded
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

    // Pre-multiply alpha before colour transform to avoid colour bleeding at edges.
    // (The app's output is already pre-multiplied if it uses standard Wayland alpha.)
    // We work on straight-alpha for the colour math and re-multiply at the end.
    float a = color.a;
    vec3 rgb = (a > 0.0) ? color.rgb / a : vec3(0.0);

    // Linearise sRGB, apply inverse CTM, re-encode.
    rgb = srgb_to_linear(rgb);
    rgb = icc_ctm_inverse * rgb;
    rgb = linear_to_srgb(rgb);

    // Re-apply alpha.
    color = vec4(rgb * a, a);

    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.0, 0.2, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
