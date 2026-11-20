//! egui-Zeichenbefehle (`ClippedPrimitive`) auf D3D11: ein Vertex-/Indexpuffer je Bild, Scissor
//! aus `clip_rect`, Texturen als `R8G8B8A8_UNORM` (auch Teil-Updates und Freigaben).
//!
//! Alles im Gamma-Raum: egui liefert sRGB-Farben vormultipliziert, die Swapchain ist
//! `B8G8R8A8_UNORM` (kein sRGB) → Mischung `ONE / INV_SRC_ALPHA` direkt auf den Werten.
//! Gezeichnet wird in das gerade gebundene Render-Target (nach dem Videobild).

use std::collections::HashMap;
use std::mem::{offset_of, size_of};

use egui::epaint::{ImageData, ImageDelta, Primitive, Vertex};
use egui::{TextureFilter, TextureId, TextureOptions, TextureWrapMode, TexturesDelta};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::app::log::LogLevel;
use crate::log_msg;
use crate::render::renderer::{blob_bytes, compile};
use crate::render::shaders::EGUI_SHADER;

struct Texture {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    sampler: ID3D11SamplerState,
    w: usize,
    h: usize,
}

/// Ein Zeichenaufruf: Indexbereich, Basis-Vertex, Textur und Scissor (Pixel).
struct DrawCmd {
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
    texture: TextureId,
    scissor: RECT,
}

pub struct EguiPainter {
    device: ID3D11Device,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    screen: ID3D11Buffer,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    vb: Option<ID3D11Buffer>,
    vb_cap: usize,
    ib: Option<ID3D11Buffer>,
    ib_cap: usize,
    textures: HashMap<TextureId, Texture>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    cmds: Vec<DrawCmd>,
}

fn fail(text: String) -> String {
    log_msg!(LogLevel::Error, "{}", text);
    text
}

fn dynamic_buffer(
    device: &ID3D11Device,
    bytes: usize,
    bind: D3D11_BIND_FLAG,
) -> Option<ID3D11Buffer> {
    let bd = D3D11_BUFFER_DESC {
        ByteWidth: u32::try_from(bytes).ok()?,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    let mut b = None;
    // SAFETY: Beschreibung und Ausgabezeiger sind lokal.
    if let Err(e) = unsafe { device.CreateBuffer(&bd, None, Some(&mut b)) } {
        log_msg!(LogLevel::Error, "egui: Puffer {} Byte: {}", bytes, e);
        return None;
    }
    b
}

/// Daten in einen dynamischen Puffer schreiben (WRITE_DISCARD).
fn upload<T: Copy>(ctx: &ID3D11DeviceContext, buf: &ID3D11Buffer, data: &[T]) -> bool {
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: Der Puffer fasst mindestens `data.len()` Elemente (siehe `ensure_buffers`).
    unsafe {
        if ctx
            .Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))
            .is_err()
        {
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), m.pData.cast::<T>(), data.len());
        ctx.Unmap(buf, 0);
    }
    true
}

fn d3d_filter(o: &TextureOptions) -> D3D11_FILTER {
    match (o.minification, o.magnification) {
        (TextureFilter::Linear, TextureFilter::Linear) => D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        (TextureFilter::Nearest, TextureFilter::Nearest) => D3D11_FILTER_MIN_MAG_MIP_POINT,
        (TextureFilter::Nearest, TextureFilter::Linear) => D3D11_FILTER_MIN_POINT_MAG_MIP_LINEAR,
        (TextureFilter::Linear, TextureFilter::Nearest) => D3D11_FILTER_MIN_LINEAR_MAG_MIP_POINT,
    }
}

fn d3d_address(o: &TextureOptions) -> D3D11_TEXTURE_ADDRESS_MODE {
    match o.wrap_mode {
        TextureWrapMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
        TextureWrapMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
        TextureWrapMode::MirroredRepeat => D3D11_TEXTURE_ADDRESS_MIRROR,
    }
}

