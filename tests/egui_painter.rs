//! egui-Painter auf D3D11: Rechteck in ein Offscreen-Ziel zeichnen und zurücklesen; Zielgröße 0
//! (minimiertes Fenster, Review-Fokus 3) wird ignoriert.

use airplay::render::egui_painter::EguiPainter;
use egui::{Color32, Rect, pos2, vec2};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

const SIZE: u32 = 64;

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    target: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
}

fn create_device(driver: D3D_DRIVER_TYPE) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    // SAFETY: Ausgabezeiger sind lokal.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .ok()?;
    }
    Some((device?, context?))
}

/// D3D11-Device ohne Fenster mit einem 64×64-Ziel wie die Swapchain (B8G8R8A8_UNORM).
fn gpu() -> Gpu {
    let (device, context) = create_device(D3D_DRIVER_TYPE_HARDWARE)
        .or_else(|| create_device(D3D_DRIVER_TYPE_WARP))
        .expect("D3D11CreateDevice");
    let td = D3D11_TEXTURE2D_DESC {
        Width: SIZE,
        Height: SIZE,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let (mut target, mut rtv) = (None, None);
    // SAFETY: Beschreibungen und Ausgabezeiger sind lokal.
    unsafe {
        device
            .CreateTexture2D(&td, None, Some(&mut target))
            .expect("Zieltextur");
        let target = target.as_ref().expect("Zieltextur");
        device
            .CreateRenderTargetView(target, None, Some(&mut rtv))
            .expect("RTV");
    }
    Gpu {
        device,
        context,
        target: target.expect("Zieltextur"),
        rtv: rtv.expect("RTV"),
    }
}

impl Gpu {
    fn clear_and_bind(&self) {
        // SAFETY: gültige Objekte.
        unsafe {
            self.context
                .OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            self.context
                .ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 1.0]);
        }
    }

    /// Pixel (x, y) als (r, g, b, a) über eine Staging-Textur.
    fn read_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let td = D3D11_TEXTURE2D_DESC {
            Width: SIZE,
            Height: SIZE,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        // SAFETY: gültige Objekte; gelesen wird innerhalb der gemappten 64×64-Textur.
        unsafe {
            self.device
                .CreateTexture2D(&td, None, Some(&mut staging))
                .expect("Staging");
            let staging = staging.expect("Staging");
            self.context.CopyResource(&staging, &self.target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .expect("Map");
            let p = m
                .pData
                .cast::<u8>()
                .add(y as usize * m.RowPitch as usize + x as usize * 4);
            let bgra = [*p, *p.add(1), *p.add(2), *p.add(3)];
            self.context.Unmap(&staging, 0);
            [bgra[2], bgra[1], bgra[0], bgra[3]]
        }
    }
}

/// Ein egui-Durchlauf, der `rect` (Punkte) in `color` füllt.
fn red_rect_frame(
    ctx: &egui::Context,
    screen_pts: f32,
    rect: Rect,
    color: Color32,
) -> (Vec<egui::ClippedPrimitive>, egui::TexturesDelta, f32) {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            vec2(screen_pts, screen_pts),
        )),
        ..Default::default()
    };
    let out = ctx.run_pass(raw, |ctx| {
        ctx.layer_painter(egui::LayerId::background())
            .rect_filled(rect, 0.0, color);
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    (prims, out.textures_delta, out.pixels_per_point)
}

#[test]
fn painter_draws_rect_into_offscreen_target() {
    let g = gpu();
    let mut painter = EguiPainter::new(&g.device).expect("EguiPainter::new");
    let ctx = egui::Context::default();
    // Ganze Fläche rot, dazu ein halbtransparentes blaues Feld oben links (Mischung).
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0))),
        ..Default::default()
    };
    let mut out = ctx.run_pass(raw, |ctx| {
        let p = ctx.layer_painter(egui::LayerId::background());
        p.rect_filled(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0)),
            0.0,
            Color32::RED,
        );
        p.rect_filled(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(16.0, 16.0)),
            0.0,
            Color32::from_rgba_unmultiplied(0, 0, 255, 128),
        );
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(!prims.is_empty());
    g.clear_and_bind();
    painter.paint(
        &g.context,
        [SIZE, SIZE],
        out.pixels_per_point,
        &prims,
        &mut out.textures_delta,
    );
    assert_eq!(g.read_pixel(32, 32), [255, 0, 0, 255]);
    let mixed = g.read_pixel(4, 4);
    assert!(
        (120..=135).contains(&mixed[0]) && mixed[1] == 0 && (120..=135).contains(&mixed[2]),
        "Mischung vormultipliziert: {mixed:?}"
    );
}

#[test]
fn painter_ignores_zero_size() {
    let g = gpu();
    let mut painter = EguiPainter::new(&g.device).expect("EguiPainter::new");
    let ctx = egui::Context::default();
    let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0));
    // Der erste Durchlauf bringt die Schrift-Textur mit; bei Größe 0 muss sie trotzdem ankommen.
    let (prims, mut delta, ppp) = red_rect_frame(&ctx, 64.0, full, Color32::GREEN);
    assert!(!delta.set.is_empty(), "erster Durchlauf ohne Textur");
    g.clear_and_bind();
    painter.paint(&g.context, [0, 0], ppp, &prims, &mut delta);
    painter.paint(&g.context, [0, SIZE], ppp, &prims, &mut Default::default());
    painter.paint(&g.context, [SIZE, 0], ppp, &prims, &mut Default::default());
    painter.paint(
        &g.context,
        [SIZE, SIZE],
        0.0,
        &prims,
        &mut Default::default(),
    );
    assert_eq!(
        g.read_pixel(32, 32),
        [0, 0, 0, 255],
        "bei Größe 0 nichts gezeichnet"
    );
    // Danach normal zeichnen: Textur ist noch da.
    let (prims, mut delta, ppp) = red_rect_frame(&ctx, 64.0, full, Color32::GREEN);
    painter.paint(&g.context, [SIZE, SIZE], ppp, &prims, &mut delta);
    assert_eq!(g.read_pixel(32, 32), [0, 255, 0, 255]);
}
