//! HLSL-Quelltext des Videoshaders (wie v1 `shaders.hlsl.h`), zur Laufzeit per `D3DCompile`.

/// Vollbild-Dreieck aus SV_VertexID; Viewport bestimmt das Zielrechteck.
pub const VIDEO_SHADER: &str = r#"
cbuffer Params : register(b0) {
    float fullRange;   // 1 = Full Range (0-255), 0 = Limited (16-235)
    float bt601;       // 1 = BT.601, 0 = BT.709
    float2 pad;
};
Texture2D<float>  texY  : register(t0);
Texture2D<float2> texUV : register(t1);
SamplerState samp : register(s0);

struct VsOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

VsOut vs_main(uint id : SV_VertexID) {
    VsOut o;
    float2 uv = float2((id << 1) & 2, id & 2);
    o.uv = uv;
    o.pos = float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}

float4 ps_main(VsOut i) : SV_Target {
    float y = texY.Sample(samp, i.uv);
    float2 c = texUV.Sample(samp, i.uv) - 0.5;
    if (fullRange < 0.5) {
        y = (y - 16.0 / 255.0) * (255.0 / 219.0);
        c = c * (255.0 / 224.0);
    }
    float3 rgb;
    if (bt601 > 0.5) {
        rgb = float3(y + 1.402 * c.y, y - 0.344136 * c.x - 0.714136 * c.y, y + 1.772 * c.x);
    } else {
        rgb = float3(y + 1.5748 * c.y, y - 0.187324 * c.x - 0.468124 * c.y, y + 1.8556 * c.x);
    }
    return float4(saturate(rgb), 1);
}
"#;

/// egui-Dreiecke: Position in Punkten, Farbe vormultipliziert im Gamma-Raum (wie die Textur).
pub const EGUI_SHADER: &str = r#"
cbuffer Screen : register(b0) {
    float2 screenPoints;   // Zielgröße in Punkten
    float2 pad;
};
Texture2D tex : register(t0);
SamplerState samp : register(s0);

struct VsIn {
    float2 pos : POSITION;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};
struct VsOut {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

VsOut vs_main(VsIn i) {
    VsOut o;
    o.pos = float4(2.0 * i.pos.x / screenPoints.x - 1.0, 1.0 - 2.0 * i.pos.y / screenPoints.y, 0.0, 1.0);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}

float4 ps_main(VsOut i) : SV_TARGET {
    return i.color * tex.Sample(samp, i.uv);
}
"#;
