use crate::config::Config;
use anyhow::{anyhow, Result};
use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderPreset {
    Off,
    SoftCrt,
    Blurpi,
    Royale,
}

impl ShaderPreset {
    pub const ALL: [Self; 4] = [Self::Off, Self::SoftCrt, Self::Blurpi, Self::Royale];

    pub fn parse(value: &str) -> Self {
        match value {
            "soft_crt" => Self::SoftCrt,
            "blurpi" => Self::Blurpi,
            "royale" => Self::Royale,
            _ => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::SoftCrt => "soft_crt",
            Self::Blurpi => "blurpi",
            Self::Royale => "royale",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off (sharp pixels)",
            Self::SoftCrt => "Soft CRT",
            Self::Blurpi => "Blurpi-inspired",
            Self::Royale => "CRT Royale-inspired",
        }
    }

    pub fn defaults(self) -> ShaderSettings {
        let (bloom, scanlines, curvature, mask, vignette) = match self {
            Self::Off => (0.0, 0.0, 0.0, 0.0, 0.0),
            Self::SoftCrt => (0.55, 0.35, 0.035, 0.25, 0.20),
            Self::Blurpi => (0.90, 0.15, 0.0, 0.10, 0.12),
            Self::Royale => (0.45, 0.50, 0.06, 0.45, 0.30),
        };
        ShaderSettings {
            preset: self,
            bloom,
            scanlines,
            curvature,
            mask,
            vignette,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShaderSettings {
    pub preset: ShaderPreset,
    pub bloom: f32,
    pub scanlines: f32,
    pub curvature: f32,
    pub mask: f32,
    pub vignette: f32,
}

impl ShaderSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            preset: ShaderPreset::parse(&config.shader_preset),
            bloom: config.shader_bloom.clamp(0.0, 2.0),
            scanlines: config.shader_scanlines.clamp(0.0, 1.0),
            curvature: config.shader_curvature.clamp(0.0, 0.25),
            mask: config.shader_mask.clamp(0.0, 1.0),
            vignette: config.shader_vignette.clamp(0.0, 1.0),
        }
    }
}

#[derive(Default)]
pub struct ShaderState {
    renderer: Option<ShaderRenderer>,
    error: Option<String>,
}

impl ShaderState {
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn paint(
        &mut self,
        painter: &egui_glow::Painter,
        info: egui::PaintCallbackInfo,
        texture_id: egui::TextureId,
        game_size: [usize; 2],
        settings: ShaderSettings,
    ) -> Result<()> {
        if settings.preset == ShaderPreset::Off {
            return Ok(());
        }
        let Some(game_texture) = painter.texture(texture_id) else {
            return Err(anyhow!("game texture is not available to the shader"));
        };
        if self.renderer.is_none() {
            match ShaderRenderer::new(painter.gl().clone()) {
                Ok(renderer) => self.renderer = Some(renderer),
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    return Err(e);
                }
            }
        }
        let result = self
            .renderer
            .as_mut()
            .expect("shader renderer initialized above")
            .paint(
                painter.gl(),
                painter.intermediate_fbo(),
                game_texture,
                info,
                game_size,
                settings,
            );
        if let Err(e) = &result {
            self.error = Some(format!("{e:#}"));
        }
        result
    }
}

struct ShaderRenderer {
    gl: Arc<glow::Context>,
    bright_program: glow::Program,
    blur_program: glow::Program,
    composite_program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    bloom_fbo: [Option<glow::Framebuffer>; 2],
    bloom_tex: [Option<glow::Texture>; 2],
    bloom_size: (u32, u32),
}

impl ShaderRenderer {
    fn new(gl: Arc<glow::Context>) -> Result<Self> {
        let version = egui_glow::ShaderVersion::get(&gl);
        let vertex = vertex_source(version);
        let bright = fragment_source(version, BRIGHT_BODY);
        let blur = fragment_source(version, BLUR_BODY);
        let composite = fragment_source(version, COMPOSITE_BODY);

        let bright_program = create_program(&gl, &vertex, &bright)
            .map_err(|e| anyhow!("compiling the bright-pass shader: {e}"))?;
        let blur_program = create_program(&gl, &vertex, &blur)
            .map_err(|e| anyhow!("compiling the blur shader: {e}"))?;
        let composite_program = create_program(&gl, &vertex, &composite)
            .map_err(|e| anyhow!("compiling the CRT shader: {e}"))?;

        let vertices: [f32; 12] = [
            -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
        ];
        let vao = unsafe { gl.create_vertex_array() }
            .map_err(|e| anyhow!("creating the shader VAO: {e}"))?;
        let vbo =
            unsafe { gl.create_buffer() }.map_err(|e| anyhow!("creating the shader VBO: {e}"))?;
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                std::slice::from_raw_parts(
                    vertices.as_ptr() as *const u8,
                    vertices.len() * std::mem::size_of::<f32>(),
                ),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }

