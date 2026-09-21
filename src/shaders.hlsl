// One full-screen triangle over the viewport; a 2x3 matrix maps its UVs into
// the source texture (crop, flips, rotation and fill-crop folded into it).
cbuffer Crop : register(b0)
{
    float4 row0;   // su = row0.x*u + row0.y*v + row0.z
    float4 row1;   // sv = row1.x*u + row1.y*v + row1.z
};

Texture2D    source         : register(t0);
SamplerState source_sampler : register(s0);

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

VsOut vs_main(uint id : SV_VertexID)
{
    // ids 0,1,2 -> (0,0), (2,0), (0,2): a triangle that covers the whole viewport.
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    float3 q = float3(uv, 1.0);
    o.uv = float2(dot(row0.xyz, q), dot(row1.xyz, q));
    return o;
}

float4 ps_main(VsOut i) : SV_Target
{
    return source.Sample(source_sampler, i.uv);
}

// Selection overlay: frozen frame, dimmed outside the selection, 2 px border.
cbuffer Overlay : register(b1)
{
    float4 sel;    // u0, v0, u1, v1 of the selection
    float4 px;     // 1/w, 1/h, has_selection, unused
};

float4 ps_overlay(VsOut i) : SV_Target
{
    float4 c = source.Sample(source_sampler, i.uv);
    float4 dim = c * float4(0.4, 0.4, 0.4, 1.0);
    if (px.z < 0.5)
        return dim;
    bool inside = i.uv.x >= sel.x && i.uv.x <= sel.z && i.uv.y >= sel.y && i.uv.y <= sel.w;
    if (inside)
        return c;
    float2 b = px.xy * 2.0;
    bool border = i.uv.x >= sel.x - b.x && i.uv.x <= sel.z + b.x && i.uv.y >= sel.y - b.y && i.uv.y <= sel.w + b.y;
    return border ? float4(1.0, 0.85, 0.2, 1.0) : dim;
}
