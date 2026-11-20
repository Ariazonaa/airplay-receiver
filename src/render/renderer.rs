//! D3D11 im SDL-Fenster: Flip-Model-Swapchain mit maximal 1 Bild Vorlauf (niedrige Latenz).
//! Das Device ist multithread-geschützt, weil FFmpeg es mitbenutzt.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_TEXTURE2D, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::{Interface, PCSTR, s};

use crate::app::log::LogLevel;
use crate::ffi::ffmpeg as av;
use crate::ffi::sdl3::properties::SDL_GetPointerProperty;
use crate::ffi::sdl3::video::{
    SDL_GetWindowProperties, SDL_GetWindowSizeInPixels, SDL_PROP_WINDOW_WIN32_HWND_POINTER,
    SDL_Window,
};
use crate::log_msg;
use crate::media::video_decoder::Frame;
use crate::platform::win::paths::from_wide;
use crate::render::letterbox::Rect;
use crate::render::shaders::VIDEO_SHADER;

const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// Shader, Sampler und Konstantenpuffer für das Videobild.
struct VideoPipeline {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    params: ID3D11Buffer,
}

/// NV12-Textur mit Sichten auf Luma (R8) und Chroma (R8G8).
struct VideoTexture {
    tex: ID3D11Texture2D,
    srv_y: ID3D11ShaderResourceView,
    srv_uv: ID3D11ShaderResourceView,
    w: i32,
    h: i32,
    dynamic: bool,
}

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain2,
    target: Option<ID3D11RenderTargetView>,
    frame_wait: HANDLE,
    width: i32,
    height: i32,

    pipeline: VideoPipeline,
    video: Option<VideoTexture>,
    video_w: i32,
    video_h: i32,
    full_range: f32,
    bt601: f32,
    sws: *mut av::SwsContext,
    nv12: Vec<u8>,
}

fn hr_hex(e: &windows::core::Error) -> String {
    format!("0x{:08x}", e.code().0 as u32)
}

fn fail(text: String) -> String {
    log_msg!(LogLevel::Error, "{}", text);
    text
}

pub(crate) fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    // SAFETY: Zeiger und Größe stammen vom Blob, der während der Ausleihe lebt.
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
}

/// HLSL-Quelltext `src` (Name `name` für Fehlermeldungen) übersetzen.
pub(crate) fn compile(
    src: &str,
    name: PCSTR,
    entry: PCSTR,
    target: PCSTR,
) -> Result<ID3DBlob, String> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: Quelltext und Länge passen; Ausgabezeiger zeigen auf lokale Options.
    let r = unsafe {
        D3DCompile(
            src.as_ptr().cast::<c_void>(),
            src.len(),
            name,
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match (r, code) {
        (Ok(()), Some(code)) => Ok(code),
        _ => {
            let msg = errors.as_ref().map_or_else(
                || "?".to_string(),
                |e| {
                    String::from_utf8_lossy(blob_bytes(e))
                        .trim_end_matches('\0')
                        .to_string()
                },
            );
            Err(fail(format!("renderer: Shader: {msg}")))
        }
    }
}

fn create_video_pipeline(device: &ID3D11Device) -> Result<VideoPipeline, String> {
    let vs_blob = compile(VIDEO_SHADER, s!("video"), s!("vs_main"), s!("vs_4_0"))?;
    let ps_blob = compile(VIDEO_SHADER, s!("video"), s!("ps_main"), s!("ps_4_0"))?;
    let (mut vs, mut ps, mut sampler, mut params) = (None, None, None, None);
    let sd = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MaxLOD: D3D11_FLOAT32_MAX,
        ..Default::default()
    };
    let bd = D3D11_BUFFER_DESC {
        ByteWidth: 16,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    // SAFETY: Beschreibungen und Ausgabezeiger sind lokal und gültig.
    unsafe {
        let _ = device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs));
        let _ = device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps));
        let _ = device.CreateSamplerState(&sd, Some(&mut sampler));
        let _ = device.CreateBuffer(&bd, None, Some(&mut params));
    }
    match (vs, ps, sampler, params) {
        (Some(vs), Some(ps), Some(sampler), Some(params)) => Ok(VideoPipeline {
            vs,
            ps,
            sampler,
            params,
        }),
        _ => Err(fail("renderer: Shader: Objekte nicht erzeugt".to_string())),
    }
}