        Ok(Self {
            gl,
            bright_program,
            blur_program,
            composite_program,
            vao,
            vbo,
            bloom_fbo: [None, None],
            bloom_tex: [None, None],
            bloom_size: (0, 0),
        })
    }

    fn paint(
        &mut self,
        gl: &glow::Context,
        target: Option<glow::Framebuffer>,
        game_texture: glow::Texture,
        info: egui::PaintCallbackInfo,
        game_size: [usize; 2],
        settings: ShaderSettings,
    ) -> Result<()> {
        let viewport = info.viewport_in_pixels();
        let game_width = game_size[0].max(1) as u32;
        let game_height = game_size[1].max(1) as u32;
        self.ensure_bloom(gl, game_width, game_height)?;
        let (bloom_width, bloom_height) = self.bloom_size;
        let bloom_a = self.bloom_fbo[0].ok_or_else(|| anyhow!("missing bloom FBO A"))?;
        let bloom_b = self.bloom_fbo[1].ok_or_else(|| anyhow!("missing bloom FBO B"))?;
        let bloom_tex_a = self.bloom_tex[0].ok_or_else(|| anyhow!("missing bloom texture A"))?;
        let bloom_tex_b = self.bloom_tex[1].ok_or_else(|| anyhow!("missing bloom texture B"))?;

        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.bind_vertex_array(Some(self.vao));

            // Bright pass.
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(bloom_a));
            gl.viewport(0, 0, bloom_width as i32, bloom_height as i32);
            gl.use_program(Some(self.bright_program));
            bind_texture(gl, 0, Some(game_texture));
            set_i32(gl, self.bright_program, "u_game", 0);
            set_2f(
                gl,
                self.bright_program,
                "u_game_size",
                game_width as f32,
                game_height as f32,
            );
            set_1f(gl, self.bright_program, "u_threshold", 0.55);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            // Horizontal blur.
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(bloom_b));
            gl.viewport(0, 0, bloom_width as i32, bloom_height as i32);
            gl.use_program(Some(self.blur_program));
            bind_texture(gl, 0, Some(bloom_tex_a));
            set_i32(gl, self.blur_program, "u_source", 0);
            set_2f(
                gl,
                self.blur_program,
                "u_texel",
                1.0 / bloom_width as f32,
                1.0 / bloom_height as f32,
            );
            set_2f(gl, self.blur_program, "u_dir", 1.0, 0.0);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            // Vertical blur.
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(bloom_a));
            gl.viewport(0, 0, bloom_width as i32, bloom_height as i32);
            bind_texture(gl, 0, Some(bloom_tex_b));
            set_2f(gl, self.blur_program, "u_dir", 0.0, 1.0);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            // CRT composite.
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.viewport(
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            );
            gl.use_program(Some(self.composite_program));
            bind_texture(gl, 0, Some(game_texture));
            bind_texture(gl, 1, Some(bloom_tex_a));
            set_i32(gl, self.composite_program, "u_game", 0);
            set_i32(gl, self.composite_program, "u_bloom", 1);
            set_2f(
                gl,
                self.composite_program,
                "u_game_size",
                game_width as f32,
                game_height as f32,
            );
            set_1f(
                gl,
                self.composite_program,
                "u_bloom_strength",
                settings.bloom,
            );
            set_1f(
                gl,
                self.composite_program,
                "u_scanline_strength",
                settings.scanlines,
            );
            set_1f(
                gl,
                self.composite_program,
                "u_curvature",
                settings.curvature,
            );
            set_1f(gl, self.composite_program, "u_mask_strength", settings.mask);
            set_1f(gl, self.composite_program, "u_vignette", settings.vignette);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        Ok(())
    }

    fn ensure_bloom(
        &mut self,
        gl: &glow::Context,
        game_width: u32,
        game_height: u32,
    ) -> Result<()> {
        let size = ((game_width / 2).max(1), (game_height / 2).max(1));
        if self.bloom_size == size && self.bloom_fbo.iter().all(Option::is_some) {
            return Ok(());
        }
        unsafe {
            for fbo in self.bloom_fbo.iter().flatten() {
                gl.delete_framebuffer(*fbo);
            }
            for texture in self.bloom_tex.iter().flatten() {
                gl.delete_texture(*texture);
            }
            self.bloom_fbo = [None, None];
            self.bloom_tex = [None, None];

            for index in 0..2 {
                let texture = gl
                    .create_texture()
                    .map_err(|e| anyhow!("creating bloom texture {index}: {e}"))?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    size.0 as i32,
                    size.1 as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);

                let fbo = gl
                    .create_framebuffer()
                    .map_err(|e| anyhow!("creating bloom FBO {index}: {e}"))?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(texture);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    return Err(anyhow!("bloom FBO {index} is incomplete ({status:#x})"));
                }
                self.bloom_fbo[index] = Some(fbo);
                self.bloom_tex[index] = Some(texture);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.bloom_size = size;
        Ok(())
    }
}

