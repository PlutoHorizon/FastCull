//! Existing OpenGL implementation for non-Windows platforms.
use super::*;
use eframe::{egui, egui_glow, glow};
use glow::HasContext;
use std::sync::{Arc, Mutex, Weak};

unsafe fn texture_parameters(gl: &glow::Context, target: u32, filter: u32) {
    unsafe {
        gl.tex_parameter_i32(target, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(target, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.tex_parameter_i32(target, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(target, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    }
}
struct GpuImage {
    image: glow::Texture,
    curve: glow::Texture,
    look: glow::Texture,
    extra_look: glow::Texture,
}
struct LinearState {
    pixels: Option<Vec<[f32; 4]>>,
    gpu: Option<GpuImage>,
}
pub struct LinearTexture {
    size: [usize; 2],
    recipe: Recipe,
    state: Mutex<LinearState>,
    garbage: Arc<Mutex<Vec<GpuImage>>>,
}
impl LinearTexture {
    pub fn size_vec2(&self) -> egui::Vec2 {
        egui::vec2(self.size[0] as f32, self.size[1] as f32)
    }
}
impl Drop for LinearTexture {
    fn drop(&mut self) {
        if let Some(gpu) = self.state.get_mut().unwrap().gpu.take() {
            // Drop can occur without a current context. Delete next paint/exit.
            self.garbage.lock().unwrap().push(gpu);
        }
    }
}

pub struct Preview {
    gl: Arc<glow::Context>,
    shader: Mutex<Option<Result<(glow::Program, glow::VertexArray), String>>>,
    linear_shader: Mutex<Option<Result<(glow::Program, glow::VertexArray), String>>>,
    linear_error: Mutex<Option<String>>,
    frames: Mutex<Vec<Weak<LinearTexture>>>,
    garbage: Arc<Mutex<Vec<GpuImage>>>,
}
impl Preview {
    pub fn new(gl: Arc<glow::Context>) -> Arc<Self> {
        Arc::new(Self {
            gl,
            shader: Mutex::new(None),
            linear_shader: Mutex::new(None),
            linear_error: Mutex::new(None),
            frames: Mutex::new(Vec::new()),
            garbage: Arc::new(Mutex::new(Vec::new())),
        })
    }
    unsafe fn compile(&self, linear: bool) -> Result<(glow::Program, glow::VertexArray), String> {
        unsafe {
            let gl = &self.gl;
            let program = gl.create_program()?;
            let sources = [
                (
                    glow::VERTEX_SHADER,
                    r#"#version 330
uniform vec4 bounds; uniform vec2 uvs[4]; out vec2 uv;
void main() {
    int indices[6] = int[6](0,1,2,0,2,3); int i = indices[gl_VertexID];
    vec2 p[4] = vec2[4](bounds.xy, vec2(bounds.z,bounds.y), bounds.zw, vec2(bounds.x,bounds.w));
    gl_Position = vec4(p[i],0,1); uv = uvs[i];
}"#,
                ),
                (
                    glow::FRAGMENT_SHADER,
                    if linear {
                        SCENE_FRAGMENT
                    } else {
                        r#"#version 330
uniform sampler2D image; uniform float gain; uniform vec4 tone; uniform vec3 chroma; uniform vec3 wb; in vec2 uv; out vec4 color;
void main() {
    if (any(lessThan(uv,vec2(0))) || any(greaterThan(uv,vec2(1)))) { color=vec4(0,0,0,1); return; }
    vec3 s = texture(image,uv).rgb;
    vec3 linear = mix(s/12.92, pow((s+0.055)/1.055,vec3(2.4)), greaterThan(s,vec3(0.04045)));
    linear = clamp(linear*gain*wb,0.0,1.0);
    vec3 out_s = mix(linear*12.92, 1.055*pow(linear,vec3(1.0/2.4))-0.055, greaterThan(linear,vec3(0.0031308)));
    float y=dot(out_s,vec3(.2126,.7152,.0722));
    float lift=tone.z*pow(1.0-y,2.0)*.35 + tone.y*y*y*.35 + tone.w*pow(y,4.0)*.25 + chroma.x*pow(1.0-y,4.0)*.25;
    out_s=clamp((out_s-.5)*(1.0+tone.x*.8)+.5+lift,0.0,1.0);
    y=dot(out_s,vec3(.2126,.7152,.0722));
    float saturation=max(out_s.r,max(out_s.g,out_s.b))-min(out_s.r,min(out_s.g,out_s.b));
    float vibrance_gain=max(0.0,1.0+chroma.y*(1.0-saturation));
    out_s=mix(vec3(y),out_s,vibrance_gain*max(0.0,1.0+chroma.z));
    color = vec4(clamp(out_s,0.0,1.0),1);
}"#
                    },
                ),
            ];
            for (kind, source) in sources {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let error = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return Err(error);
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let error = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(error);
            }
            match gl.create_vertex_array() {
                Ok(vao) => Ok((program, vao)),
                Err(e) => {
                    gl.delete_program(program);
                    Err(e)
                }
            }
        }
    }
    pub fn paint(
        self: &Arc<Self>,
        painter: &egui::Painter,
        canvas: egui::Rect,
        rect: egui::Rect,
        texture: egui::TextureId,
        uvs: [egui::Pos2; 4],
        delta: Settings,
    ) {
        let preview = self.clone();
        let bounds = [
            2. * (rect.left() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.top() - canvas.top()) / canvas.height(),
            2. * (rect.right() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.bottom() - canvas.top()) / canvas.height(),
        ];
        painter.add(egui::PaintCallback {
            rect: canvas,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_, painter| {
                let mut shader = preview.shader.lock().unwrap();
                if shader.is_none() {
                    *shader = Some(unsafe { preview.compile(false) });
                    if let Some(Err(e)) = &*shader {
                        log::error!("exposure shader: {e}");
                    }
                }
                if let Some(Ok((program, vao))) = *shader
                    && let Some(texture) = painter.texture(texture)
                {
                    unsafe {
                        let gl = painter.gl();
                        gl.use_program(Some(program));
                        gl.bind_vertex_array(Some(vao));
                        gl.active_texture(glow::TEXTURE0);
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        gl.uniform_1_i32(gl.get_uniform_location(program, "image").as_ref(), 0);
                        gl.uniform_1_f32(
                            gl.get_uniform_location(program, "gain").as_ref(),
                            2f32.powf(delta.exposure),
                        );
                        gl.uniform_4_f32_slice(
                            gl.get_uniform_location(program, "tone").as_ref(),
                            &delta.basic[..4],
                        );
                        gl.uniform_3_f32_slice(
                            gl.get_uniform_location(program, "chroma").as_ref(),
                            &delta.basic[4..],
                        );
                        gl.uniform_3_f32_slice(
                            gl.get_uniform_location(program, "wb").as_ref(),
                            &delta.wb,
                        );
                        gl.uniform_4_f32_slice(
                            gl.get_uniform_location(program, "bounds").as_ref(),
                            &bounds,
                        );
                        let uv: Vec<f32> = uvs.iter().flat_map(|p| [p.x, p.y]).collect();
                        gl.uniform_2_f32_slice(
                            gl.get_uniform_location(program, "uvs[0]").as_ref(),
                            &uv,
                        );
                        gl.draw_arrays(glow::TRIANGLES, 0, 6);
                    }
                }
            })),
        });
    }
    /// Only queues owned CPU pixels. OpenGL work occurs in a paint callback.
    pub fn upload(self: &Arc<Self>, image: LinearImage) -> Arc<LinearTexture> {
        *self.linear_error.lock().unwrap() = None;
        let frame = Arc::new(LinearTexture {
            size: image.size,
            recipe: image.recipe,
            state: Mutex::new(LinearState {
                pixels: Some(image.pixels),
                gpu: None,
            }),
            garbage: self.garbage.clone(),
        });
        let mut frames = self.frames.lock().unwrap();
        frames.retain(|frame| frame.strong_count() > 0);
        frames.push(Arc::downgrade(&frame));
        frame
    }
    unsafe fn delete_image(&self, image: GpuImage) {
        unsafe {
            self.gl.delete_texture(image.image);
            self.gl.delete_texture(image.curve);
            self.gl.delete_texture(image.look);
            gl.delete_texture(image.extra_look);
        }
    }
    unsafe fn collect_garbage(&self) {
        for image in self.garbage.lock().unwrap().drain(..) {
            unsafe { self.delete_image(image) };
        }
    }
    unsafe fn upload_frame(
        &self,
        frame: &LinearTexture,
        pixels: &[[f32; 4]],
    ) -> Result<GpuImage, String> {
        unsafe {
            let gl = &self.gl;
            let [w, h] = frame.size;
            let limit = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as usize;
            if w == 0 || h == 0 || w > limit || h > limit || w.checked_mul(h) != Some(pixels.len())
            {
                return Err("浮点 RAW 预览尺寸无效或超过显卡纹理上限".into());
            }
            if pixels.iter().flatten().any(|v| !v.is_finite()) {
                return Err("浮点 RAW 预览含无效数值".into());
            }
            if !valid_curve(&frame.recipe.curve) || !frame.recipe.baseline_exposure.is_finite() {
                return Err("RAW 色调曲线或曝光基线无效".into());
            }
            if [frame.recipe.look.as_ref(), frame.recipe.extra_look.as_ref()]
                .into_iter()
                .flatten()
                .any(|m| !valid_lookup(m) || m.encoding > 1)
            {
                return Err("RAW 色彩查找表无效".into());
            }
            let image = gl.create_texture()?;
            let curve = match gl.create_texture() {
                Ok(value) => value,
                Err(error) => {
                    gl.delete_texture(image);
                    return Err(error);
                }
            };
            let look = match gl.create_texture() {
                Ok(value) => value,
                Err(error) => {
                    gl.delete_texture(image);
                    gl.delete_texture(curve);
                    return Err(error);
                }
            };
            let extra_look = match gl.create_texture() {
                Ok(v) => v,
                Err(e) => {
                    gl.delete_texture(image);
                    gl.delete_texture(curve);
                    gl.delete_texture(look);
                    return Err(e);
                }
            };
            let result = GpuImage {
                image,
                curve,
                look,
                extra_look,
            };
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(image));
            texture_parameters(gl, glow::TEXTURE_2D, glow::LINEAR);
            let bytes = std::slice::from_raw_parts(
                pixels.as_ptr().cast::<u8>(),
                std::mem::size_of_val(pixels),
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA32F as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
            let identity = [0., 1.];
            let curve_data: &[f32] = if valid_curve(&frame.recipe.curve) {
                &frame.recipe.curve
            } else {
                &identity
            };
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(curve));
            texture_parameters(gl, glow::TEXTURE_2D, glow::NEAREST);
            let bytes = std::slice::from_raw_parts(
                curve_data.as_ptr().cast::<u8>(),
                std::mem::size_of_val(curve_data),
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R32F as i32,
                curve_data.len() as i32,
                1,
                0,
                glow::RED,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
            let identity = HsvLookup {
                dims: [1, 1, 1],
                data: vec![[0., 1., 1.]],
                encoding: 0,
            };
            for (unit, texture, map) in [
                (2, look, frame.recipe.look.as_ref()),
                (3, extra_look, frame.recipe.extra_look.as_ref()),
            ] {
                let lookup = map.filter(|v| valid_lookup(v)).unwrap_or(&identity);
                let [nh, ns, nv] = lookup.dims;
                let lookup_limit = gl.get_parameter_i32(glow::MAX_3D_TEXTURE_SIZE) as usize;
                if lookup.dims.into_iter().any(|n| n > lookup_limit) {
                    self.delete_image(result);
                    return Err("RAW 色彩查找表超过显卡上限".into());
                }
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_3D, Some(texture));
                texture_parameters(gl, glow::TEXTURE_3D, glow::NEAREST);
                gl.tex_parameter_i32(
                    glow::TEXTURE_3D,
                    glow::TEXTURE_WRAP_R,
                    glow::CLAMP_TO_EDGE as i32,
                );
                let bytes = std::slice::from_raw_parts(
                    lookup.data.as_ptr().cast::<u8>(),
                    std::mem::size_of_val(lookup.data.as_slice()),
                );
                gl.tex_image_3d(
                    glow::TEXTURE_3D,
                    0,
                    glow::RGB32F as i32,
                    ns as i32,
                    nh as i32,
                    nv as i32,
                    0,
                    glow::RGB,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytes)),
                );
            }
            gl.active_texture(glow::TEXTURE0);
            let status = gl.get_error();
            if status != glow::NO_ERROR {
                self.delete_image(result);
                return Err(format!("浮点 RAW 纹理上传失败（OpenGL {status:#x}）"));
            }
            Ok(result)
        }
    }
    pub fn paint_linear(
        self: &Arc<Self>,
        painter: &egui::Painter,
        canvas: egui::Rect,
        rect: egui::Rect,
        texture: Arc<LinearTexture>,
        uvs: [egui::Pos2; 4],
        settings: Settings,
    ) {
        let preview = self.clone();
        let bounds = [
            2. * (rect.left() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.top() - canvas.top()) / canvas.height(),
            2. * (rect.right() - canvas.left()) / canvas.width() - 1.,
            1. - 2. * (rect.bottom() - canvas.top()) / canvas.height(),
        ];
        painter.add(egui::PaintCallback {
            rect: canvas,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_, painter| {
                unsafe {
                    preview.collect_garbage();
                }
                let mut shader = preview.linear_shader.lock().unwrap();
                if shader.is_none() {
                    *shader = Some(unsafe { preview.compile(true) });
                }
                let Some(Ok((program, vao))) = *shader else {
                    return;
                };
                let mut state = texture.state.lock().unwrap();
                if let Some(pixels) = state.pixels.take() {
                    match unsafe { preview.upload_frame(&texture, &pixels) } {
                        Ok(gpu) => state.gpu = Some(gpu),
                        Err(error) => {
                            log::error!("linear RAW upload: {error}");
                            *preview.linear_error.lock().unwrap() = Some(error);
                        }
                    }
                }
                let Some(gpu) = &state.gpu else {
                    return;
                };
                unsafe {
                    let gl = painter.gl();
                    gl.use_program(Some(program));
                    gl.bind_vertex_array(Some(vao));
                    for (unit, target, image, uniform) in [
                        (0, glow::TEXTURE_2D, gpu.image, "image"),
                        (1, glow::TEXTURE_2D, gpu.curve, "curve"),
                        (2, glow::TEXTURE_3D, gpu.look, "look_table"),
                        (3, glow::TEXTURE_3D, gpu.extra_look, "extra_look_table"),
                    ] {
                        gl.active_texture(glow::TEXTURE0 + unit);
                        gl.bind_texture(target, Some(image));
                        gl.uniform_1_i32(
                            gl.get_uniform_location(program, uniform).as_ref(),
                            unit as i32,
                        );
                    }
                    let encoded = texture
                        .recipe
                        .look
                        .as_ref()
                        .is_some_and(|v| v.encoding == 1 && v.dims[2] > 1);
                    gl.uniform_1_i32(
                        gl.get_uniform_location(program, "look_encoded").as_ref(),
                        i32::from(encoded),
                    );
                    gl.uniform_1_i32(
                        gl.get_uniform_location(program, "extra_look_encoded")
                            .as_ref(),
                        i32::from(
                            texture
                                .recipe
                                .extra_look
                                .as_ref()
                                .is_some_and(|m| m.encoding == 1 && m.dims[2] > 1),
                        ),
                    );
                    gl.uniform_1_f32(
                        gl.get_uniform_location(program, "gain").as_ref(),
                        2f32.powf(
                            (settings.exposure + texture.recipe.baseline_exposure).clamp(-20., 20.),
                        ),
                    );
                    gl.uniform_4_f32_slice(
                        gl.get_uniform_location(program, "tone").as_ref(),
                        &settings.basic[..4],
                    );
                    gl.uniform_3_f32_slice(
                        gl.get_uniform_location(program, "chroma").as_ref(),
                        &settings.basic[4..],
                    );
                    gl.uniform_3_f32_slice(
                        gl.get_uniform_location(program, "wb").as_ref(),
                        &settings.wb,
                    );
                    gl.uniform_4_f32_slice(
                        gl.get_uniform_location(program, "bounds").as_ref(),
                        &bounds,
                    );
                    let uv: Vec<f32> = uvs.iter().flat_map(|p| [p.x, p.y]).collect();
                    gl.uniform_2_f32_slice(
                        gl.get_uniform_location(program, "uvs[0]").as_ref(),
                        &uv,
                    );
                    gl.draw_arrays(glow::TRIANGLES, 0, 6);
                    gl.active_texture(glow::TEXTURE0);
                }
            })),
        });
    }
    pub fn error(&self) -> Option<String> {
        let upload_error = self.linear_error.lock().unwrap().clone();
        upload_error
            .or_else(|| {
                self.linear_shader
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|s| s.as_ref().err().cloned())
            })
            .or_else(|| {
                self.shader
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|s| s.as_ref().err().cloned())
            })
    }
    pub fn destroy(&self) {
        for frame in self
            .frames
            .lock()
            .unwrap()
            .drain(..)
            .filter_map(|v| v.upgrade())
        {
            if let Some(gpu) = frame.state.lock().unwrap().gpu.take() {
                unsafe {
                    self.delete_image(gpu);
                }
            }
        }
        unsafe {
            self.collect_garbage();
        }
        if let Some(Ok((program, vao))) = self.linear_shader.lock().unwrap().take() {
            unsafe {
                self.gl.delete_program(program);
                self.gl.delete_vertex_array(vao);
            }
        }
        if let Some(Ok((program, vao))) = self.shader.lock().unwrap().take() {
            unsafe {
                self.gl.delete_program(program);
                self.gl.delete_vertex_array(vao);
            }
        }
    }
}
