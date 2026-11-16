//! The GLSL every OpenGL backend draws scenes with (WebGL 2, OpenGL ES 3,
//! desktop OpenGL 3.3): a physically based model (GGX specular, Lambert
//! diffuse) with up to [`MAX_LIGHTS`] lights, a full texture set,
//! image-based lighting, a directional shadow, ACES tone mapping.
//!
//! ## Programs
//!
//! Build each program with [`program_source`]: the `#version` line
//! ([`ES_HEADER`] or [`CORE_HEADER`]), then one `#define` line per
//! [`Variant`] flag (`INSTANCED`, `SKINNED`, `DEPTH_ONLY`, in that order),
//! then [`VERTEX`] or [`FRAGMENT`]. The sky (an environment drawn as the
//! background) is [`SKY_VERTEX`] + [`SKY_FRAGMENT`] after the version line.
//! Programs: one per variant a frame uses (`Draw::variant`), the depth-only
//! ones for the shadow casters, and the sky.
//!
//! After linking, give every sampler its texture unit once (`glUniform1i`),
//! even the ones a variant does not use: samplers of different types may not
//! share a unit.
//!
//! ## Attributes
//!
//! | Location | Attribute | Type | Source |
//! |---|---|---|---|
//! | 0 | `a_position` | vec3 | mesh |
//! | 1 | `a_normal` | vec3 | mesh |
//! | 2 | `a_uv` | vec2 | mesh; constant `(0, 0)` (`glVertexAttrib2f`) when absent |
//! | 3 | `a_color` | vec4 | mesh (linear); constant `(1, 1, 1, 1)` (`glVertexAttrib4f`) when absent |
//! | 4 | `a_joints` | uvec4 | `SKINNED`: mesh `u16` × 4, `glVertexAttribIPointer` |
//! | 5 | `a_weights` | vec4 | `SKINNED`: mesh |
//! | 6–9 | `a_instance` | mat4 (4 columns) | `INSTANCED`: per instance (divisor 1), `T · R(q) · S` of the `I` blob |
//! | 10 | `a_instance_color` | vec4 | `INSTANCED`: per instance (divisor 1, linear); constant white when the set has no colors |
//!
//! ## Uniforms
//!
//! Per frame (set once per program per frame):
//!
//! | Uniform | Type | From |
//! |---|---|---|
//! | `u_view_proj` | mat4 | `Frame::view_proj` (depth pass: `ShadowPass::view_proj`) |
//! | `u_camera` | vec3 | `Frame::camera_position` |
//! | `u_ambient` | vec3 | `Frame::ambient` (linear, × intensity) |
//! | `u_light_count` | int | `Frame::lights.len()` |
//! | `u_light_kind[i]` | int | 1 directional, 2 point, 3 spot |
//! | `u_light_color[i]` | vec3 | linear color × intensity |
//! | `u_light_position[i]` | vec3 | world position (point, spot) |
//! | `u_light_direction[i]` | vec3 | unit direction the light travels |
//! | `u_light_params[i]` | vec4 | range (0: none), cos outer half-angle, cos inner half-angle, 0 |
//! | `u_exposure` | float | `Frame::exposure` |
//! | `u_has_env` | int | 1: `Frame::env` is set and its blob loaded |
//! | `u_env` | sampler2D (unit 5) | the `E` blob's levels as the mipmaps of one RGBA16F texture, row 0 = top (+Y) uploaded first, `LINEAR_MIPMAP_LINEAR`, wrap S repeat, T clamp |
//! | `u_env_levels` | float | its level count |
//! | `u_env_sh[9]` | vec3 | its 9 SH coefficients (`sh[3i..3i+3]`) |
//! | `u_env_intensity` | float | `FrameEnv::intensity` |
//! | `u_has_shadow` | int | 1: `Frame::shadow` is set |
//! | `u_shadow_map` | sampler2DShadow (unit 6) | the depth texture, `TEXTURE_COMPARE_MODE = COMPARE_REF_TO_TEXTURE`, `LEQUAL`, `LINEAR`, clamp to edge |
//! | `u_shadow_view_proj` | mat4 | `ShadowPass::view_proj` |
//! | `u_shadow_light` | int | `ShadowPass::light` |
//! | `u_shadow_bias` | float | `ShadowPass::bias` |
//! | `u_shadow_normal_bias` | float | `ShadowPass::normal_bias` |
//! | `u_shadow_texel` | float | `1 / ShadowPass::map_size` |
//!
//! Per draw:
//!
//! | Uniform | Type | From |
//! |---|---|---|
//! | `u_model` | mat4 | `Draw::world` |
//! | `u_normal_matrix` | mat3 | `Draw::normal` |
//! | `u_bones` | sampler2D (unit 7) | `SKINNED`: `Draw::joints` as an RGBA32F texture 4 texels wide (a joint's 4 columns), one row per joint, `NEAREST` |
//! | `u_color` | vec4 | `DrawMaterial::color` (linear rgb, alpha) |
//! | `u_emissive` | vec3 | linear |
//! | `u_metalness`, `u_roughness` | float | |
//! | `u_unlit` | int | 1: flat color |
//! | `u_alpha_mode` | int | 0 blend, 1 mask, 2 opaque (`DrawMaterial::alpha`) |
//! | `u_alpha_cutoff` | float | masked: alpha below it is discarded |
//! | `u_receive_shadows` | int | `Draw::receive_shadows` |
//! | `u_has_texture` | int | 1: `u_texture` (unit 0) holds the base-color image (sRGB) |
//! | `u_has_mr_map` | int | 1: `u_mr_map` (unit 1): G roughness, B metalness |
//! | `u_has_normal_map` | int | 1: `u_normal_map` (unit 2) |
//! | `u_normal_scale` | float | `DrawMaterial::normal_scale` |
//! | `u_has_emissive_map` | int | 1: `u_emissive_map` (unit 3, sRGB) |
//! | `u_has_ao_map` | int | 1: `u_ao_map` (unit 4): R occlusion |
//!
//! Textures are uploaded as stored (8-bit, not sRGB formats): the shader
//! linearizes base-color and emissive images itself (`pow(c, 2.2)`).
//!
//! The sky program: `u_inverse_view_proj` (`Frame::inverse_view_proj`),
//! `u_camera`, `u_env`, `u_env_intensity`, `u_exposure`; drawn first, as one
//! triangle of 3 vertices without attributes (`glDrawArrays(TRIANGLES, 0,
//! 3)`), depth test and depth writes off.
//!
//! ## Environment conventions
//!
//! Directions map to the equirectangular images by
//! `u = 0.5 + atan(d.x, −d.z) / 2π`, `v = acos(d.y) / π` (v = 0: +Y). The
//! spherical harmonics use the real basis in this order (x, y, z: a unit
//! world direction):
//! `0.282095`, `0.488603·y`, `0.488603·z`, `0.488603·x`, `1.092548·x·y`,
//! `1.092548·y·z`, `0.315392·(3z² − 1)`, `1.092548·x·z`,
//! `0.546274·(x² − y²)`; `Σ sh_i·Y_i(n)` is the irradiance `E` around `n`
//! (a uniform radiance `L` gives `π·L`, see [`env`](super::env)), so a
//! Lambertian surface reflects `albedo · E / π`.

