//! OpenGL ES 3 port of the Windows renderer: the same two shaders, instanced sprites and pile quads.

use std::mem::size_of;

use glow::HasContext;

use crate::simulation::{COLUMN_WIDTH, SnowSimulation, Sprite};

/// Heights live in a 2D float texture, this many per row, as GL ES has no buffer textures.
const HEIGHT_ROW: usize = 4096;

const SPRITE_VERTEX: &str = r#"#version 300 es
precision highp float;
uniform vec2 viewSize;
uniform float scale;
uniform float time;

layout(location = 0) in vec2 aPosition;
layout(location = 1) in vec2 aVelocity;
layout(location = 2) in float aSize;
layout(location = 3) in float aAlpha;
layout(location = 4) in float aSoft;
layout(location = 5) in float aTintAmount;
layout(location = 6) in vec3 aTintColor;
layout(location = 7) in float aLump;

out vec2 vCoord;
out float vAlpha;
out float vSoft;
out vec4 vTint;
out vec2 vStreak;
out float vRadius;
out float vLump;
out float vSpin;

void main() {
    float pixels = aSize * scale;
    // About a 1/55 s exposure, capped so even the fastest grain stays a short smear and never reads as rain.
    vec2 travel = aVelocity * 0.018;
    float smear = min(length(travel), aSize * (aLump > 0.0 ? 1.2 : 2.2));
    vec2 streak = smear > 0.25 ? normalize(travel) * smear * scale : vec2(0.0);
    // A lump's ragged edge reaches past its nominal size.
    float reach = aLump > 0.0 ? 1.3 : 1.0;
    float extent = max(pixels * reach + length(streak), 2.0);
    int vid = gl_VertexID;
    vec2 corner = vec2(float(vid & 1), float(vid >> 1));
    vec2 center = aPosition / viewSize * 2.0 - 1.0;
    gl_Position = vec4(center + (corner - 0.5) * extent * 2.0 / (viewSize * scale), 0.0, 1.0);
    vCoord = vec2(corner.x, 1.0 - corner.y);
    vStreak = streak / extent;
    vRadius = pixels / extent * 0.5;
    // A streak spreads the same flake over more pixels, so each is fainter.
    vAlpha = aAlpha * min(pixels / 2.0, 1.0) * pixels / max(pixels + length(streak) * 0.6, 0.001);
    vSoft = aSoft;
    vTint = vec4(aTintColor, aTintAmount);
    vLump = aLump;
    vSpin = aLump > 0.0 ? time * (aLump - 0.5) * 9.0 + aLump * 40.0 : 0.0;
}
"#;

const SPRITE_FRAGMENT: &str = r#"#version 300 es
precision highp float;
uniform vec4 light;

in vec2 vCoord;
in float vAlpha;
in float vSoft;
in vec4 vTint;
in vec2 vStreak;
in float vRadius;
in float vLump;
in float vSpin;

out vec4 fragColor;

void main() {
    // Quad coordinates run down the screen; velocities run up.
    vec2 p = vec2(vCoord.x - 0.5, 0.5 - vCoord.y);
    vec2 half_ = vStreak * 0.5;
    float span = dot(half_, half_);
    float t = span > 0.0 ? clamp(dot(p, half_) / span, -1.0, 1.0) : 0.0;
    vec2 d = p - half_ * t;
    float r = length(d) / max(vRadius, 0.001);
    float shading = 0.0;
    if (vLump > 0.0) {
        float angle = atan(d.y, d.x) + vSpin;
        r /= 1.0 + 0.14 * sin(3.0 * angle) + 0.08 * sin(5.0 * angle + vLump * 21.0) + 0.05 * sin(2.0 * angle + vLump * 9.0);
        shading = clamp(dot(d / max(vRadius, 0.001), vec2(0.45, -0.6)), 0.0, 1.0) * 0.4;
    }
    // Brighter at the head, where the flake is now, than at the tail, where it was.
    float head = span > 0.0 ? 0.15 + 0.85 * pow(t * 0.5 + 0.5, 1.5) : 1.0;
    float a = (1.0 - smoothstep(mix(0.6, 0.0, vSoft), 1.0, r)) * vAlpha * head;
    if (a < 0.002) discard;
    vec3 core = vec3(0.965, 0.975, 0.995);
    vec3 rim = vec3(0.62, 0.67, 0.77);
    vec3 color = mix(core, rim, clamp(smoothstep(0.35, 1.0, r) * 0.65 + shading, 0.0, 1.0));
    color = mix(color, vTint.rgb, vTint.a) * light.rgb;
    fragColor = vec4(color * a, a);
}
"#;