fn window_hwnd(window: *mut SDL_Window) -> HWND {
    // SAFETY: SDL prüft den Fensterzeiger selbst (ungültig → 0 bzw. null).
    let p = unsafe {
        SDL_GetPointerProperty(
            SDL_GetWindowProperties(window),
            SDL_PROP_WINDOW_WIN32_HWND_POINTER,
            null_mut(),
        )
    };
    HWND(p)
}

fn create_target(
    device: &ID3D11Device,
    swapchain: &IDXGISwapChain2,
) -> Option<ID3D11RenderTargetView> {
    let mut target = None;
    // SAFETY: Puffer 0 existiert bei einer gültigen Swapchain; Ausgabezeiger ist lokal.
    unsafe {
        let back: ID3D11Texture2D = swapchain.GetBuffer(0).ok()?;
        device
            .CreateRenderTargetView(&back, None, Some(&mut target))
            .ok()?;
    }
    target
}

impl Renderer {
    /// D3D11-Device, Swapchain und Videopipeline für das SDL-Fenster `window`.
    #[expect(clippy::not_unsafe_ptr_arg_deref)] // SDL prüft den Fensterzeiger selbst
    pub fn init(window: *mut SDL_Window) -> Result<Self, String> {
        let hwnd = window_hwnd(window);
        if hwnd.is_invalid() {
            return Err(fail("renderer: kein HWND".to_string()));
        }
        let (mut width, mut height) = (0, 0);
        // SAFETY: wie oben; Ausgabezeiger sind lokal.
        unsafe { SDL_GetWindowSizeInPixels(window, &mut width, &mut height) };
        width = width.max(1);
        height = height.max(1);

        let mut device = None;
        let mut context = None;
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        // SAFETY: Ausgabezeiger sind lokal.
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        let (device, context) = match (created, device, context) {
            (Ok(()), Some(d), Some(c)) => (d, c),
            (r, _, _) => {
                let code = r
                    .err()
                    .map_or_else(|| "0x00000000".to_string(), |e| hr_hex(&e));
                return Err(fail(format!("renderer: D3D11CreateDevice {code}")));
            }
        };
        if let Ok(mt) = context.cast::<ID3D11Multithread>() {
            // SAFETY: gültige Schnittstelle.
            let _ = unsafe { mt.SetMultithreadProtected(true) };
        }

        // SAFETY: COM-Aufrufe auf gültigen Objekten; Beschreibung ist lokal.
        let swapchain = unsafe {
            let factory: IDXGIFactory2 = device
                .cast::<IDXGIDevice>()
                .and_then(|d| d.GetAdapter())
                .and_then(|a: IDXGIAdapter| {
                    if let Ok(desc) = a.GetDesc() {
                        log_msg!(
                            LogLevel::Info,
                            "renderer: GPU {}",
                            from_wide(&desc.Description)
                        );
                    }
                    a.GetParent()
                })
                .map_err(|e| fail(format!("renderer: DXGI-Fabrik {}", hr_hex(&e))))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width as u32,
                Height: height as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Scaling: DXGI_SCALING_NONE,
                Flags: SWAP_FLAGS.0 as u32,
                ..Default::default()
            };
            let sc1 = factory
                .CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)
                .map_err(|e| fail(format!("renderer: CreateSwapChainForHwnd {}", hr_hex(&e))))?;
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            sc1.cast::<IDXGISwapChain2>()
                .map_err(|e| fail(format!("renderer: IDXGISwapChain2 {}", hr_hex(&e))))?
        };
        let pipeline = create_video_pipeline(&device)?;
        // SAFETY: gültige Swapchain mit FRAME_LATENCY_WAITABLE_OBJECT.
        let frame_wait = unsafe {
            let _ = swapchain.SetMaximumFrameLatency(1);
            swapchain.GetFrameLatencyWaitableObject()
        };
        let target = create_target(&device, &swapchain);
        Ok(Self {
            device,
            context,
            swapchain,
            target,
            frame_wait,
            width,
            height,
            pipeline,
            video: None,
            video_w: 0,
            video_h: 0,
            full_range: 0.0,
            bt601: 0.0,
            sws: null_mut(),
            nv12: Vec::new(),
        })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Swapchain an die Fenstergröße anpassen; 0 (minimiert) und unveränderte Größen werden
    /// ignoriert.
    pub fn resize(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 || (width == self.width && height == self.height) {
            return;
        }
        // SAFETY: gültige Objekte; alle Verweise auf die Puffer sind vor ResizeBuffers gelöst.
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.target = None;
            if let Err(e) = self.swapchain.ResizeBuffers(
                0,
                width as u32,
                height as u32,
                DXGI_FORMAT_UNKNOWN,
                SWAP_FLAGS,
            ) {
                log_msg!(LogLevel::Error, "renderer: ResizeBuffers {}", hr_hex(&e));
            }
        }
        self.width = width;
        self.height = height;
        self.target = create_target(&self.device, &self.swapchain);
    }

    fn full_viewport(&self) -> D3D11_VIEWPORT {
        D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width as f32,
            Height: self.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }
    }

    /// Wartet auf die Swapchain, setzt Render-Target, löscht schwarz.
    pub fn begin_frame(&mut self) {
        let black = [0.0, 0.0, 0.0, 1.0];
        // SAFETY: gültige Objekte; das Warte-Handle gehört uns.
        unsafe {
            if !self.frame_wait.is_invalid() {
                WaitForSingleObjectEx(self.frame_wait, 100, true);
            }
            self.context
                .OMSetRenderTargets(Some(std::slice::from_ref(&self.target)), None);
            if let Some(t) = &self.target {
                self.context.ClearRenderTargetView(t, &black);
            }
            self.context.RSSetViewports(Some(&[self.full_viewport()]));
        }
    }

    /// Present; false, wenn das Device verloren ging (dann Renderer neu anlegen).
    pub fn end_frame(&mut self) -> bool {
        // SAFETY: gültige Swapchain.
        let hr = unsafe { self.swapchain.Present(1, DXGI_PRESENT(0)) };
        if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
            // SAFETY: gültiges Device.
            let reason = unsafe { self.device.GetDeviceRemovedReason() };
            log_msg!(
                LogLevel::Error,
                "renderer: Grafikgerät verloren (0x{:08x})",
                reason.err().map_or(0, |e| e.code().0 as u32)
            );
            return false;
        }
        true
    }

    pub fn has_video(&self) -> bool {
        self.video_w > 0
    }

    pub fn video_width(&self) -> i32 {
        self.video_w
    }

    pub fn video_height(&self) -> i32 {
        self.video_h
    }

    pub fn clear_video(&mut self) {
        self.video = None;
        self.video_w = 0;
        self.video_h = 0;
    }

    fn ensure_video_texture(&mut self, w: i32, h: i32, dynamic: bool) -> bool {
        // NV12 braucht gerade Maße
        let (tw, th) = ((w + 1) & !1, (h + 1) & !1);
        if self
            .video
            .as_ref()
            .is_some_and(|v| v.w == tw && v.h == th && v.dynamic == dynamic)
        {
            return true;
        }
        self.clear_video();
        let td = D3D11_TEXTURE2D_DESC {
            Width: tw as u32,
            Height: th as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: if dynamic {
                D3D11_USAGE_DYNAMIC
            } else {
                D3D11_USAGE_DEFAULT
            },
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: if dynamic {
                D3D11_CPU_ACCESS_WRITE.0 as u32
            } else {
                0
            },
            MiscFlags: 0,
        };
        let mut tex = None;
        // SAFETY: Beschreibung und Ausgabezeiger sind lokal.
        if let Err(e) = unsafe { self.device.CreateTexture2D(&td, None, Some(&mut tex)) } {
            log_msg!(
                LogLevel::Error,
                "renderer: NV12-Textur {}x{} {}",
                tw,
                th,
                hr_hex(&e)
            );
            return false;
        }
        let Some(tex) = tex else { return false };
        let mut sv = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8_UNORM,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };
        let (mut srv_y, mut srv_uv) = (None, None);
        // SAFETY: Textur und Beschreibung gültig, Ausgabezeiger lokal.
        unsafe {
            let _ = self
                .device
                .CreateShaderResourceView(&tex, Some(&sv), Some(&mut srv_y));
            sv.Format = DXGI_FORMAT_R8G8_UNORM;
            let _ = self
                .device
                .CreateShaderResourceView(&tex, Some(&sv), Some(&mut srv_uv));
        }
        let (Some(srv_y), Some(srv_uv)) = (srv_y, srv_uv) else {
            return false;
        };
        self.video = Some(VideoTexture {
            tex,
            srv_y,
            srv_uv,
            w: tw,
            h: th,
            dynamic,
        });
        log_msg!(
            LogLevel::Info,
            "renderer: Videotextur {}x{} ({})",
            tw,
            th,
            if dynamic { "Software" } else { "GPU" }
        );
        true
    }

    /// Videobild übernehmen (D3D11-Frame: GPU-Kopie; Software-Frame: Upload als NV12).
    pub fn upload_frame(&mut self, frame: &Frame) -> bool {
        // SAFETY: Das Frame lebt während des Aufrufs.
        let f = unsafe { &*frame.as_ptr() };
        if f.width <= 0 || f.height <= 0 {
            return false;
        }
        self.full_range =
            if f.color_range == av::AVCOL_RANGE_JPEG || f.format == av::AV_PIX_FMT_YUVJ420P {
                1.0
            } else {
                0.0
            };
        self.bt601 =
            if f.colorspace == av::AVCOL_SPC_BT470BG || f.colorspace == av::AVCOL_SPC_SMPTE170M {
                1.0
            } else {
                0.0
            };

        let ok = if f.format == av::AV_PIX_FMT_D3D11 {
            self.upload_gpu(f)
        } else {
            self.upload_software(f)
        };
        if ok {
            self.video_w = f.width;
            self.video_h = f.height;
        }
        ok
    }

    fn upload_gpu(&mut self, f: &av::AVFrame) -> bool {
        if !self.ensure_video_texture(f.width, f.height, false) {
            return false;
        }
        let Some(v) = &self.video else { return false };
        // data[0] = ID3D11Texture2D*, data[1] = Index im Texturarray
        let raw = f.data[0].cast::<c_void>();
        // SAFETY: Bei AV_PIX_FMT_D3D11 ist data[0] eine gültige Textur auf unserem Device,
        // die das Frame während des Aufrufs festhält.
        let Some(src) = (unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }) else {
            return false;
        };
        let index = f.data[1] as usize as u32;
        let bx = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: v.w as u32,
            bottom: v.h as u32,
            back: 1,
        };
        // SAFETY: Quelle und Ziel sind gültige NV12-Texturen gleichen Formats.
        unsafe {
            self.context
                .CopySubresourceRegion(&v.tex, 0, 0, 0, 0, src, index, Some(&bx));
        }
        true
    }

    fn upload_software(&mut self, f: &av::AVFrame) -> bool {
        if !self.ensure_video_texture(f.width, f.height, true) {
            return false;
        }
        let Some(v) = &self.video else { return false };
        let (w, h) = (v.w, v.h);
        let (wu, hu) = (w as usize, h as usize);
        self.nv12.resize(wu * hu * 3 / 2, 0);
        // SAFETY: f ist ein gültiges Software-Frame; nv12 fasst Y (w*h) und UV (w*h/2);
        // die gemappte Textur hat h Zeilen Y und h/2 Zeilen UV mit RowPitch >= w.
        unsafe {
            self.sws = av::sws_getCachedContext(
                self.sws,
                f.width,
                f.height,
                f.format,
                w,
                h,
                av::AV_PIX_FMT_NV12,
                av::SWS_FAST_BILINEAR as i32,
                null_mut(),
                null_mut(),
                std::ptr::null(),
            );
            if self.sws.is_null() {
                return false;
            }
            let base = self.nv12.as_mut_ptr();
            // libswscale liest immer 4 Ebenen-Zeiger und 4 Zeilenlängen (NV12 nutzt 2).
            let dst = [base, base.add(wu * hu), null_mut(), null_mut()];
            let dst_stride = [w, w, 0, 0];
            av::sws_scale(
                self.sws,
                f.data.as_ptr().cast::<*const u8>(),
                f.linesize.as_ptr(),
                0,
                f.height,
                dst.as_ptr(),
                dst_stride.as_ptr(),
            );
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .context
                .Map(&v.tex, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))
                .is_err()
            {
                return false;
            }
            let out = m.pData.cast::<u8>();
            let pitch = m.RowPitch as usize;
            for y in 0..hu {
                std::ptr::copy_nonoverlapping(dst[0].add(y * wu), out.add(y * pitch), wu);
            }
            let out_uv = out.add(pitch * hu);
            for y in 0..hu / 2 {
                std::ptr::copy_nonoverlapping(dst[1].add(y * wu), out_uv.add(y * pitch), wu);
            }
            self.context.Unmap(&v.tex, 0);
        }
        true
    }

    /// Zuletzt übernommenes Bild in `dst` zeichnen (Fensterpixel).
    pub fn draw_video(&mut self, dst: Rect) {
        let Some(v) = &self.video else { return };
        if dst.w <= 0 || dst.h <= 0 {
            return;
        }
        let p = &self.pipeline;
        let vp = D3D11_VIEWPORT {
            TopLeftX: dst.x as f32,
            TopLeftY: dst.y as f32,
            Width: dst.w as f32,
            Height: dst.h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: alle Objekte gehören zu unserem Device; der gemappte Puffer ist 16 Byte groß.
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .context
                .Map(&p.params, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))
                .is_ok()
            {
                let values = [self.full_range, self.bt601, 0.0, 0.0];
                std::ptr::copy_nonoverlapping(values.as_ptr(), m.pData.cast::<f32>(), 4);
                self.context.Unmap(&p.params, 0);
            }
            let c = &self.context;
            c.RSSetViewports(Some(&[vp]));
            c.OMSetRenderTargets(Some(std::slice::from_ref(&self.target)), None);
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&p.vs, None);
            c.PSSetShader(&p.ps, None);
            c.PSSetShaderResources(0, Some(&[Some(v.srv_y.clone()), Some(v.srv_uv.clone())]));
            c.PSSetSamplers(0, Some(&[Some(p.sampler.clone())]));
            c.PSSetConstantBuffers(0, Some(&[Some(p.params.clone())]));
            c.OMSetBlendState(None, None, 0xffff_ffff);
            c.RSSetState(None);
            c.Draw(3, 0);
            c.PSSetShaderResources(0, Some(&[None, None]));
            c.RSSetViewports(Some(&[self.full_viewport()]));
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.clear_video();
        // SAFETY: sws gehört uns (null erlaubt); das Warte-Handle ebenso.
        unsafe {
            av::sws_freeContext(self.sws);
            self.sws = null_mut();
            self.context.ClearState();
            self.target = None;
            if !self.frame_wait.is_invalid() {
                let _ = CloseHandle(self.frame_wait);
            }
        }
    }
}