/// Most lights a draw uses (ambient lights are summed apart).
pub const MAX_LIGHTS: usize = 8;

/// First line for WebGL 2 and OpenGL ES 3.
pub const ES_HEADER: &str = "#version 300 es\n";
/// First line for desktop OpenGL 3.3 core profiles.
pub const CORE_HEADER: &str = "#version 330 core\n";

/// Which program a draw needs: the `#define`s before the shader body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Variant {
    /// `INSTANCED`: per-instance matrices and colors (attributes 6–10).
    pub instanced: bool,
    /// `SKINNED`: joints and weights (attributes 4, 5) and `u_bones`.
    pub skinned: bool,
    /// `DEPTH_ONLY`: the shadow map pass (no color output).
    pub depth_only: bool,
}

impl Variant {
    /// The `#define` lines, in their canonical order.
    pub fn defines(&self) -> String {
        let mut s = String::new();
        if self.instanced {
            s.push_str("#define INSTANCED\n");
        }
        if self.skinned {
            s.push_str("#define SKINNED\n");
        }
        if self.depth_only {
            s.push_str("#define DEPTH_ONLY\n");
        }
        s
    }
}

/// A shader stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// [`VERTEX`].
    Vertex,
    /// [`FRAGMENT`].
    Fragment,
}

/// The full source of one stage of `variant`'s program: `header` (its
/// `#version` line), the variant's `#define`s, then the stage's body.
/// Every backend builds its programs exactly so.
pub fn program_source(header: &str, stage: Stage, variant: Variant) -> String {
    let body = match stage {
        Stage::Vertex => VERTEX,
        Stage::Fragment => FRAGMENT,
    };
    format!("{header}{}{body}", variant.defines())
}