const PILE_VERTEX: &str = r#"#version 300 es
precision highp float;
uniform vec2 viewSize;
uniform float columnWidth;
uniform int columnCount;
uniform float pileTop;
uniform float pileBottom;
uniform vec2 origin;

out vec2 vLocal;

void main() {
    int vid = gl_VertexID;
    vec2 corner = vec2(float(vid & 1), float(vid >> 1));
    vec2 p = vec2(corner.x * float(columnCount) * columnWidth, mix(pileBottom, pileTop, corner.y));
    gl_Position = vec4((p + origin) / viewSize * 2.0 - 1.0, 0.0, 1.0);
    vLocal = p;
}
"#;

const PILE_FRAGMENT: &str = r#"#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;
uniform float scale;
uniform float time;
uniform float columnWidth;
uniform int columnCount;
uniform int occluderCount;
uniform vec2 origin;
uniform int heightOffset;
uniform vec4 light;
uniform vec4 shadowTint;
uniform vec4 occluders[16];
uniform sampler2D heights;

in vec2 vLocal;
out vec4 fragColor;

float heightIndex(int index) {
    return texelFetch(heights, ivec2(index % 4096, index / 4096), 0).r;
}

float heightAt(int start, float x) {
    float f = x / columnWidth - 0.5;
    float base = floor(f);
    int i = clamp(int(base), 0, columnCount - 1);
    int j = min(i + 1, columnCount - 1);
    return mix(heightIndex(start + i), heightIndex(start + j), clamp(f - base, 0.0, 1.0));
}

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

void main() {
    vec2 world = vLocal + origin;
    for (int k = 0; k < occluderCount; k++) {
        vec4 o = occluders[k];
        if (world.x >= o.x && world.x < o.z && world.y >= o.y && world.y < o.w) discard;
    }

    float x = vLocal.x;
    float level = heightAt(heightOffset, x);
    float bed = heightAt(heightOffset + columnCount, x);
    float depth = level - vLocal.y;
    float coverage = clamp(depth * scale + 0.5, 0.0, 1.0)
        * clamp((vLocal.y - bed) * scale + 0.5, 0.0, 1.0)
        * clamp((level - bed) / 0.75, 0.0, 1.0);
    if (coverage <= 0.0) discard;

    float slope = (heightAt(heightOffset, x + 2.0) - heightAt(heightOffset, x - 2.0)) * 0.25;
    float lit = dot(normalize(vec2(-slope, 1.0)), normalize(vec2(-0.5, 1.0)));

    vec3 shadow = mix(vec3(0.72, 0.78, 0.88), shadowTint.rgb, shadowTint.a);
    vec3 surface = mix(vec3(0.925, 0.94, 0.965), min(shadowTint.rgb * 1.18, vec3(1.0)), shadowTint.a * 0.2);
    float shade = (1.0 - exp(-depth / 30.0)) * 0.32 + (1.0 - lit) * 0.6 * exp(-depth / 5.0);
    vec3 color = mix(surface, shadow, clamp(shade, 0.0, 1.0));
    color = mix(color, vec3(0.975, 0.982, 0.995), exp(-depth * 0.9) * 0.6);
    // Pixel coordinates run up here and down in Direct3D; the grain is random either way.
    color += (hash(floor(gl_FragCoord.xy)) - 0.5) * 0.02;

    float seed = hash(floor(gl_FragCoord.xy * 0.5) + 31.7);
    if (seed > 0.992) {
        float twinkle = pow(clamp(sin(time * (0.6 + seed * 1.8) + seed * 90.0), 0.0, 1.0), 28.0);
        color = mix(color, vec3(1.0), twinkle * exp(-depth / 7.0));
    }

    color *= light.rgb;
    fragColor = vec4(color * coverage, coverage);
}
"#;