impl Drop for ShaderRenderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.bright_program);
            self.gl.delete_program(self.blur_program);
            self.gl.delete_program(self.composite_program);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_buffer(self.vbo);
            for fbo in self.bloom_fbo.iter().flatten() {
                self.gl.delete_framebuffer(*fbo);
            }
            for texture in self.bloom_tex.iter().flatten() {
                self.gl.delete_texture(*texture);
            }
        }
    }
}

fn bind_texture(gl: &glow::Context, unit: u32, texture: Option<glow::Texture>) {
    unsafe {
        gl.active_texture(glow::TEXTURE0 + unit);
        gl.bind_texture(glow::TEXTURE_2D, texture);
    }
}

fn set_i32(gl: &glow::Context, program: glow::Program, name: &str, value: i32) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_1_i32(Some(&location), value);
        }
    }
}

fn set_1f(gl: &glow::Context, program: glow::Program, name: &str, value: f32) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_1_f32(Some(&location), value);
        }
    }
}

fn set_2f(gl: &glow::Context, program: glow::Program, name: &str, x: f32, y: f32) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_2_f32(Some(&location), x, y);
        }
    }
}

fn create_program(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program> {
    unsafe {
        let program = gl
            .create_program()
            .map_err(|e| anyhow!("creating program: {e}"))?;
        let vs = compile_shader(gl, glow::VERTEX_SHADER, vertex)?;
        let fs = compile_shader(gl, glow::FRAGMENT_SHADER, fragment)?;
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.bind_attrib_location(program, 0, "a_pos");
        gl.link_program(program);
        let linked = gl.get_program_link_status(program);
        let log = gl.get_program_info_log(program);
        gl.detach_shader(program, vs);
        gl.detach_shader(program, fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !linked {
            gl.delete_program(program);
            return Err(anyhow!("link failed: {log}"));
        }
        if !log.trim().is_empty() {
            log::debug!("shader link log: {log}");
        }
        Ok(program)
    }
}

fn compile_shader(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader> {
    unsafe {
        let shader = gl
            .create_shader(kind)
            .map_err(|e| anyhow!("creating shader: {e}"))?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(anyhow!("compile failed: {log}\n{source}"));
        }
        Ok(shader)
    }
}

fn vertex_source(version: egui_glow::ShaderVersion) -> String {
    let header = version.version_declaration();
    let precision = if version.is_embedded() {
        "precision highp float;\n"
    } else {
        ""
    };
    let (input, output) = if version.is_new_shader_interface() {
        ("in", "out")
    } else {
        ("attribute", "varying")
    };
    format!(
        "{header}{precision}{input} vec2 a_pos;\n{output} vec2 v_uv;\n\
         void main() {{\n\
           v_uv = a_pos * 0.5 + 0.5;\n\
           gl_Position = vec4(a_pos, 0.0, 1.0);\n\
         }}\n"
    )
}

fn fragment_source(version: egui_glow::ShaderVersion, body: &str) -> String {
    let header = version.version_declaration();
    let precision = if version.is_embedded() {
        "precision mediump float;\n"
    } else {
        ""
    };
    let (input, output, tex, frag) = if version.is_new_shader_interface() {
        ("in", "out vec4 frag_color;", "texture", "frag_color")
    } else {
        ("varying", "", "texture2D", "gl_FragColor")
    };
    format!(
        "{header}{precision}{input} vec2 v_uv;\n{output}\n\
         #define TEX {tex}\n#define FRAG_COLOR {frag}\n{body}\n"
    )
}

const BRIGHT_BODY: &str = r#"
uniform sampler2D u_game;
uniform vec2 u_game_size;
uniform float u_threshold;

void main() {
    // egui game textures use a top-left origin, while the bloom FBOs use
    // OpenGL's bottom-left origin. Flip only the game sample so bloom stays
    // aligned with the screen-space effects in the composite pass.
    vec2 game_uv = vec2(v_uv.x, 1.0 - v_uv.y);
    vec2 texel = 1.0 / u_game_size;
    vec3 color = (
        TEX(u_game, game_uv + vec2(-0.5, -0.5) * texel).rgb +
        TEX(u_game, game_uv + vec2( 0.5, -0.5) * texel).rgb +
        TEX(u_game, game_uv + vec2(-0.5,  0.5) * texel).rgb +
        TEX(u_game, game_uv + vec2( 0.5,  0.5) * texel).rgb
    ) * 0.25;
    float luma = dot(color, vec3(0.299, 0.587, 0.114));
    float bright = smoothstep(u_threshold, 1.0, luma);
    FRAG_COLOR = vec4(color * bright, 1.0);
}
"#;

const BLUR_BODY: &str = r#"
uniform sampler2D u_source;
uniform vec2 u_texel;
uniform vec2 u_dir;

void main() {
    vec2 step = u_dir * u_texel;
    vec3 color = TEX(u_source, v_uv).rgb * 0.227027;
    color += TEX(u_source, v_uv + step * 1.3846).rgb * 0.1945946;
    color += TEX(u_source, v_uv - step * 1.3846).rgb * 0.1945946;
    color += TEX(u_source, v_uv + step * 3.2308).rgb * 0.1216216;
    color += TEX(u_source, v_uv - step * 3.2308).rgb * 0.1216216;
    color += TEX(u_source, v_uv + step * 5.0769).rgb * 0.054054;
    color += TEX(u_source, v_uv - step * 5.0769).rgb * 0.054054;
    color += TEX(u_source, v_uv + step * 6.9231).rgb * 0.016216;
    color += TEX(u_source, v_uv - step * 6.9231).rgb * 0.016216;
    FRAG_COLOR = vec4(color, 1.0);
}
"#;

const COMPOSITE_BODY: &str = r#"
uniform sampler2D u_game;
uniform sampler2D u_bloom;
uniform vec2 u_game_size;
uniform float u_bloom_strength;
uniform float u_scanline_strength;
uniform float u_curvature;
uniform float u_mask_strength;
uniform float u_vignette;

void main() {
    vec2 centered = v_uv * 2.0 - 1.0;
    float radius2 = dot(centered, centered);
    vec2 uv = (centered * (1.0 + radius2 * u_curvature)) * 0.5 + 0.5;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        FRAG_COLOR = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    // The game texture is top-left origin (egui), unlike the bloom texture
    // written by our own FBO passes. Sample the game with a flipped V and
    // keep `uv` for the bloom and screen-space CRT effects.
    vec2 game_uv = vec2(uv.x, 1.0 - uv.y);
    vec3 color = TEX(u_game, game_uv).rgb;
    vec3 bloom = TEX(u_bloom, uv).rgb;
    color += bloom * u_bloom_strength;

    float scanline = 0.5 + 0.5 * cos(uv.y * u_game_size.y * 3.14159265);
    color *= mix(1.0, scanline, u_scanline_strength);

    float m = mod(gl_FragCoord.x, 3.0);
    vec3 mask;
    if (m < 1.0) {
        mask = vec3(1.0, 1.0 - u_mask_strength, 1.0 - u_mask_strength);
    } else if (m < 2.0) {
        mask = vec3(1.0 - u_mask_strength, 1.0, 1.0 - u_mask_strength);
    } else {
        mask = vec3(1.0 - u_mask_strength, 1.0 - u_mask_strength, 1.0);
    }
    color *= mask;
    color *= clamp(1.0 - u_vignette * radius2, 0.0, 1.0);
    FRAG_COLOR = vec4(clamp(color, 0.0, 1.0), 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip() {
        for preset in ShaderPreset::ALL {
            assert_eq!(ShaderPreset::parse(preset.as_str()), preset);
        }
        assert_eq!(ShaderPreset::parse("unknown"), ShaderPreset::Off);
    }

    #[test]
    fn off_preset_has_no_effect() {
        let settings = ShaderPreset::Off.defaults();
        assert_eq!(settings.bloom, 0.0);
        assert_eq!(settings.scanlines, 0.0);
        assert_eq!(settings.curvature, 0.0);
        assert_eq!(settings.mask, 0.0);
        assert_eq!(settings.vignette, 0.0);
    }

    #[test]
    fn settings_clamp_from_config() {
        let config = Config {
            shader_preset: "soft_crt".into(),
            shader_bloom: 9.0,
            shader_scanlines: -1.0,
            shader_curvature: 9.0,
            shader_mask: -1.0,
            shader_vignette: 9.0,
            ..Config::default()
        };
        let settings = ShaderSettings::from_config(&config);
        assert_eq!(settings.preset, ShaderPreset::SoftCrt);
        assert_eq!(settings.bloom, 2.0);
        assert_eq!(settings.scanlines, 0.0);
        assert_eq!(settings.curvature, 0.25);
        assert_eq!(settings.mask, 0.0);
        assert_eq!(settings.vignette, 1.0);
    }
}