/// The vertex shader (after the `#version` line and the defines).
pub const VERTEX: &str = r#"precision highp float;
precision highp int;
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in vec4 a_color;
#ifdef SKINNED
layout(location = 4) in uvec4 a_joints;
layout(location = 5) in vec4 a_weights;
uniform highp sampler2D u_bones;
#endif
#ifdef INSTANCED
layout(location = 6) in mat4 a_instance;
layout(location = 10) in vec4 a_instance_color;
#endif
uniform mat4 u_model;
uniform mat3 u_normal_matrix;
uniform mat4 u_view_proj;
out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_color;
#ifdef SKINNED
mat4 bone(uint j) {
  int y = int(j);
  return mat4(texelFetch(u_bones, ivec2(0, y), 0), texelFetch(u_bones, ivec2(1, y), 0),
              texelFetch(u_bones, ivec2(2, y), 0), texelFetch(u_bones, ivec2(3, y), 0));
}
#endif
void main() {
  vec4 position = vec4(a_position, 1.0);
  vec3 normal = a_normal;
  v_color = a_color;
#ifdef SKINNED
  mat4 skin = a_weights.x * bone(a_joints.x) + a_weights.y * bone(a_joints.y)
            + a_weights.z * bone(a_joints.z) + a_weights.w * bone(a_joints.w);
  position = skin * position;
  normal = mat3(skin) * normal;
#endif
#ifdef INSTANCED
  position = a_instance * position;
  normal = transpose(inverse(mat3(a_instance))) * normal;
  v_color *= a_instance_color;
#endif
  vec4 world = u_model * position;
  v_world = world.xyz;
  v_normal = u_normal_matrix * normal;
  v_uv = a_uv;
  gl_Position = u_view_proj * world;
}
"#;

/// The fragment shader (after the `#version` line and the defines).
pub const FRAGMENT: &str = r#"precision highp float;
precision highp int;
#ifdef DEPTH_ONLY
void main() {}
#else
precision highp sampler2DShadow;
#define MAX_LIGHTS 8
in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_color;
uniform vec3 u_camera;
uniform vec3 u_ambient;
uniform int u_light_count;
uniform int u_light_kind[MAX_LIGHTS];
uniform vec3 u_light_color[MAX_LIGHTS];
uniform vec3 u_light_position[MAX_LIGHTS];
uniform vec3 u_light_direction[MAX_LIGHTS];
uniform vec4 u_light_params[MAX_LIGHTS];
uniform float u_exposure;
uniform vec4 u_color;
uniform vec3 u_emissive;
uniform float u_metalness;
uniform float u_roughness;
uniform int u_unlit;
uniform int u_alpha_mode;
uniform float u_alpha_cutoff;
uniform int u_has_texture;
uniform sampler2D u_texture;
uniform int u_has_mr_map;
uniform sampler2D u_mr_map;
uniform int u_has_normal_map;
uniform sampler2D u_normal_map;
uniform float u_normal_scale;
uniform int u_has_emissive_map;
uniform sampler2D u_emissive_map;
uniform int u_has_ao_map;
uniform sampler2D u_ao_map;
uniform int u_has_env;
uniform sampler2D u_env;
uniform float u_env_levels;
uniform vec3 u_env_sh[9];
uniform float u_env_intensity;
uniform int u_has_shadow;
uniform int u_receive_shadows;
uniform sampler2DShadow u_shadow_map;
uniform mat4 u_shadow_view_proj;
uniform int u_shadow_light;
uniform float u_shadow_bias;
uniform float u_shadow_normal_bias;
uniform float u_shadow_texel;
out vec4 o_color;
const float PI = 3.14159265;

// Equirectangular coordinates of a direction (v = 0: straight up).
vec2 equirect(vec3 d) {
  return vec2(0.5 + atan(d.x, -d.z) / (2.0 * PI), acos(clamp(d.y, -1.0, 1.0)) / PI);
}