/// The light the snow is drawn in.
#[derive(Clone, Copy)]
pub struct Ambient {
    pub light: [f32; 3],
    pub shadow_tint: [f32; 4],
}

struct Program {
    program: glow::Program,
    uniforms: Vec<(&'static str, Option<glow::UniformLocation>)>,
}

impl Program {
    fn new(gl: &glow::Context, vertex: &str, fragment: &str, names: &[&'static str]) -> Result<Program, String> {
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            let uniforms = names.iter().map(|&name| (name, gl.get_uniform_location(program, name))).collect();
            Ok(Program { program, uniforms })
        }
    }

    fn at(&self, name: &str) -> Option<&glow::UniformLocation> {
        self.uniforms.iter().find(|(n, _)| *n == name).and_then(|(_, location)| location.as_ref())
    }
}

pub struct Renderer {
    pub gl: glow::Context,
    sprite: Program,
    pile: Program,
    sprite_array: glow::VertexArray,
    empty_array: glow::VertexArray,
    sprite_buffer: glow::Buffer,
    sprite_capacity: usize,
    heights: glow::Texture,
    height_rows: usize,
    sprite_scratch: Vec<Sprite>,
    height_scratch: Vec<f32>,
}

impl Renderer {
    pub fn new(gl: glow::Context) -> Result<Renderer, String> {
        let sprite = Program::new(&gl, SPRITE_VERTEX, SPRITE_FRAGMENT, &["viewSize", "scale", "time", "light"])?;
        let pile = Program::new(
            &gl,
            PILE_VERTEX,
            PILE_FRAGMENT,
            &[
                "viewSize",
                "scale",
                "time",
                "columnWidth",
                "columnCount",
                "pileTop",
                "pileBottom",
                "origin",
                "occluderCount",
                "heightOffset",
                "light",
                "shadowTint",
                "occluders",
                "heights",
            ],
        )?;
        unsafe {
            let sprite_buffer = gl.create_buffer()?;
            let sprite_array = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(sprite_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(sprite_buffer));
            let stride = size_of::<Sprite>() as i32;
            let attributes: [(u32, i32, i32); 8] = [(0, 2, 0), (1, 2, 8), (2, 1, 16), (3, 1, 20), (4, 1, 24), (5, 1, 28), (6, 3, 32), (7, 1, 44)];
            for (index, size, offset) in attributes {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset);
                gl.vertex_attrib_divisor(index, 1);
            }
            gl.bind_vertex_array(None);
            let empty_array = gl.create_vertex_array()?;

            let heights = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(heights));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);