/// RGBA-Bytes (vormultipliziert, Gamma) eines egui-Bildes.
fn rgba_bytes(image: &ImageData) -> ([usize; 2], Vec<u8>) {
    match image {
        ImageData::Color(img) => (
            img.size,
            img.pixels.iter().flat_map(|c| c.to_array()).collect(),
        ),
    }
}

impl EguiPainter {
    /// Shader, Eingabelayout und Zustände für egui auf `device`.
    pub fn new(device: &ID3D11Device) -> Result<Self, String> {
        let vs_blob = compile(EGUI_SHADER, s!("egui"), s!("vs_main"), s!("vs_4_0"))?;
        let ps_blob = compile(EGUI_SHADER, s!("egui"), s!("ps_main"), s!("ps_4_0"))?;
        let elements = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: offset_of!(Vertex, pos) as u32,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: offset_of!(Vertex, uv) as u32,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("COLOR"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                InputSlot: 0,
                AlignedByteOffset: offset_of!(Vertex, color) as u32,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];
        let rt = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_INV_DEST_ALPHA,
            DestBlendAlpha: D3D11_BLEND_ONE,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let blend_desc = D3D11_BLEND_DESC {
            AlphaToCoverageEnable: false.into(),
            IndependentBlendEnable: false.into(),
            RenderTarget: [rt; 8],
        };
        let raster_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            // egui hält keine feste Umlaufrichtung ein
            CullMode: D3D11_CULL_NONE,
            ScissorEnable: true.into(),
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        let (mut vs, mut ps, mut layout, mut blend, mut raster) = (None, None, None, None, None);
        // SAFETY: Beschreibungen, Bytecode und Ausgabezeiger sind lokal und gültig.
        unsafe {
            let _ = device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs));
            let _ = device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps));
            let _ = device.CreateInputLayout(&elements, blob_bytes(&vs_blob), Some(&mut layout));
            let _ = device.CreateBlendState(&blend_desc, Some(&mut blend));
            let _ = device.CreateRasterizerState(&raster_desc, Some(&mut raster));
        }
        let screen = dynamic_buffer(device, 16, D3D11_BIND_CONSTANT_BUFFER);
        match (vs, ps, layout, blend, raster, screen) {
            (Some(vs), Some(ps), Some(layout), Some(blend), Some(raster), Some(screen)) => {
                Ok(Self {
                    device: device.clone(),
                    vs,
                    ps,
                    layout,
                    screen,
                    blend,
                    raster,
                    vb: None,
                    vb_cap: 0,
                    ib: None,
                    ib_cap: 0,
                    textures: HashMap::new(),
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    cmds: Vec::new(),
                })
            }
            _ => Err(fail("egui: D3D11-Objekte nicht erzeugt".to_string())),
        }
    }

    /// Texturänderungen übernehmen (auch bei Größe 0, sonst fehlt später die Schrift), dann
    /// zeichnen, dann freigeben. `delta` ist danach leer.
    pub fn paint(
        &mut self,
        ctx: &ID3D11DeviceContext,
        target_size: [u32; 2],
        ppp: f32,
        prims: &[egui::ClippedPrimitive],
        delta: &mut TexturesDelta,
    ) {
        for (id, deltas) in delta.set.drain() {
            for d in &deltas {
                self.set_texture(ctx, id, d);
            }
        }
        let free: Vec<TextureId> = delta.free.drain().collect();
        if target_size[0] > 0 && target_size[1] > 0 && ppp.is_finite() && ppp > 0.0 {
            self.draw(ctx, target_size, ppp, prims);
        }
        for id in free {
            self.textures.remove(&id);
        }
    }

    fn set_texture(&mut self, ctx: &ID3D11DeviceContext, id: TextureId, d: &ImageDelta) {
        let ([w, h], bytes) = rgba_bytes(&d.image);
        if w == 0 || h == 0 || bytes.len() != w * h * 4 {
            return;
        }
        let pitch = (w * 4) as u32;
        match d.pos {
            Some([x, y]) => {
                let Some(t) = self.textures.get(&id) else {
                    log_msg!(
                        LogLevel::Warn,
                        "egui: Teil-Update für unbekannte Textur {:?}",
                        id
                    );
                    return;
                };
                if x + w > t.w || y + h > t.h {
                    log_msg!(
                        LogLevel::Warn,
                        "egui: Teil-Update außerhalb der Textur {:?}",
                        id
                    );
                    return;
                }
                let bx = D3D11_BOX {
                    left: x as u32,
                    top: y as u32,
                    front: 0,
                    right: (x + w) as u32,
                    bottom: (y + h) as u32,
                    back: 1,
                };
                // SAFETY: Box liegt in der Textur; `bytes` hat h Zeilen zu `pitch` Byte.
                unsafe {
                    ctx.UpdateSubresource(&t.tex, 0, Some(&bx), bytes.as_ptr().cast(), pitch, 0);
                }
            }
            None => match self.create_texture(w, h, &bytes, &d.options) {
                Some(t) => {
                    self.textures.insert(id, t);
                }
                None => log_msg!(LogLevel::Error, "egui: Textur {}x{} nicht erzeugt", w, h),
            },
        }
    }

    fn create_texture(
        &self,
        w: usize,
        h: usize,
        bytes: &[u8],
        o: &TextureOptions,
    ) -> Option<Texture> {
        let td = D3D11_TEXTURE2D_DESC {
            Width: u32::try_from(w).ok()?,
            Height: u32::try_from(h).ok()?,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: (w * 4) as u32,
            SysMemSlicePitch: 0,
        };
        let sv = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };
        let address = d3d_address(o);
        let sd = D3D11_SAMPLER_DESC {
            Filter: d3d_filter(o),
            AddressU: address,
            AddressV: address,
            AddressW: address,
            MaxLOD: D3D11_FLOAT32_MAX,
            ..Default::default()
        };
        let (mut tex, mut srv, mut sampler) = (None, None, None);
        // SAFETY: `bytes` hat h Zeilen zu w*4 Byte; Beschreibungen und Ausgabezeiger sind lokal.
        unsafe {
            self.device
                .CreateTexture2D(&td, Some(&init), Some(&mut tex))
                .ok()?;
            let tex = tex.as_ref()?;
            self.device
                .CreateShaderResourceView(tex, Some(&sv), Some(&mut srv))
                .ok()?;
            self.device
                .CreateSamplerState(&sd, Some(&mut sampler))
                .ok()?;
        }
        Some(Texture {
            tex: tex?,
            srv: srv?,
            sampler: sampler?,
            w,
            h,
        })
    }

    /// Puffer so groß machen, dass `vertices`/`indices` hineinpassen.
    fn ensure_buffers(&mut self) -> bool {
        if self.vb.is_none() || self.vertices.len() > self.vb_cap {
            let cap = (self.vertices.len() * 2).max(4096);
            self.vb = dynamic_buffer(
                &self.device,
                cap * size_of::<Vertex>(),
                D3D11_BIND_VERTEX_BUFFER,
            );
            self.vb_cap = if self.vb.is_some() { cap } else { 0 };
        }
        if self.ib.is_none() || self.indices.len() > self.ib_cap {
            let cap = (self.indices.len() * 2).max(8192);
            self.ib = dynamic_buffer(
                &self.device,
                cap * size_of::<u32>(),
                D3D11_BIND_INDEX_BUFFER,
            );
            self.ib_cap = if self.ib.is_some() { cap } else { 0 };
        }
        self.vb.is_some() && self.ib.is_some()
    }

    /// Meshes sammeln (Scissor in Pixeln, auf das Ziel begrenzt; leere übersprungen).
    fn collect(&mut self, target_size: [u32; 2], ppp: f32, prims: &[egui::ClippedPrimitive]) {
        self.vertices.clear();
        self.indices.clear();
        self.cmds.clear();
        let (w, h) = (target_size[0] as f32, target_size[1] as f32);
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else {
                continue; // eigene Zeichen-Callbacks benutzt die Oberfläche nicht
            };
            if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                continue;
            }
            let c = p.clip_rect;
            let x0 = (c.min.x * ppp).round().clamp(0.0, w);
            let y0 = (c.min.y * ppp).round().clamp(0.0, h);
            let x1 = (c.max.x * ppp).round().clamp(0.0, w);
            let y1 = (c.max.y * ppp).round().clamp(0.0, h);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let (Ok(base_vertex), Ok(first_index), Ok(index_count)) = (
                i32::try_from(self.vertices.len()),
                u32::try_from(self.indices.len()),
                u32::try_from(mesh.indices.len()),
            ) else {
                break;
            };
            self.vertices.extend_from_slice(&mesh.vertices);
            self.indices.extend_from_slice(&mesh.indices);
            self.cmds.push(DrawCmd {
                first_index,
                index_count,
                base_vertex,
                texture: mesh.texture_id,
                scissor: RECT {
                    left: x0 as i32,
                    top: y0 as i32,
                    right: x1 as i32,
                    bottom: y1 as i32,
                },
            });
        }
    }

    fn draw(
        &mut self,
        ctx: &ID3D11DeviceContext,
        target_size: [u32; 2],
        ppp: f32,
        prims: &[egui::ClippedPrimitive],
    ) {
        self.collect(target_size, ppp, prims);
        if self.cmds.is_empty() || !self.ensure_buffers() {
            return;
        }
        let (Some(vb), Some(ib)) = (self.vb.clone(), self.ib.clone()) else {
            return;
        };
        let (w, h) = (target_size[0] as f32, target_size[1] as f32);
        let screen = [w / ppp, h / ppp, 0.0, 0.0];
        if !upload(ctx, &vb, &self.vertices)
            || !upload(ctx, &ib, &self.indices)
            || !upload(ctx, &self.screen, &screen)
        {
            return;
        }
        let stride = size_of::<Vertex>() as u32;
        let offset = 0u32;
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: w,
            Height: h,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let vb_slot = Some(vb);
        // SAFETY: alle Objekte gehören zu unserem Device; Zeiger auf lokale Werte leben während
        // der Aufrufe; die Indexbereiche liegen in den eben beschriebenen Puffern.
        unsafe {
            ctx.IASetInputLayout(&self.layout);
            ctx.IASetVertexBuffers(0, 1, Some(&vb_slot), Some(&stride), Some(&offset));
            ctx.IASetIndexBuffer(&ib, DXGI_FORMAT_R32_UINT, 0);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.screen.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xffff_ffff);
            ctx.OMSetDepthStencilState(None, 0);
            ctx.RSSetState(&self.raster);
            ctx.RSSetViewports(Some(&[viewport]));
            for cmd in &self.cmds {
                let Some(t) = self.textures.get(&cmd.texture) else {
                    continue;
                };
                ctx.RSSetScissorRects(Some(&[cmd.scissor]));
                ctx.PSSetShaderResources(0, Some(&[Some(t.srv.clone())]));
                ctx.PSSetSamplers(0, Some(&[Some(t.sampler.clone())]));
                ctx.DrawIndexed(cmd.index_count, cmd.first_index, cmd.base_vertex);
            }
            // Zustände für den Videoshader im nächsten Bild zurücksetzen
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.RSSetState(None);
            ctx.OMSetBlendState(None, None, 0xffff_ffff);
            ctx.IASetInputLayout(None);
            ctx.IASetVertexBuffers(0, 1, Some(&None), Some(&0), Some(&0));
        }
    }
}