// The environment's irradiance around n, from its spherical harmonics.
vec3 sh_irradiance(vec3 n) {
  return u_env_sh[0] * 0.282095
    + u_env_sh[1] * 0.488603 * n.y
    + u_env_sh[2] * 0.488603 * n.z
    + u_env_sh[3] * 0.488603 * n.x
    + u_env_sh[4] * 1.092548 * n.x * n.y
    + u_env_sh[5] * 1.092548 * n.y * n.z
    + u_env_sh[6] * 0.315392 * (3.0 * n.z * n.z - 1.0)
    + u_env_sh[7] * 1.092548 * n.x * n.z
    + u_env_sh[8] * 0.546274 * (n.x * n.x - n.y * n.y);
}

// Karis' analytic approximation of the split-sum environment BRDF.
vec3 env_brdf(vec3 f0, float rough, float nv) {
  const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
  const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
  vec4 r = rough * c0 + c1;
  float a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
  vec2 ab = vec2(-1.04, 1.04) * a004 + r.zw;
  return f0 * ab.x + ab.y;
}

// The normal bent by the normal map, in a cotangent frame built from screen
// derivatives (no tangents needed). Texture v grows downward while the map's
// green points up the image: the bitangent is -dp/dv.
vec3 perturb(vec3 n, vec3 p, vec2 uv) {
  vec3 m = texture(u_normal_map, uv).xyz * 2.0 - 1.0;
  m.xy *= u_normal_scale;
  vec3 dp1 = dFdx(p);
  vec3 dp2 = dFdy(p);
  vec2 duv1 = dFdx(uv);
  vec2 duv2 = dFdy(uv);
  vec3 dp2perp = cross(dp2, n);
  vec3 dp1perp = cross(n, dp1);
  vec3 t = dp2perp * duv1.x + dp1perp * duv2.x;
  vec3 b = -(dp2perp * duv1.y + dp1perp * duv2.y);
  float scale = inversesqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
  return normalize(mat3(t * scale, b * scale, n) * m);
}

// 1: lit, 0: in the shadow light's shadow (3x3 PCF).
float shadow(vec3 n) {
  vec4 p = u_shadow_view_proj * vec4(v_world + n * u_shadow_normal_bias, 1.0);
  vec3 c = p.xyz / p.w * 0.5 + 0.5;
  if (c.x < 0.0 || c.x > 1.0 || c.y < 0.0 || c.y > 1.0 || c.z > 1.0) return 1.0;
  float sum = 0.0;
  for (int x = -1; x <= 1; x++) {
    for (int y = -1; y <= 1; y++) {
      vec2 o = vec2(float(x), float(y)) * u_shadow_texel;
      sum += texture(u_shadow_map, vec3(c.xy + o, c.z - u_shadow_bias));
    }
  }
  return sum / 9.0;
}

