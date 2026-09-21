// One full-screen triangle; the crop is applied by remapping its UVs.
cbuffer Crop : register(b0)
{
    float2 uv_offset; // top-left of the crop in [0,1]
    float2 uv_scale;  // size of the crop in [0,1]
};

Texture2D    source       : register(t0);
SamplerState linear_clamp : register(s0);

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
    o.uv = uv_offset + uv * uv_scale;
    return o;
}

float4 ps_main(VsOut i) : SV_Target
{
    return source.Sample(linear_clamp, i.uv);
}