            Ok(Renderer {
                gl,
                sprite,
                pile,
                sprite_array,
                empty_array,
                sprite_buffer,
                sprite_capacity: 0,
                heights,
                height_rows: 0,
                sprite_scratch: Vec::new(),
                height_scratch: Vec::new(),
            })
        }
    }

    /// Draws one display's snow into the current surface, `scale` pixels per simulation point.
    pub fn draw(&mut self, simulation: &SnowSimulation, pixels: (i32, i32), scale: f32, ambient: Ambient, time: f32) {
        let sprite_count = simulation.sprite_count();
        if self.sprite_scratch.len() < sprite_count {
            self.sprite_scratch.resize(sprite_count.max(1024) * 3 / 2, Sprite::default());
        }
        let columns = simulation.column_count();
        let rows = columns.div_ceil(HEIGHT_ROW).max(1);
        if self.height_scratch.len() < rows * HEIGHT_ROW {
            self.height_scratch.resize(rows * HEIGHT_ROW, 0.0);
        }
        let sprites = simulation.write_sprites(&mut self.sprite_scratch);
        let piles = simulation.write_layers(&mut self.height_scratch[..columns]);
        let gl = &self.gl;
        let view = [simulation.width(), simulation.height()];
        let light = [ambient.light[0], ambient.light[1], ambient.light[2], 1.0];
        unsafe {
            gl.viewport(0, 0, pixels.0, pixels.1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            if !piles.is_empty() {
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.heights));
                let bytes = std::slice::from_raw_parts(self.height_scratch.as_ptr().cast::<u8>(), rows * HEIGHT_ROW * 4);
                if rows > self.height_rows {
                    self.height_rows = rows;
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R32F as i32,
                        HEIGHT_ROW as i32,
                        rows as i32,
                        0,
                        glow::RED,
                        glow::FLOAT,
                        glow::PixelUnpackData::Slice(Some(bytes)),
                    );
                } else {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        HEIGHT_ROW as i32,
                        rows as i32,
                        glow::RED,
                        glow::FLOAT,
                        glow::PixelUnpackData::Slice(Some(bytes)),
                    );
                }
                let p = &self.pile;
                gl.use_program(Some(p.program));
                gl.bind_vertex_array(Some(self.empty_array));
                gl.uniform_2_f32(p.at("viewSize"), view[0], view[1]);
                gl.uniform_1_f32(p.at("scale"), scale);
                gl.uniform_1_f32(p.at("time"), time);
                gl.uniform_1_f32(p.at("columnWidth"), COLUMN_WIDTH);
                gl.uniform_4_f32_slice(p.at("light"), &light);
                gl.uniform_4_f32_slice(p.at("shadowTint"), &ambient.shadow_tint);
                gl.uniform_1_i32(p.at("heights"), 0);
                for pile in &piles {
                    let occluders = pile.occluders.len().min(16);
                    gl.uniform_1_i32(p.at("columnCount"), pile.count as i32);
                    gl.uniform_1_f32(p.at("pileTop"), pile.top + 2.0);
                    gl.uniform_1_f32(p.at("pileBottom"), pile.bottom);
                    gl.uniform_2_f32(p.at("origin"), pile.origin.x, pile.origin.y);
                    gl.uniform_1_i32(p.at("heightOffset"), pile.offset as i32);
                    gl.uniform_1_i32(p.at("occluderCount"), occluders as i32);
                    if occluders > 0 {
                        gl.uniform_4_f32_slice(p.at("occluders"), pile.occluders[..occluders].as_flattened());
                    }
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                }
            }

            if sprites > 0 {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.sprite_buffer));
                let bytes = std::slice::from_raw_parts(self.sprite_scratch.as_ptr().cast::<u8>(), sprites * size_of::<Sprite>());
                if sprites > self.sprite_capacity {
                    self.sprite_capacity = sprites.max(1024) * 3 / 2;
                    gl.buffer_data_size(glow::ARRAY_BUFFER, (self.sprite_capacity * size_of::<Sprite>()) as i32, glow::STREAM_DRAW);
                }
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
                let s = &self.sprite;
                gl.use_program(Some(s.program));
                gl.bind_vertex_array(Some(self.sprite_array));
                gl.uniform_2_f32(s.at("viewSize"), view[0], view[1]);
                gl.uniform_1_f32(s.at("scale"), scale);
                gl.uniform_1_f32(s.at("time"), time);
                gl.uniform_4_f32_slice(s.at("light"), &light);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, sprites as i32);
            }
            gl.bind_vertex_array(None);
        }
    }
}