// ACES filmic curve (Narkowicz' fit).
vec3 aces(vec3 x) {
  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

void main() {
  vec4 base = u_color * v_color;
  if (u_has_texture == 1) {
    vec4 t = texture(u_texture, v_uv);
    base *= vec4(pow(t.rgb, vec3(2.2)), t.a);
  }
  if (u_alpha_mode == 1) {
    if (base.a < u_alpha_cutoff) discard;
    base.a = 1.0;
  } else if (u_alpha_mode == 2) {
    base.a = 1.0;
  }
  vec3 emissive = u_emissive;
  if (u_has_emissive_map == 1) {
    emissive *= pow(texture(u_emissive_map, v_uv).rgb, vec3(2.2));
  }
  vec3 color;
  if (u_unlit == 1) {
    color = base.rgb;
  } else {
    vec3 n = normalize(v_normal);
    if (!gl_FrontFacing) n = -n;
    vec3 geometric = n;
    if (u_has_normal_map == 1) n = perturb(n, v_world, v_uv);
    float metal = u_metalness;
    float rough = u_roughness;
    if (u_has_mr_map == 1) {
      vec4 mr = texture(u_mr_map, v_uv);
      rough *= mr.g;
      metal *= mr.b;
    }
    float ao = u_has_ao_map == 1 ? texture(u_ao_map, v_uv).r : 1.0;
    vec3 v = normalize(u_camera - v_world);
    float nv = max(dot(n, v), 1e-4);
    rough = clamp(rough, 0.04, 1.0);
    float a = rough * rough;
    float a2 = a * a;
    float k = (rough + 1.0) * (rough + 1.0) / 8.0;
    vec3 f0 = mix(vec3(0.04), base.rgb, metal);
    vec3 diffuse = base.rgb * (1.0 - metal);
    // Ambient light: diffuse, plus a little specular so metals are not black.
    color = u_ambient * (diffuse + f0 * 0.5) * ao;
    if (u_has_env == 1) {
      vec3 irradiance = max(sh_irradiance(n), vec3(0.0)) / PI;
      vec3 r = reflect(-v, n);
      vec3 prefiltered = textureLod(u_env, equirect(r), rough * (u_env_levels - 1.0)).rgb;
      color += (irradiance * diffuse + prefiltered * env_brdf(f0, rough, nv)) * u_env_intensity * ao;
    }
    float lit = 1.0;
    if (u_has_shadow == 1 && u_receive_shadows == 1) lit = shadow(geometric);
    for (int i = 0; i < MAX_LIGHTS; i++) {
      if (i >= u_light_count) break;
      vec3 l;
      float att = 1.0;
      if (u_light_kind[i] == 1) {
        l = -normalize(u_light_direction[i]);
      } else {
        vec3 d = u_light_position[i] - v_world;
        float dist = length(d);
        l = d / max(dist, 1e-4);
        float range = u_light_params[i].x;
        if (range > 0.0) {
          float f = clamp(1.0 - dist / range, 0.0, 1.0);
          att = f * f;
        }
        if (u_light_kind[i] == 3) {
          float c = dot(-l, normalize(u_light_direction[i]));
          att *= smoothstep(u_light_params[i].y, u_light_params[i].z, c);
        }
      }
      if (i == u_shadow_light) att *= lit;
      float nl = max(dot(n, l), 0.0);
      if (nl <= 0.0 || att <= 0.0) continue;
      vec3 h = normalize(v + l);
      float nh = max(dot(n, h), 0.0);
      float vh = max(dot(v, h), 0.0);
      float dd = nh * nh * (a2 - 1.0) + 1.0;
      float D = a2 / (PI * dd * dd);
      float G = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
      vec3 F = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
      vec3 spec = D * G * F / (4.0 * nv * nl + 1e-4);
      vec3 kd = (1.0 - F) * (1.0 - metal);
      // Punctual lights in "intensity 1 = full white" units: × PI.
      color += (kd * base.rgb / PI + spec) * u_light_color[i] * att * nl * PI;
    }
  }
  color += emissive;
  color = pow(aces(color * u_exposure), vec3(1.0 / 2.2));
  // Premultiplied alpha (canvases, TextureViews and GL areas composite so).
  o_color = vec4(color * base.a, base.a);
}
#endif
"#;

/// The sky's vertex shader: one triangle covering the viewport, from
/// `gl_VertexID` (no attributes).
pub const SKY_VERTEX: &str = r#"precision highp float;
precision highp int;
out vec2 v_ndc;
void main() {
  vec2 p = vec2(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID & 2) * 2 - 1));
  v_ndc = p;
  gl_Position = vec4(p, 0.0, 1.0);
}
"#;

/// The sky's fragment shader: the environment's sharpest level seen from
/// the camera, tone mapped like the scene.
pub const SKY_FRAGMENT: &str = r#"precision highp float;
in vec2 v_ndc;
uniform mat4 u_inverse_view_proj;
uniform vec3 u_camera;
uniform sampler2D u_env;
uniform float u_env_intensity;
uniform float u_exposure;
out vec4 o_color;
const float PI = 3.14159265;
vec3 aces(vec3 x) {
  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}
void main() {
  vec4 far = u_inverse_view_proj * vec4(v_ndc, 1.0, 1.0);
  vec3 d = normalize(far.xyz / far.w - u_camera);
  vec2 uv = vec2(0.5 + atan(d.x, -d.z) / (2.0 * PI), acos(clamp(d.y, -1.0, 1.0)) / PI);
  vec3 c = textureLod(u_env, uv, 0.0).rgb * u_env_intensity;
  o_color = vec4(pow(aces(c * u_exposure), vec3(1.0 / 2.2)), 1.0);
}
"#;
